// 录音浮窗(对齐 Handy overlay.rs): 定位/显隐
// C41: 定位全链路 NS 坐标系(NSEvent.mouseLocation → NSScreen.frame → NSWindow.setFrameOrigin)
// 此前混用 tauri cursor_position(NS y-up) 与 monitor.position(CG y-down), 跨屏必判错屏。
use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
use tauri::{Emitter, Manager};

use crate::log;

/// 全局显示空间(y 向下, 原点在主显示器左上)的点 —— 与 NSScreen(y 向上)是两套坐标系
#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CgPoint;
    fn CGGetDisplaysWithPoint(
        point: CgPoint,
        max_displays: u32,
        displays: *mut u32,
        count: *mut u32,
    ) -> i32;
    fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CgRect;
    // 与 deliver.rs 保持同一签名（否则 clashing_extern_declarations）
    fn CFRelease(cf: *mut std::ffi::c_void);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

/// 光标所在显示器（截图目标屏）
///
/// `index` 是 `screencapture -D` 使用的编号（1 = 主显示器）；`origin`/`size` 是全局显示空间
/// （点为单位，y 向下，原点在主显示器左上）的位置与尺寸，用于日志核对与后续裁剪。
#[derive(Debug, Clone, Copy)]
pub struct DisplayTarget {
    pub index: usize,
    pub origin: (f64, f64),
    pub size: (f64, f64),
}

/// 取光标所在显示器。
///
/// 用途：截图默认抓主显示器，多显示器下与用户实际看的屏不一致（用户实测反馈）。
/// 与 hud 定位同源——都以"光标所在屏"为目标屏。
/// 实现只用 CoreGraphics（全局显示空间，线程安全）：截图跑在后台线程，不能依赖主线程的 NSScreen。
/// 编号取自 CGGetActiveDisplayList 的下标 +1（与 screencapture 的编号规则一致，主显示器排第一；
/// 已实测：本机 1 = 内建屏 3456x2160，2 = 外接屏 5120x2880）。
pub fn display_target_at_cursor() -> Option<DisplayTarget> {
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev);
        let mut hit = [0u32; 8];
        let mut hit_n = 0u32;
        if CGGetDisplaysWithPoint(p, hit.len() as u32, hit.as_mut_ptr(), &mut hit_n) != 0 || hit_n == 0
        {
            return None;
        }
        let mut list = [0u32; 16];
        let mut list_n = 0u32;
        if CGGetActiveDisplayList(list.len() as u32, list.as_mut_ptr(), &mut list_n) != 0 {
            return None;
        }
        let idx = (0..list_n as usize).find(|i| list[*i] == hit[0])?;
        let b = CGDisplayBounds(list[idx]);
        Some(DisplayTarget {
            index: idx + 1,
            origin: (b.origin.x, b.origin.y),
            size: (b.size.width, b.size.height),
        })
    }
}

pub fn position_hud_at_cursor(app: &tauri::AppHandle) {
    let Some(w) = app.get_webview_window("hud") else {
        log::log(app, "hud 窗口不存在!");
        return;
    };
    let app2 = app.clone();
    let pos_mode = crate::settings::get_config(app.clone())
        .map(|c| c.overlay_position)
        .unwrap_or_else(|_| "bottom".to_string());
    let r = w.run_on_main_thread(move || unsafe {
        position_on_main(&app2, &pos_mode);
    });
    if let Err(e) = r {
        log::log(app, &format!("hud 定位调度失败: {e}"));
    }
}

/// NS 坐标系点(左下原点, y 向上)——模块私有新类型:
/// 模块外构造不出也消费不了, 坐标混用在类型上不可表达(而非靠文档/检测)
#[derive(Clone, Copy)]
struct NsPoint {
    x: f64,
    y: f64,
}

impl NsPoint {
    /// 唯一构造源 1: 光标
    unsafe fn from_cursor() -> Self {
        let l = NSEvent::mouseLocation();
        Self { x: l.x, y: l.y }
    }
    /// 唯一构造源 2: 屏内偏移(屏 frame 也只在模块内读取)
    fn in_screen(f: objc2_foundation::NSRect, dx: f64, dy: f64) -> Self {
        Self { x: f.origin.x + dx, y: f.origin.y + dy }
    }
    fn within(&self, f: objc2_foundation::NSRect) -> bool {
        self.x >= f.origin.x && self.x < f.origin.x + f.size.width
            && self.y >= f.origin.y && self.y < f.origin.y + f.size.height
    }
}

/// 全仓库唯一的窗口坐标写入点: 一切 setFrameOrigin 只准从这里走
unsafe fn place(nsw: &NSWindow, p: NsPoint) {
    nsw.setFrameOrigin(objc2_foundation::NSPoint::new(p.x, p.y));
}

