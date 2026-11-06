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
// Mutex 不再需要: 快照镜像层与 ui 侧通知槽都已迁 core(2026-11-05)

/// 前端读到的整份快照(core::snapshot 的直通形态; 无拼装无状态)。
/// 阶段判定(recording/busy 等)在前端, 从 cur/queue 的段状态自己判。
#[derive(Default, Clone, serde::Serialize)]
pub struct Snapshot {
    pub version: u64,
    pub recording: bool,
    /// 音量(RMS 0-1000 定点)
    pub level: u32,
    /// 当前段 = 用户最后说的那条(录音中 partial 在它的 text 上实时更新)
    pub cur: Option<crate::core::SegView>,
    /// 其余段(时间序: 旧→新; 含排队/转写中/失败)
    pub queue: Vec<crate::core::SegView>,
    /// 是否有未处理的失败段
    pub failed: bool,
    // ── 一次性通知(取走即清) ──
    pub msg: String,
    /// 交付载荷(带 warning 时前端延长提示)
    pub finished: Option<serde_json::Value>,
    pub err: Option<String>,
    /// 最后一段进入 LLM 润色(事实, 展示端据此显示"润色中")
    pub polishing: bool,
}

// 快照镜像层已删除(2026-11-05 用户指正): static HUD: Mutex<Snapshot> 是第二份
// 事实源("拷贝进快照+事件泵改快照"的转换层)——撕裂/漏刷(LAST_CORE_VER 补丁)皆源于
// 此。现在 poll() 直读 core(core::snapshot 一次锁取齐), 本模块无跨调用数据状态。

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




/// 一次性通知槽(pump → poll 的交接箱)——已迁 core(2026-11-05):
/// 写入=core::notice_put(必 bump version), 取走=snapshot 内 mem::take。
/// 曾住 ui static 且写入不 bump version——前端按 version 去重把槽内容丢弃,
/// 没听清后浮窗停在"转写中"永不收窗(真机回归, 本批修复)。

/// 事件→一次性槽的纯映射(展示保护规则的载体, 便于单测):
/// recording=true 时收尾类事件不覆盖阶段(issue #2); 返回值=是否需要弹窗。
/// 槽本体住 core(pending_notice)——写入经 core::notice_put(必 bump version)。
fn apply_event(o: &mut crate::core::PendingNotice, recording: bool, ev: &crate::core::AppEvent) -> bool {
    match ev {
        crate::core::AppEvent::Notify(m) => {
            o.msg = m.clone();
            true
        }
        crate::core::AppEvent::NoSpeech(r) => {
            // 没听清 = 这一段不会再有交付: 提示 + 阶段收掉(否则前端按"转写中"判忙永不收窗, issue #9)
            o.msg = r.clone();
            o.polishing = false;
            true
        }
        crate::core::AppEvent::Delivered(p) => {
            if !recording {
                o.polishing = false;
            }
            o.finished = Some(p.clone());
            false
        }
        crate::core::AppEvent::Polishing => {
            if !recording {
                o.polishing = true;
            }
            false
        }
        crate::core::AppEvent::Error(r) => {
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

/// 事件泵(批4 起, 2026-11-05 职责纯化): 事件队列的唯一消费者。
/// drain → apply_event 纯函数(展示保护规则: 录音中不被上一段收尾打扰)
/// → core::notice_put 写槽(取走即清; 写入必 bump version——
/// 前端以 version 去重, 槽写入不 bump 提示会被丢弃)+需弹窗事件 show。
/// 装配期由 lib.rs 启动。
pub fn start_event_pump(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let events = crate::core::drain_events();
        if !events.is_empty() {
            let recording = crate::core::session_recording();
            let mut show = false;
            crate::core::notice_put(|o| {
                for ev in &events {
                    if apply_event(o, recording, ev) {
                        show = true;
                    }
                }
            });
            if show {
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

/// 前端唯一读接口: **直通 core::snapshot**(2026-11-06 段一生单列表重构后,
/// 这里不再有任何拼装——cur/queue 的组装只在 core::views_of 唯一点,
/// 状态阶段等展示判定全在前端, 从段列表自己判)。
pub fn poll() -> Snapshot {
    let a = crate::core::snapshot();
    let o = a.oneshot;
    Snapshot {
        version: a.version,
        recording: a.recording,
        level: a.level,
        cur: a.cur,
        queue: a.queue,
        failed: a.failed,
        msg: o.msg,
        finished: o.finished,
        err: o.err,
        polishing: o.polishing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> crate::core::PendingNotice {
        crate::core::PendingNotice::default()
    }

    // 录音中收到上一条的交付/润色回调: 载荷照给, 但不许置"润色中"阶段(issue #2)
    #[test]
    fn delivered_and_polishing_do_not_disturb_recording() {
        let mut o = slot();
        assert!(!apply_event(&mut o, true, &crate::core::AppEvent::Polishing));
        apply_event(&mut o, true, &crate::core::AppEvent::Delivered(serde_json::json!({"final": "上一条"})));
        assert!(!o.polishing, "录音中润色标记被压制");
        assert!(o.finished.is_some(), "交付载荷照给");
    }

    // 非录音时润色置位, 交付后收掉
    #[test]
    fn delivered_and_polishing_apply_when_not_recording() {
        let mut o = slot();
        apply_event(&mut o, false, &crate::core::AppEvent::Polishing);
        assert!(o.polishing);
        apply_event(&mut o, false, &crate::core::AppEvent::Delivered(serde_json::json!({"final": "x"})));
        assert!(!o.polishing, "交付收掉润色阶段");
    }

    // 录音中出错: 不写槽(不打扰); 非录音时错误写入并收掉阶段
    #[test]
    fn error_is_dropped_while_recording() {
        let mut o = slot();
        apply_event(&mut o, true, &crate::core::AppEvent::Error("连接失败".into()));
        assert!(o.err.is_none());
        apply_event(&mut o, false, &crate::core::AppEvent::Error("连接失败".into()));
        assert_eq!(o.err.as_deref(), Some("连接失败"));
    }

    // 空转写必须收掉润色阶段(否则前端按忙判定 → 永不收起, issue #9)+ 需弹窗
    #[test]
    fn no_speech_ends_phase_and_shows() {
        let mut o = slot();
        apply_event(&mut o, false, &crate::core::AppEvent::Polishing);
        assert!(apply_event(&mut o, false, &crate::core::AppEvent::NoSpeech("没听清".into())), "NoSpeech 需弹窗");
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

    // 阶段判定已迁前端(从段状态判), Rust 侧不再有 status 概念——
    // derive_status 与其用例随镜像层/展示态一起删除(2026-11-06)
}
