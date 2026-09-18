//! HUD(录音浮窗)唯一的拥有者与唯一入口。
//!
//! 定则(用户): **HUD 自己管理自己的状态与显示逻辑; 外界只能告诉它"发生了什么事实"**——
//! 一条新录音开始了 / 这一条录完了 / 回显内容变了 / 音量变了 / 正在润色 / 交付完了 / 出错了。
//! 由这两句话推出三条硬约束, 全部在代码层可检查:
//!   1) 外界不许读写 HUD 内部状态: 没有 `pub static HUD`、没有 `hud_set`, 结构体只在 `poll()` 里整份拷出去;
//!   2) 外界不许直接控制浮窗窗口(显示/隐藏/移动/改尺寸): 这些只发生在本模块与 `overlay.rs`(窗口层);
//!   3) 外界不许自己发 hud 事件: 事件通道在打包版不可达(C43b), 唯一读通道是 `poll()`(前端 150ms 轮询)。
//!
//! 分工: `overlay.rs` 只管窗口机制(定位/显隐/尺寸), 本模块管状态机与策略(阶段、存活、最短显示、提示);
//! 前端 `src/hud.ts` 是它的视图(把阶段映射成文案与布局)。
//!
//! 修过的坑(为什么这些规则长这样, 见 issue #7):
//!   - 状态被交付流程覆盖 → 正在录的回显被清掉: 现在"阶段"由本模块持有, 交付只报事实, 由本模块决定是否采纳;
//!   - 隐藏定时器与显示竞争(按下不显示、松手才出现) → `MIN_SHOW_MS` 最短显示守卫;
//!   - 尺寸变化触发重新贴屏(瞬间消失再出现、位置也跳) → 尺寸只在中轴线上伸缩, 位置只在"按下"时定一次。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// 对外可感知的阶段。**显示文案不在这里拼**——那是视图(hud.ts)的事, 这里只表达语义。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Recording,
    Transcribing,
    Polishing,
}

impl Phase {
    /// 线上升值(前端按这个字符串分支; 空串 = 待命)
    fn wire(self) -> &'static str {
        match self {
            Phase::Recording => "recording",
            Phase::Transcribing => "transcribing",
            Phase::Polishing => "busy",
        }
    }
    fn wire_str(self) -> String {
        self.wire().to_string()
    }
}

/// 前端唯一读到的东西(整份拷贝, 无共享可变引用)
#[derive(Default, Clone, serde::Serialize)]
pub struct Snapshot {
    pub version: u64,
    /// 阶段: recording | transcribing | busy | ""
    pub status: String,
    /// 录音中的实时回显(逐字)
    pub partial: String,
    /// 音量(RMS 0-1000 定点)
    pub level: u32,
    /// 一次性提示(轮询取后清)
    pub msg: String,
    /// 交付载荷(一次性; 带 warning 时前端延长提示)
    pub finished: Option<serde_json::Value>,
    /// 错误(一次性; 录音中不打扰, 不采纳——见 `apply_error`)
    pub err: Option<String>,
    /// 左栏: 当前段(录音中 = 实时文字; 否则 = 正在转写的那段)
    pub seg_current: Option<crate::segment_queue::SegView>,
    /// 右栏: 队列里其余段(空 = 右栏不显示)
    pub seg_queue: Vec<crate::segment_queue::SegView>,
    /// 是否有未处理的失败段(存活规则: 只剩失败时 2.5s 后收起)
    pub seg_failed: bool,
}

static HUD: Mutex<Snapshot> = Mutex::new(Snapshot {
    version: 0,
    status: String::new(),
    partial: String::new(),
    level: 0,
    msg: String::new(),
    finished: None,
    err: None,
    seg_current: None,
    seg_queue: Vec::new(),
    seg_failed: false,
});

/// 浮窗最近一次显示的时刻(毫秒, UNIX_EPOCH); 0 = 从未显示
static LAST_SHOW_MS: AtomicU64 = AtomicU64::new(0);

/// 最短显示时长(毫秒): 显示后这段时间内的隐藏请求一律忽略
pub const MIN_SHOW_MS: u64 = 400;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 唯一的状态写入路径(私有): 每次都推进 version, 前端据此去重
fn update(f: impl FnOnce(&mut Snapshot)) {
    let mut g = HUD.lock().unwrap();
    f(&mut g);
    g.version += 1;
}