/// 必须主线程: NSWindow 操作约束(与 tray/paste_tx 同一模型)
unsafe fn position_on_main(app: &tauri::AppHandle, pos_mode: &str) {
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(w) = app.get_webview_window("hud") else {
        return;
    };
    let ns_ptr = match w.ns_window() {
        Ok(p) => p,
        Err(e) => {
            log::log(app, &format!("hud ns_window 获取失败: {e}"));
            return;
        }
    };
    let nsw: &NSWindow = unsafe { &*(ns_ptr.cast::<NSWindow>()) };

    // 光标位置 + 光标所在屏(全 NS 坐标, y 向上)
    let loc = NsPoint::from_cursor();
    let screens = NSScreen::screens(mtm);
    let target = screens
        .iter()
        .map(|s| s.frame())
        .find(|f| loc.within(*f))
        .or_else(|| NSScreen::mainScreen(mtm).map(|s| s.frame()));
    let Some(f) = target else {
        log::log(app, "hud 定位失败: 无可用显示器");
        return;
    };

    // 窗口尺寸取实际值: 单栏 480 / 出现队列时 664(分段队列定则), 高度随内容(92..400)。
    // 不能再写死 480x200 —— hud_resize 之后要按新尺寸保持"水平居中/垂直按配置"
    let size = nsw.frame().size;
    let (ww, wh) = (size.width, size.height);
    // 屏内水平居中, 垂直按配置(NS y 向上: 底部=minY+60)
    let p = NsPoint::in_screen(
        f,
        (f.size.width - ww) / 2.0,
        match pos_mode {
            "top" => f.size.height - wh - 60.0,
            "center" => (f.size.height - wh) / 2.0,
            _ => 60.0,
        },
    );
    // 守卫: in_screen 构造保证了屏内性, 此处复核窗口整体落屏
    if p.x < f.origin.x - 1.0
        || p.x + ww > f.origin.x + f.size.width + 1.0
        || p.y < f.origin.y - 1.0
        || p.y + wh > f.origin.y + f.size.height + 1.0
    {
        log::elog(&format!(
            "[hud] 定位越界被拒: 目标=({:.0},{:.0}) 屏=({:.0},{:.0})x({:.0}x{:.0})",
            p.x, p.y, f.origin.x, f.origin.y, f.size.width, f.size.height
        ));
        return;
    }
    log::elog(&format!(
        "[hud] 定位: 光标=({:.0},{:.0}) 屏=({:.0},{:.0})x({:.0}x{:.0}) -> ({:.0},{:.0})",
        loc.x, loc.y, f.origin.x, f.origin.y, f.size.width, f.size.height, p.x, p.y
    ));
    place(nsw, p);
}

pub fn show_hud(app: &tauri::AppHandle) {
    position_hud_at_cursor(app);
    if let Some(w) = app.get_webview_window("hud") {
        // B42: 根治抢焦点——hud 窗口永不成为 key window(用户实测"抢焦点"后输入光标丢失)
        // Handy 语义对照(crates/handy/src/macos/overlay.rs:7 FloatingPanel: NSPanel):
        //   nonactivatingPanel ≙ set_focusable(false) ✓
        //   canJoinAllSpaces+fullScreenAuxiliary ≙ visible_on_all_workspaces(全屏 App 上浮窗可见)
        let _ = w.set_focusable(false);
        let _ = w.set_visible_on_all_workspaces(true);
        if let Err(e) = w.show() {
            log::log(app, &format!("hud show 失败: {e}"));
        } else {
            // B42: 浮窗绝不抢焦点——抢了就把 app 退到后台(hud 是 always_on_top 仍可见),
            // 否则用户随后 CmdV 会贴进浮窗
            if w.is_focused().unwrap_or(false) {
                if let Some(mtm) = objc2::MainThreadMarker::new() {
                    objc2_app_kit::NSApplication::sharedApplication(mtm).deactivate();
                    log::log(app, "hud 抢了焦点, 已交还");
                }
            }
            let vis = w.is_visible().unwrap_or(false);
            let focused = w.is_focused().unwrap_or(false);
            log::log(app, &format!("hud show ok visible={vis} focused={focused}"));
        }
        let _ = Emitter::emit_to(app, "hud", "hud-state", "recording");
        crate::hud_set(|h| { h.status = "recording".into(); h.partial.clear(); h.finished = None; h.err = None; });
    }
}

pub fn show_hud_msg(app: &tauri::AppHandle, msg: &str) {
    if let Some(w) = app.get_webview_window("hud") {
        let _ = Emitter::emit_to(app, "hud", "hud-msg", msg);
        let _ = w.show();
    }
}

pub fn hide_hud(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("hud") {
        let _ = w.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证：返回的显示器确实包含光标点（任何显示器数量下都成立）。
    /// 无光标/无显示器时返回 None → 跳过（CI 机器也适用）。
    #[test]
    fn display_target_matches_cursor_location() {
        unsafe {
            let Some(t) = display_target_at_cursor() else {
                return;
            };
            let ev = CGEventCreate(std::ptr::null());
            assert!(!ev.is_null());
            let p = CGEventGetLocation(ev);
            CFRelease(ev);
            println!(
                "编号 {} 的显示器: origin=({:.0},{:.0}) size={:.0}x{:.0} | 光标=({:.0},{:.0})",
                t.index, t.origin.0, t.origin.1, t.size.0, t.size.1, p.x, p.y
            );
            let inside = p.x >= t.origin.0
                && p.x < t.origin.0 + t.size.0
                && p.y >= t.origin.1
                && p.y < t.origin.1 + t.size.1;
            assert!(
                inside,
                "光标 ({:.0},{:.0}) 不在返回的显示器 {} 范围内",
                p.x, p.y, t.index
            );
        }
    }
}
