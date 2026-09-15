// clang-format off
#![allow(unused_unsafe)]
// 菜单栏托盘: objc2 手写 NSStatusItem(tauri tray API 在本环境不显示, 3 轮失败后弃用)
// 菜单事件经 mpsc 发回 lib.rs 处理
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::define_class;
use objc2::{sel, AnyThread, DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSData, ns_string, NSString};
use std::sync::mpsc::Sender;

type TrayTx = Sender<String>;

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = TrayTx]
    struct TrayHandler;

    impl TrayHandler {
        #[unsafe(method(trayAction:))]
        fn tray_action(&self, sender: &NSMenuItem) {
            let _ = DefinedClass::ivars(self).send(sender.tag().to_string());
        }
    }
);

pub struct MacTray {
    item: Retained<NSStatusItem>,
    handler: Retained<TrayHandler>,
}

impl MacTray {
    /// 录音态: 图标染红(与 Swift 版一致)
    pub fn set_recording(&self, on: bool) {
        if let Some(mtm) = MainThreadMarker::new() {
            if let Some(btn) = self.item.button(mtm) {
                let png: &[u8] = if on {
                    include_bytes!("../icons/menubar-rec.png")
                } else {
                    include_bytes!("../icons/menubar.png")
                };
                let data = unsafe { NSData::dataWithBytes_length(png.as_ptr() as *const std::ffi::c_void, png.len()) };
                if let Some(icon) = unsafe { NSImage::initWithData(NSImage::alloc(), &data) } {
                    unsafe {
                        icon.setTemplate(true);
                        let _ = icon.setSize(objc2_foundation::NSSize::new(24.5, 16.0));
                        btn.setImage(Some(&icon));
                        let tint = if on { Some(NSColor::systemRedColor()) } else { None };
                        btn.setContentTintColor(tint.as_deref());
                    }
                }
            }
        }
    }

    pub fn set_transcribing(&self, on: bool) {
        if let Some(mtm) = MainThreadMarker::new() {
            if let Some(btn) = self.item.button(mtm) {
                let tint = if on {
                    Some(&*NSColor::systemYellowColor())
                } else {
                    None
                };
                btn.setContentTintColor(tint);
            }
        }
    }
}
// SAFETY: NSStatusItem/TrayHandler 均为主线程对象; 所有创建/销毁都经 run_on_main_thread 保证在主线程
unsafe impl Send for MacTray {}

impl Drop for MacTray {
    fn drop(&mut self) {
        unsafe {
            NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
        }
    }
}

fn add_item(
    menu: &NSMenu,
    handler: &TrayHandler,
    title: &str,
    tag: isize,
    enabled: bool,
    checked: bool,
) {
    let mtm = MainThreadMarker::new().expect("add_item 需在主线程");
    let item = unsafe { NSMenuItem::new(mtm) };
    unsafe {
        item.setTitle(&*NSString::from_str(title));
        item.setEnabled(enabled);
        item.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
        if enabled {
            item.setAction(Some(sel!(trayAction:)));
            item.setTarget(Some(handler));
        }
        item.setTag(tag);
        menu.addItem(&item);
    }
}

impl MacTray {
    /// 构建托盘 + 菜单; 事件发到 tx
    pub fn new(
        mtm: MainThreadMarker,
        tx: Sender<String>,
        recording: bool,
        hotkey: &str,
        llm_on: bool,
        clip_on: bool,
    ) -> Result<Self, String> {
        let alloc = TrayHandler::alloc();
        let handler: Retained<TrayHandler> = unsafe { msg_send![super(alloc.set_ivars(tx)), init] };
        let status_bar = unsafe { NSStatusBar::systemStatusBar() };
        let item = unsafe { status_bar.statusItemWithLength(-1f64) }; // NSVariableStatusItemLength
        // C41: 菜单栏图标沿用 Swift 版自绘(声波→箭头→文本框), @2x 位图 + template 适配深浅色
        let png: &[u8] = if recording {
            include_bytes!("../icons/menubar-rec.png")
        } else {
            include_bytes!("../icons/menubar.png")
        };
        let data = unsafe { NSData::dataWithBytes_length(png.as_ptr() as *const std::ffi::c_void, png.len()) };
        let icon = unsafe { NSImage::initWithData(NSImage::alloc(), &data) }.ok_or("menubar.png 解码失败")?;
        unsafe {
            icon.setTemplate(true);
            icon.setSize(objc2_foundation::NSSize::new(24.5, 16.0));
            if let Some(btn) = item.button(mtm) {
                btn.setImage(Some(&icon));
                btn.setToolTip(Some(&*NSString::from_str("Denpa Jack")));
            }
        }
        let menu = unsafe { NSMenu::new(mtm) };
        let status_text = if recording {
            format!("状态: 录音中…")
        } else {
            format!("状态: 待命 (按住 {hotkey} 说话)")
        };
        add_item(&menu, &handler, &status_text, 0, false, false);
        unsafe {
            let sep = NSMenuItem::separatorItem(mtm);
            menu.addItem(&sep);
        }
        add_item(&menu, &handler, "录音 (松开转写)", 6, true, false);
        add_item(&menu, &handler, "LLM 纠错(慢速高保真)", 1, true, llm_on);
        add_item(&menu, &handler, "仅复制不粘贴", 2, true, clip_on);
        unsafe {
            let sep = NSMenuItem::separatorItem(mtm);
            menu.addItem(&sep);
        }
        add_item(&menu, &handler, "打开数据目录", 3, true, false);
        add_item(&menu, &handler, "打开词典配置", 7, true, false);
        add_item(&menu, &handler, "设置…", 4, true, false);
        unsafe {
            let sep = NSMenuItem::separatorItem(mtm);
            menu.addItem(&sep);
        }
        add_item(&menu, &handler, "退出", 5, true, false);
        unsafe {
            item.setMenu(Some(&menu));
        }
        Ok(Self { item, handler })
    }
}
