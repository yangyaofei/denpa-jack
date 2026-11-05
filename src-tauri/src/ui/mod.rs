//! ui 域：把状态呈现给人（docs/ARCHITECTURE.md 四域之一）。
//!
//! 门面（外部只许经 ui:: 调这些）：
//!   - hud::show_partial / show_final / show_delivered / polishing / error / no_speech / dismiss
//!   - overlay::show / hide / reposition（+DisplayTarget 类型）
//!   - tray::init / status · tray_events::init（装配期）
//!   - update::check / install
//!
//! 内部（域外不可见，改组不影响调用方）：
//!   - hud.rs — 浮窗状态机+策略（何时显/隐/显示什么）
//!   - overlay.rs — 窗口机制（NS 全链路坐标契约, 见 build.rs 坐标闸）
//!   - tray.rs / tray_events.rs — 菜单栏与事件泵
//!   - update.rs — 内置更新检查/安装
//!
//! 禁止：直接改业务状态（一律读 core 投影）；被 capture/pipeline/model 调用
//! （展示是流程末端——反向通知走 core，见调用律）。
pub mod hud;
pub mod overlay;
pub mod tray;
pub mod tray_events;
pub mod update;

// ───────── 门面(点名导出, 批4 起 capture/pipeline 经这些符号调显隐)─────────
// 提示类(notify/error/no_speech/delivered/polishing)不走这里——外部写
// core::emit, hud 自己的事件泵消费(见 hud::start_event_pump)。
pub use hud::{recording_started, recording_ended};
pub use hud::hide as dismiss;
pub use tray_events::refresh_status;
pub use overlay::{cursor_point, display_scale, display_target_at, main_display_target, DisplayTarget};
