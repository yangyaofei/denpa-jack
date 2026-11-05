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
    pub seg_current: Option<crate::core::SegView>,
    /// 右栏: 队列里其余段(空 = 右栏不显示)
    pub seg_queue: Vec<crate::core::SegView>,
    /// 是否有未处理的失败段(存活规则: 只剩失败时 2.5s 后收起)
    pub seg_failed: bool,
}

// 快照镜像层已删除(2026-11-05 用户指正): static HUD: Mutex<Snapshot> 是第二份
// 事实源("拷贝进快照+事件泵改快照"的转换层)——撕裂/漏刷(LAST_CORE_VER 补丁)皆源于
// 此。现在 poll() 直读 core(core::hud_all 一次锁取齐), 本模块无跨调用数据状态。

/// 最短显示时长(毫秒): 显示后这段时间内的隐藏请求一律忽略
pub const MIN_SHOW_MS: u64 = 400;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 浮窗最近一次显示的时刻(毫秒, UNIX_EPOCH); 0 = 从未显示
static LAST_SHOW_MS: AtomicU64 = AtomicU64::new(0);

/// 隐藏请求是否应当被忽略(显示不足 `MIN_SHOW_MS`)
fn should_skip_hide(now: u64, shown: u64) -> bool {
    shown > 0 && now.saturating_sub(shown) < MIN_SHOW_MS
}

// ───────────────────────── 对外唯一接口 ─────────────────────────

/// 一条新录音开始了(按下热键)。定位并显示浮窗, 清掉上一条的内容。
/// 事实(在录/电平/partial 归零)迁移进 core; 这里只编排展示与窗口。
pub fn recording_started(app: &tauri::AppHandle, display: &crate::ui::overlay::DisplayTarget) {
    crate::core::session_started();
    // 与截图共用同一个目标显示器（capture_target::resolve 的快照）
    crate::ui::overlay::position_hud(app, Some(display));
    LAST_SHOW_MS.store(now_ms(), Ordering::SeqCst);
    crate::ui::overlay::show_window(app);
}

/// 这一条录完了(松手)。只表达事实, 显示成什么样由本模块决定。
pub fn recording_ended(app: &tauri::AppHandle) {
    let _ = app;
    crate::core::session_ended();
}




/// 一次性通知槽(pump → poll 的交接箱): msg/err/finished 是"待前端取走的事件",
/// **不是状态镜像**——持久数据(partial/level/段列表)一律 poll 直读 core。
/// 展示保护规则(录音中不被上一段收尾打扰)在 pump 写槽时判 recording(时点准确)。
#[derive(Default)]
struct Oneshot {
    msg: String,
    finished: Option<serde_json::Value>,
    err: Option<String>,
    polishing: bool,
}

static ONESHOT: Mutex<Oneshot> = Mutex::new(Oneshot {
    msg: String::new(),
    finished: None,
    err: None,
    polishing: false,
});

/// 事件→一次性槽的纯映射(展示保护规则的载体, 便于单测):
/// recording=true 时收尾类事件不覆盖阶段(issue #2); 返回值=是否需要弹窗。
fn apply_event(o: &mut Oneshot, recording: bool, ev: &crate::core::HudEvent) -> bool {
    match ev {
        crate::core::HudEvent::Notify(m) => {
            o.msg = m.clone();
            true
        }
        crate::core::HudEvent::NoSpeech(r) => {
            // 没听清 = 这一段不会再有交付: 提示 + 阶段收掉(否则前端按"转写中"判忙永不收窗, issue #9)
            o.msg = r.clone();
            o.polishing = false;
            true
        }
        crate::core::HudEvent::Delivered(p) => {
            if !recording {
                o.polishing = false;
            }
            o.finished = Some(p.clone());
            false
        }
        crate::core::HudEvent::Polishing => {
            if !recording {
                o.polishing = true;
            }
            false
        }
        crate::core::HudEvent::Error(r) => {
            crate::log::elog(&format!("[hud] error: {r}"));
            if !recording {
                // 错误意味着这一段已结束, 阶段一起收掉(issue #9 同类)
                o.polishing = false;
                o.err = Some(r.clone());
            }
            false
        }
    }
}

/// status 优先级纯函数: 录音 > 润色(通知槽) > 转写中(投影) > 空
fn derive_status(recording: bool, polishing: bool, cur_transcribing: bool, has_notice: bool) -> String {
    if recording {
        Phase::Recording.wire_str()
    } else if polishing {
        Phase::Polishing.wire_str()
    } else if cur_transcribing && !has_notice {
        Phase::Transcribing.wire_str()
    } else {
        String::new()
    }
}

