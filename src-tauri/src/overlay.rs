// 录音浮窗(对齐 Handy overlay.rs): 定位/显隐
// C41: 定位全链路 NS 坐标系(NSEvent.mouseLocation → NSScreen.frame → NSWindow.setFrameOrigin)
// 此前混用 tauri cursor_position(NS y-up) 与 monitor.position(CG y-down), 跨屏必判错屏。
use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
use tauri::{Emitter, Manager};

use crate::log;

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

    // 窗口尺寸 480x200; 屏内水平居中, 垂直按配置(NS y 向上: 底部=minY+60)
    let p = NsPoint::in_screen(
        f,
        (f.size.width - 480.0) / 2.0,
        match pos_mode {
            "top" => f.size.height - 200.0 - 60.0,
            "center" => (f.size.height - 200.0) / 2.0,
            _ => 60.0,
        },
    );
    // 守卫: in_screen 构造保证了屏内性, 此处复核窗口整体落屏(尺寸 480x200)
    if p.x < f.origin.x - 1.0
        || p.x + 480.0 > f.origin.x + f.size.width + 1.0
        || p.y < f.origin.y - 1.0
        || p.y + 200.0 > f.origin.y + f.size.height + 1.0
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