fn is_recording(g: &Snapshot) -> bool {
    g.status == Phase::Recording.wire()
}

/// 隐藏请求是否应当被忽略(显示不足 `MIN_SHOW_MS`)
fn should_skip_hide(now: u64, shown: u64) -> bool {
    shown > 0 && now.saturating_sub(shown) < MIN_SHOW_MS
}

// ───────────────────────── 状态迁移(纯函数, 便于单测) ─────────────────────────

/// 新的一条录音开始: 清掉上一条的残留内容, 进入 Recording
fn apply_recording_started(g: &mut Snapshot) {
    g.status = Phase::Recording.wire_str();
    g.partial.clear();
    g.finished = None;
    g.err = None;
    g.msg.clear();
}

/// 这一条录完了(松手): 进入 Transcribing。**不动 partial**——
/// 结果要几百毫秒后才到, 此时清空回显就是用户看到的"瞬间消失再出现"。
fn apply_recording_ended(g: &mut Snapshot) {
    g.status = Phase::Transcribing.wire_str();
}

/// 交付完成: 报"交付完了"这个事实。录音中不采纳——
/// 用户可能已经在录下一段, 那段的状态不能被上一条的收尾覆盖(issue #2)。
fn apply_delivered(g: &mut Snapshot, payload: serde_json::Value) {
    if !is_recording(g) {
        g.status.clear();
    }
    g.finished = Some(payload);
}

/// 进入 LLM 润色阶段: 同样不许覆盖"录音中"
fn apply_polishing(g: &mut Snapshot) {
    if !is_recording(g) {
        g.status = Phase::Polishing.wire_str();
    }
}

/// 错误: 录音中不打扰(只落日志, 由调用方记), 不写进快照
fn apply_error(g: &mut Snapshot, reason: &str) {
    if !is_recording(g) {
        g.err = Some(reason.to_string());
    }
}

// ───────────────────────── 对外唯一接口 ─────────────────────────

/// 一条新录音开始了(按下热键)。定位并显示浮窗, 清掉上一条的内容。
pub fn recording_started(app: &tauri::AppHandle) {
    update(apply_recording_started);
    crate::overlay::position_hud_at_cursor(app);
    LAST_SHOW_MS.store(now_ms(), Ordering::SeqCst);
    crate::overlay::show_window(app);
}

/// 这一条录完了(松手)。只表达事实, 显示成什么样由本模块决定。
pub fn recording_ended(app: &tauri::AppHandle) {
    let _ = app;
    update(apply_recording_ended);
}

/// 实时回显(逐字)
pub fn partial(app: &tauri::AppHandle, text: &str) {
    let _ = app;
    let t = text.to_string();
    update(|g| g.partial = t);
}

/// 音量(RMS 0-1000 定点)
pub fn level(app: &tauri::AppHandle, lv: u32) {
    let _ = app;
    update(|g| g.level = lv);
}

/// 进入 AI 润色阶段
pub fn polishing(app: &tauri::AppHandle) {
    let _ = app;
    update(apply_polishing);
}

/// 一次性提示(静音提醒/太短丢弃/已达上限/权限缺失…)。确保窗口可见(但不重新定位)。
pub fn notify(app: &tauri::AppHandle, msg: &str) {
    let m = msg.to_string();
    update(|g| g.msg = m);
    crate::overlay::show_window(app);
}

/// 交付完成(带载荷: raw/final/llm_used/delivered/warning)
pub fn delivered(app: &tauri::AppHandle, payload: serde_json::Value) {
    let _ = app;
    update(|g| apply_delivered(g, payload));
}

/// 出错(引擎/交付级)。录音中只落日志, 不打扰录音。
pub fn error(app: &tauri::AppHandle, reason: &str) {
    let _ = app;
    crate::log::elog(&format!("[hud] error: {reason}"));
    let r = reason.to_string();
    update(|g| apply_error(g, &r));
}

