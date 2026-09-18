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
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSData, NSString};
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

/// 托盘图标三态。**状态用烘焙好的彩色位图表达, 不依赖系统染色**(contentTintColor)。
/// 原因(用户实测): 桌面壁纸是深红 + 菜单栏半透明时, "template 位图 + 红色染色"几乎看不出,
/// 表现为"录音时图标消失/变黑"。彩色位图把颜色写进像素, 再加一圈白色描边保证任何底色上都可辨。
#[derive(Clone, Copy, PartialEq)]
pub enum TrayIcon {
    Idle,
    Recording,
    Transcribing,
}

/// 唯一的状态→图标映射(创建时与三态切换都走这里, 保证一致)
fn set_status_icon(item: &NSStatusItem, state: TrayIcon) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(btn) = item.button(mtm) else { return };
    let (png, template): (&[u8], bool) = match state {
        // 待命: 黑色 template(随浅/深色菜单栏自动反色)
        TrayIcon::Idle => (include_bytes!("../icons/menubar.png"), true),
        // 录音: 红 + 白描边(非 template, 颜色写死)
        TrayIcon::Recording => (include_bytes!("../icons/menubar-rec-red.png"), false),
        // 转写: 琥珀 + 白描边
        TrayIcon::Transcribing => (include_bytes!("../icons/menubar-trans-amber.png"), false),
    };
    let data = unsafe { NSData::dataWithBytes_length(png.as_ptr() as *const std::ffi::c_void, png.len()) };
    let Some(icon) = (unsafe { NSImage::initWithData(NSImage::alloc(), &data) }) else { return };
    unsafe {
        icon.setTemplate(template);
        icon.setSize(objc2_foundation::NSSize::new(24.5, 16.0));
        btn.setImage(Some(&icon));
        // 彩色位图自带颜色: 清掉染色, 避免 template 色遮挡(原来这里染红/黄, 见上)
        btn.setContentTintColor(None);
    }
}

pub struct MacTray {
    item: Retained<NSStatusItem>,
    /// 持有 TrayHandler 防止其被释放: 菜单项的 target 是弱引用, 释放后菜单动作失效
    #[allow(dead_code)]
    handler: Retained<TrayHandler>,
    /// 两个状态标志: 三态图标由二者共同决定(转写优先于录音, 都没有=待命)
    recording: std::cell::Cell<bool>,
    transcribing: std::cell::Cell<bool>,
}

impl MacTray {
    /// 按当前标志刷新图标(唯一入口)
    fn refresh_icon(&self) {
        let state = if self.recording.get() {
            TrayIcon::Recording
        } else if self.transcribing.get() {
            TrayIcon::Transcribing
        } else {
            TrayIcon::Idle
        };
        set_status_icon(&self.item, state);
    }

    /// 录音态
    pub fn set_recording(&self, on: bool) {
        self.recording.set(on);
        self.refresh_icon();
    }

    /// 转写态
    pub fn set_transcribing(&self, on: bool) {
        self.transcribing.set(on);
        self.refresh_icon();
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
        // C41: 菜单栏图标沿用 Swift 版自绘(声波→箭头→文本框); 三态映射统一走 set_status_icon
        set_status_icon(&item, if recording { TrayIcon::Recording } else { TrayIcon::Idle });
        if let Some(btn) = item.button(mtm) {
            unsafe {
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
        Ok(Self {
            item,
            handler,
            recording: std::cell::Cell::new(recording),
            transcribing: std::cell::Cell::new(false),
        })
    }
}
