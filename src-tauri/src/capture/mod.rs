//! capture 域：从世界拿信号并决策（docs/ARCHITECTURE.md 四域之一）。
//!
//! 门面（外部只许调这些）：
//!   - focus::focus_snapshot / focused_element_info / ax_trusted — 前台与焦点感知
//! 内部（域外不可见，改组不影响调用方）：
//!   - focus.rs — AX 只读探测 + lsappinfo 前台应用 + FocusLock
//!
//! 禁止：调用他域内部符号；持有跨域可变状态（状态一律进 core）。
//! 生长点（批 2 起陆续归位）：key(热键+三规则) · audio(录音+麦克风) ·
//! screen(目标探测+截图) · permissions(三项守门)。
pub mod focus;

pub use focus::{ax_trusted, focused_element_info, frontmost_bundle, FocusInfo, FocusLock};
