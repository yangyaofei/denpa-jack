//! capture 域：从世界拿信号并决策（docs/ARCHITECTURE.md 四域之一）。
//!
//! 门面（外部只许经 capture:: 调这些）：
//!   - key::send_ctrl(sig) — 热键/托盘/试音的唯一信号入口
//!   - recording::ctrl_start / ctrl_stop / ctrl_abort — 会话状态机（经效果分派调）
//!   - audio::resolve_mic / list_mics — 麦克风解析
//!   - focus::focus_snapshot / focused_element_info / ax_trusted — 前台与焦点感知
//!   - screen::session_capture / active_target::detect — 屏幕上下文
//!   - permissions::ensure / state — 权限守门
//!
//! 内部（域外不可见，改组不影响调用方）：
//!   - key.rs — 热键注册+三规则(按下Start/松开Stop/长按AutoStopDue)+Esc 取消窗
//!   - recording.rs — 录音会话状态机(采样/分片/交接 SessionHandoff)
//!   - audio.rs — 麦克风解析+采集启动 · feedback.rs — 试音反馈音
//!   - mic_watch.rs — 设备插拔监视 · screen.rs — 截图+LLM 编码
//!   - active_target.rs — 活动目标探测(焦点窗口/指针窗口/显示器)
//!   - permissions.rs — 麦克风+辅助功能权限
//!   - focus.rs — AX 只读探测+lsappinfo 前台应用+FocusLock
//!
//! 禁止：调用他域内部符号；持有跨域可变状态（状态一律进 core）。
pub mod active_target;
pub mod audio;
pub mod feedback;
pub mod focus;
pub mod key;
pub mod mic_watch;
pub mod permissions;
pub mod recording;
pub mod screen;

// 批2 路径约定: 外部经 crate::capture::<子模块>::<符号> 层级路径调用(如
// capture::recording::ctrl_start)。批3 门面收口时改为 crate::capture::ctrl_start
// 直达意图符号, 机制符号收回域内。

pub use focus::{ax_trusted, focused_element_info, frontmost_bundle, FocusInfo, FocusLock};
