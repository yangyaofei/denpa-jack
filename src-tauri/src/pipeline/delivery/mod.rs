// 交付域(阶段A 第4步从 deliver.rs 分家; 2026-11-02 焦点探测族归位 capture 后纯化):
// mod.rs = CGEvent 键击模拟(交付动作); ax.rs = AX 直写(交付动作);
// paste.rs = 回执式粘贴事务(paste_tx 归位)。
// 感知类(frontmost_bundle/FocusLock/focused_element_info/ax_trusted)已搬 capture/focus.rs。
pub mod ax;
pub mod paste;

pub use ax::ax_insert;

// CGEvent 按键模拟(线程安全, 无 HIToolbox 主线程断言; C39)
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(source: *const std::ffi::c_void, key_code: u16, down: bool) -> *mut std::ffi::c_void;
    fn CGEventSetFlags(e: *mut std::ffi::c_void, flags: u64);
    fn CGEventPost(tap: u32, e: *mut std::ffi::c_void);
}

/// Cmd+按下 key_code(v 粘贴 / return 回车)
pub fn cg_cmd_key(key_code: u16) -> Result<(), String> {
    cg_key_tap(key_code, 1 << 20)
}

/// 按下 key_code(带修饰 flags), 按住 100ms 再释放(Handy #165: 过快释放丢 chord)
pub fn cg_key_tap(key_code: u16, flags: u64) -> Result<(), String> {
    const KCG_HID_EVENT_TAP: u32 = 0;
    unsafe {
        let down = CGEventCreateKeyboardEvent(std::ptr::null(), key_code, true);
        if down.is_null() {
            return Err("CGEventCreateKeyboardEvent 失败".into());
        }
        CGEventSetFlags(down, flags);
        CGEventPost(KCG_HID_EVENT_TAP, down);
        CFRelease(down);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let up = CGEventCreateKeyboardEvent(std::ptr::null(), key_code, false);
        if !up.is_null() {
            CGEventSetFlags(up, flags);
            CGEventPost(KCG_HID_EVENT_TAP, up);
            CFRelease(up);
        }
    }
    Ok(())
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
}
