//! 活动目标探测：主窗口 + 它所在的显示器。
//!
//! 只做一件事，两个消费者：
//! - 截图（`screen_context`）：按配置决定截窗口还是截显示器（窗口缺失时回退显示器）
//! - 浮窗（`overlay`）：直接用 `display` 摆放
//!
//! 判定顺序（用户定则）：
//! 1. **先看光标**：当前键盘焦点（输入光标所在）窗口 → 主窗口；它的屏 = 显示器
//! 2. **没有光标**：鼠标指针下的普通窗口 → 主窗口；它的屏 = 显示器
//! 3. **鼠标下也没有窗口**：没有主窗口；显示器 = 指针所在屏
//!
//! 因此：`display` 一定有值；`window` 可能为 None（桌面/本应用/过小窗口/无焦点）。
use crate::overlay::{self, DisplayTarget};
use crate::screen_context::WindowRef;

#[derive(Clone)]
pub struct ActiveTarget {
    /// 活动显示器（一定有）
    pub display: DisplayTarget,
    /// 主窗口（可能没有）
    pub window: Option<WindowRef>,
}

/// 探测当前活动目标（每次用时探测一次，截图与浮窗共用同一个结果）
pub fn detect_active_target() -> ActiveTarget {
    // 1. 光标：键盘焦点所在窗口（AX 焦点元素 → 窗口框 → CG 窗口列表匹配）
    if let Some(w) = crate::screen_context::focused_window() {
        let display = display_of_window(&w);
        return ActiveTarget {
            display,
            window: Some(w),
        };
    }
    // 2. 鼠标：指针下的窗口
    let cursor = overlay::cursor_point();
    let display = cursor
        .and_then(overlay::display_target_at)
        .unwrap_or_else(overlay::main_display_target);
    let window = cursor.and_then(crate::screen_context::window_at);
    ActiveTarget { display, window }
}

/// 窗口中心点所在的显示器（窗口在屏外/取不到时回退主显示器）
fn display_of_window(w: &WindowRef) -> DisplayTarget {
    let cx = w.frame.0 + w.frame.2 / 2.0;
    let cy = w.frame.1 + w.frame.3 / 2.0;
    overlay::display_target_at((cx, cy)).unwrap_or_else(overlay::main_display_target)
}