/// 事件泵(批4 起, 2026-11-05 职责纯化): 事件队列的唯一消费者。
/// drain → apply_event 纯函数(含展示保护) → 写一次性槽(poll 合并取走) + 需弹窗的事件 show。
/// 装配期由 lib.rs 启动。
pub fn start_event_pump(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        for ev in crate::core::hud_drain() {
            let recording = crate::core::session_recording();
            let mut o = ONESHOT.lock().unwrap();
            if apply_event(&mut o, recording, &ev) {
                drop(o);
                crate::ui::overlay::show_window(&app);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
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
    // 快照镜像层已删: 窗口隐藏后前端停 poll, 下次显示时 poll 直读 core 自然是新数据
    crate::ui::overlay::hide_window(app);
}

/// 前端上报浮窗宽度(单栏 480 / 两栏 664)。**只改宽度, 高度固定**:
/// 高度固定后窗口位置在"按下定位一次"之后就再也不动(用户报过"位置跟原来不完全一样")。
pub fn resize_width(app: &tauri::AppHandle, width: f64) {
    crate::ui::overlay::resize_width(app, width.clamp(480.0, 720.0));
}

/// 前端唯一读接口: 直读 core(2026-11-05 按用户指正删除快照镜像层——
/// 旧实现"拷进 static HUD + 事件泵改快照"是第二份事实源, 撕裂/漏刷皆源于此)。
/// 一次 core 锁取齐持久投影+事件, 本函数只做纯组装, 无任何跨调用状态:
///   左栏 = 录音中显示实时文字; 没在录音时 = 正在转写的那段
///   右栏 = 队列里其余段(录音时, 正在转写的那段退到右栏)
/// 展示保护规则(录音中不被上一段的收尾事件打扰)在此判 recording。
pub fn poll() -> Snapshot {
    let a = crate::core::hud_all();
    // 一次性通知(pump 写入的交接箱, 取走即清; 展示保护规则已在 pump 判过)
    let o = std::mem::take(&mut *ONESHOT.lock().unwrap());
    // HUD 不显示重转段(用户定则: 重转是后台任务, 录音浮窗只管语音链路的段;
    // 重转状态看主窗历史页)。views() 保持全量, 各投影端自决过滤
    let queue: Vec<_> = a.queue.into_iter().filter(|s| s.source != "rerun").collect();
    let mut s = Snapshot {
        version: a.version,
        status: String::new(),
        partial: a.partial.clone(),
        level: a.level,
        msg: o.msg,
        finished: o.finished,
        err: o.err,
        seg_current: if a.recording {
            Some(crate::core::SegView { state: "recording".into(), source: "live".into(), text: a.partial })
        } else {
            a.cur.clone()
        },
        seg_queue: if a.recording {
            // 录音时正在转写的那段退到右栏队首
            let mut q = Vec::with_capacity(queue.len() + 1);
            if let Some(c) = a.cur.clone() {
                q.push(c);
            }
            q.extend(queue);
            q
        } else {
            queue
        },
        seg_failed: a.failed,
    };
    // status 优先级: 纯函数推导
    let cur_transcribing = a.cur.as_ref().map(|c| c.state == "transcribing").unwrap_or(false);
    let has_notice = !s.msg.is_empty() || s.finished.is_some() || s.err.is_some();
    s.status = derive_status(a.recording, o.polishing, cur_transcribing, has_notice);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> Oneshot {
        Oneshot::default()
    }

    // 录音中收到上一条的交付/润色回调: 载荷照给, 但不许置"润色中"阶段(issue #2)
    #[test]
    fn delivered_and_polishing_do_not_disturb_recording() {
        let mut o = slot();
        assert!(!apply_event(&mut o, true, &crate::core::HudEvent::Polishing));
        apply_event(&mut o, true, &crate::core::HudEvent::Delivered(serde_json::json!({"final": "上一条"})));
        assert!(!o.polishing, "录音中润色标记被压制");
        assert!(o.finished.is_some(), "交付载荷照给");
    }

    // 非录音时润色置位, 交付后收掉
    #[test]
    fn delivered_and_polishing_apply_when_not_recording() {
        let mut o = slot();
        apply_event(&mut o, false, &crate::core::HudEvent::Polishing);
        assert!(o.polishing);
        apply_event(&mut o, false, &crate::core::HudEvent::Delivered(serde_json::json!({"final": "x"})));
        assert!(!o.polishing, "交付收掉润色阶段");
    }

    // 录音中出错: 不写槽(不打扰); 非录音时错误写入并收掉阶段
    #[test]
    fn error_is_dropped_while_recording() {
        let mut o = slot();
        apply_event(&mut o, true, &crate::core::HudEvent::Error("连接失败".into()));
        assert!(o.err.is_none());
        apply_event(&mut o, false, &crate::core::HudEvent::Error("连接失败".into()));
        assert_eq!(o.err.as_deref(), Some("连接失败"));
    }

    // 空转写必须收掉润色阶段(否则前端按忙判定 → 永不收起, issue #9)+ 需弹窗
    #[test]
    fn no_speech_ends_phase_and_shows() {
        let mut o = slot();
        apply_event(&mut o, false, &crate::core::HudEvent::Polishing);
        assert!(apply_event(&mut o, false, &crate::core::HudEvent::NoSpeech("没听清".into())), "NoSpeech 需弹窗");
        assert!(!o.polishing);
        assert_eq!(o.msg, "没听清");
    }

    // 最短显示守卫: 显示后 400ms 内的隐藏请求忽略
    #[test]
    fn short_show_guard() {
        assert!(should_skip_hide(1_000, 800)); // 显示 200ms
        assert!(!should_skip_hide(1_000, 600)); // 显示 400ms
        assert!(!should_skip_hide(1_000, 0)); // 从未显示过
    }

    // status 优先级: 录音 > 润色 > 转写中 > 空
    #[test]
    fn status_priority() {
        assert_eq!(derive_status(true, true, true, false), "recording");
        assert_eq!(derive_status(false, true, true, false), "busy");
        assert_eq!(derive_status(false, false, true, false), "transcribing");
        assert_eq!(derive_status(false, false, true, true), "", "有一次性通知时不显示转写中");
        assert_eq!(derive_status(false, false, false, false), "");
    }
}