/// 前端请求隐藏(唯一隐藏入口)。带最短显示守卫:
/// 前端有多个定时器来源(提示 2.5s / 完成 350ms / 失败 2.5s), 上一次的定时器可能在
/// "刚按下"之后才到期, 把刚显示的窗口又藏起来(症状: 按下不显示、松手才出现)。
pub fn hide(app: &tauri::AppHandle) {
    let now = now_ms();
    let shown = LAST_SHOW_MS.load(Ordering::SeqCst);
    if should_skip_hide(now, shown) {
        crate::log::log(
            app,
            &format!("hud hide 忽略(显示仅 {}ms < {}ms)", now.saturating_sub(shown), MIN_SHOW_MS),
        );
        return;
    }
    // 隐藏时清快照: 下次显示不会闪上一次的内容
    update(|g| {
        g.partial.clear();
        g.status.clear();
        g.finished = None;
        g.err = None;
        g.msg.clear();
    });
    crate::overlay::hide_window(app);
}

/// 前端上报内容尺寸(只改尺寸, 保持水平中心与底边; 见 `overlay::resize_window`)
pub fn resize(app: &tauri::AppHandle, width: f64, height: f64) {
    crate::overlay::resize_window(app, width.clamp(480.0, 720.0), height.clamp(92.0, 400.0));
}

/// 前端唯一读接口: 取一份快照。一次性字段(msg/err/finished)取后清。
/// 队列派生的三个字段在这里现算(分段队列的规则见 `segment_queue.rs`):
///   左栏 = 录音中显示实时文字; 没在录音时 = 正在转写的那段
///   右栏 = 队列里其余段(录音时, 正在转写的那段退到右栏)
pub fn poll() -> Snapshot {
    let (cur, mut queue) = crate::segment_queue::views();
    let mut g = HUD.lock().unwrap();
    let recording = is_recording(&g);
    g.seg_current = if recording {
        Some(crate::segment_queue::SegView {
            state: "recording".into(),
            text: g.partial.clone(),
        })
    } else {
        cur
    };
    g.seg_queue = if recording {
        if let Some(c) = crate::segment_queue::views().0 {
            queue.insert(0, c);
        }
        queue
    } else {
        queue
    };
    g.seg_failed = crate::segment_queue::has_failed();
    let snap = g.clone();
    // 一次性字段取后清(前端拿到即消费)
    g.finished = None;
    g.err = None;
    g.msg.clear();
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot::default()
    }

    // 新录音开始时, 上一条的残留内容必须清干净(否则会闪上一条)
    #[test]
    fn recording_started_clears_previous() {
        let mut g = snap();
        g.partial = "上一段".into();
        g.finished = Some(serde_json::json!({"final": "x"}));
        g.err = Some("旧错误".into());
        g.msg = "旧提示".into();
        apply_recording_started(&mut g);
        assert_eq!(g.status, "recording");
        assert!(g.partial.is_empty() && g.finished.is_none() && g.err.is_none() && g.msg.is_empty());
    }

    // 松手进转写: 不能清 partial(结果还没到, 清空就是"回显瞬间消失")
    #[test]
    fn recording_ended_keeps_partial() {
        let mut g = snap();
        apply_recording_started(&mut g);
        g.partial = "正在说的话".into();
        apply_recording_ended(&mut g);
        assert_eq!(g.status, "transcribing");
        assert_eq!(g.partial, "正在说的话");
    }

    // 录音中收到上一条的交付/润色回调: 不许覆盖"录音中"与实时回显
    #[test]
    fn delivered_and_polishing_do_not_disturb_recording() {
        let mut g = snap();
        apply_recording_started(&mut g);
        g.partial = "新一段的话".into();
        apply_polishing(&mut g);
        apply_delivered(&mut g, serde_json::json!({"final": "上一条"}));
        assert_eq!(g.status, "recording");
        assert_eq!(g.partial, "新一段的话");
        assert!(g.finished.is_some());
    }

    // 录音中出错: 不写进快照(不打扰); 松手后(转写态)才提示
    #[test]
    fn error_is_dropped_while_recording() {
        let mut g = snap();
        apply_recording_started(&mut g);
        apply_error(&mut g, "连接失败");
        assert!(g.err.is_none());
        apply_recording_ended(&mut g);
        apply_error(&mut g, "连接失败");
        assert_eq!(g.err.as_deref(), Some("连接失败"));
    }

    // 最短显示守卫: 显示后 400ms 内的隐藏请求忽略
    #[test]
    fn short_show_guard() {
        assert!(should_skip_hide(1_000, 800)); // 显示 200ms
        assert!(!should_skip_hide(1_000, 600)); // 显示 400ms
        assert!(!should_skip_hide(1_000, 0)); // 从未显示过
    }
}
