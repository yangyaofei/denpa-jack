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

/// 托盘图标三态。**状态用细线 template 位图 + 系统染色表达**（回到队列化之前的效果）。
/// 历史：C41 首次实现就是这样（`menubar.png` 细线字形，录音/转写只改染色）；
/// 之后我把三态做成"烘焙彩色位图 + 加粗字形"，那是另起一套设计、叠加在原来的逻辑上，
/// 用户明确否掉并给出规则：修 bug 回到原逻辑里改，不要在旧逻辑上再摞一层。
// 菜单栏图标：**恒定一张细线 template 图（menubar.png），不随录音/转写切换**。
//
// 历史（2026-09-18 定案）：
//   1. C41 原始实现：定义了 `set_recording`/`set_transcribing`（改 systemRed/systemYellow 染色），
//      但 `set_recording(true)` 从未被调用；其余调用都在非主线程直接碰 AppKit，
//      `MainThreadMarker::new()` 拿不到主线程就静默 return → **实际效果：图标恒定不变**。
//   2. `d4989f3`（"托盘三态修复"）：补上主线程调度与四处调用点 → 图标开始真的随状态变红/黄。
//   3. `affa729`：我把三态改成"烘焙彩色位图 + 加粗字形"，属在旧逻辑上另叠一套设计；
//      `d28f555` 虽回到细线字形，但仍在变色。
// 结论（用户裁决）：以前图标就是不变的，"为什么它一定要变？"。
// 状态反馈由浮窗负责（`● 录音中` / `… 转写中` / `✦ AI 润色中`），菜单栏图标不再参与状态表达。
pub struct MacTray {
    item: Retained<NSStatusItem>,
    /// 持有 TrayHandler 防止其被释放: 菜单项的 target 是弱引用, 释放后菜单动作失效
    #[allow(dead_code)]
    handler: Retained<TrayHandler>,
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
        // C41: 菜单栏图标沿用 Swift 版自绘(声波→箭头→文本框), 恒定不变(见上方 MacTray 注释)
        let png: &[u8] = include_bytes!("../icons/menubar.png");
        let data = unsafe { NSData::dataWithBytes_length(png.as_ptr() as *const std::ffi::c_void, png.len()) };
        let icon = unsafe { NSImage::initWithData(NSImage::alloc(), &data) }.ok_or("menubar.png 解码失败")?;
        unsafe {
            icon.setTemplate(true);
            icon.setSize(objc2_foundation::NSSize::new(24.5, 16.0));
        }
        if let Some(btn) = item.button(mtm) {
            unsafe {
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
