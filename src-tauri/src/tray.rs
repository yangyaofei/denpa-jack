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

/// 托盘图标三态。**状态用细线 template 位图 + 系统染色表达**（回到队列化之前的效果）。
/// 历史：C41 首次实现就是这样（`menubar.png` 细线字形，录音/转写只改染色）；
/// 之后我把三态做成"烘焙彩色位图 + 加粗字形"，那是另起一套设计、叠加在原来的逻辑上，
/// 用户明确否掉并给出规则：修 bug 回到原逻辑里改，不要在旧逻辑上再摞一层。
#[derive(Clone, Copy, PartialEq)]
pub enum TrayIcon {
    Idle,
    Recording,
    Transcribing,
}

/// 唯一的状态→图标映射（创建时与三态切换都走这里）。
///
/// 与原实现的三处差异（都是原来那套逻辑里的缺陷，不是新设计）：
/// 1. 原来 `set_recording` 设红色染色、`set_transcribing(false)` 把染色清成 `None`——
///    两组动作各自改同一个染色通道，转写结束会把录音态的染色一起清掉（残留状态）。
///    现在每次状态变化都按"当前状态"重算位图与染色，不留残留。
/// 2. 原来两处各自 `setTemplate`/`setSize`/`setImage`，创建处第三处再写一遍；
///    现在只有这一个函数做这件事。
/// 3. `transcribing` 仍不改位图（与原实现一致：只把染色换成黄），位图保持待命细线字形。
fn set_status_icon(item: &NSStatusItem, state: TrayIcon) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(btn) = item.button(mtm) else { return };
    // (位图, 染色)
    let (png, tint): (&[u8], Option<Retained<NSColor>>) = match state {
        TrayIcon::Idle => (include_bytes!("../icons/menubar.png"), None),
        TrayIcon::Recording => (
            include_bytes!("../icons/menubar-rec.png"),
            Some(unsafe { NSColor::systemRedColor() }),
        ),
        TrayIcon::Transcribing => (
            include_bytes!("../icons/menubar.png"),
            Some(unsafe { NSColor::systemYellowColor() }),
        ),
    };
    let data = unsafe { NSData::dataWithBytes_length(png.as_ptr() as *const std::ffi::c_void, png.len()) };
    let Some(icon) = (unsafe { NSImage::initWithData(NSImage::alloc(), &data) }) else { return };
    unsafe {
        // 全为 template：字形按菜单栏明暗自动反色，染色只负责改颜色本身
        icon.setTemplate(true);
        icon.setSize(objc2_foundation::NSSize::new(24.5, 16.0));
        btn.setImage(Some(&icon));
        btn.setContentTintColor(tint.as_deref());
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
