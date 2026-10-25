// 交付域(阶段A 第4步从 deliver.rs 分家):
// mod.rs = 前台应用探测 + FocusLock + CGEvent 键击模拟(交付域公共能力);
// ax.rs = Accessibility API(焦点探测/直写/权限); paste.rs = 回执式粘贴事务(paste_tx 归位)。
pub mod ax;
pub mod paste;

pub use ax::{ax_insert, ax_trusted, focused_element_info};
#[allow(unused_imports)] // FocusInfo: 消费方经 delivery::ax::FocusInfo 全路径使用, 此处保持重导出完整
pub use ax::FocusInfo;

/// 前台应用 bundle id(lsappinfo, 系统自带)
pub fn frontmost_bundle() -> Option<String> {
    let front = std::process::Command::new("lsappinfo").arg("front").output().ok()?;
    let asn = String::from_utf8_lossy(&front.stdout).trim().to_string();
    if asn.is_empty() {
        return None;
    }
    let info = std::process::Command::new("lsappinfo")
        .args(["info", "-only", "bundleid", &asn])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&info.stdout).trim().to_string();
    s.split('=').next_back().map(|v| v.trim_matches('"').to_string())
}

/// FocusLock: 录音开始快照, 粘贴前校验(B10)
#[derive(Clone, Default)]
pub struct FocusLock {
    pub bundle: Option<String>,
}

impl FocusLock {
    pub fn snapshot() -> Self {
        Self { bundle: frontmost_bundle() }
    }
    pub fn still_valid(&self) -> bool {
        match (&self.bundle, frontmost_bundle()) {
            (Some(a), Some(b)) => a.as_str() == b.as_str(),
            (None, None) => true,
            // 快照拿不到时不拦截(宽松); 当前拿得到但不等 → 拦截
            (None, Some(_)) => true,
            (Some(_), None) => true,
        }
    }
}

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
