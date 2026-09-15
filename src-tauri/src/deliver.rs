// 交付层: AX 直写光标(前后对比防静默失败) + FocusLock 防止文本贴到其它应用 + 权限检测
// 移植自 Swift OutputAndHistory.swift
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};

type AXUIElementRef = core_foundation::base::CFTypeRef; // *const c_void

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        el: AXUIElementRef,
        attr: CFStringRef,
        out: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, val: CFTypeRef) -> i32;
    fn AXIsProcessTrustedWithOptions(opts: core_foundation::dictionary::CFDictionaryRef) -> bool;
}

const KAX_SELECTED_TEXT: &str = "AXSelectedText";
const KAX_FOCUSED_ELEMENT: &str = "AXFocusedUIElement";

fn ax_copy(el: AXUIElementRef, attr: &str) -> Option<String> {
    let name = CFString::new(attr);
    let mut out: CFTypeRef = std::ptr::null_mut();
    let r = unsafe { AXUIElementCopyAttributeValue(el, name.as_concrete_TypeRef(), &mut out) };
    if r != 0 || out.is_null() {
        return None;
    }
    let v = unsafe { CFType::wrap_under_create_rule(out) };
    v.downcast::<CFString>().map(|s| s.to_string())
}

/// AX 直写光标处: 读选区(before) → 写入 → 读回(after) 对比, 防"报成功但 app 静默忽略"
pub fn ax_insert(text: &str) -> Result<(), String> {
    unsafe {
        let sys = AXUIElementCreateSystemWide();
        if sys.is_null() {
            return Err("AXUIElementCreateSystemWide 失败".into());
        }
        let focused_name = CFString::new(KAX_FOCUSED_ELEMENT);
        let mut focused: CFTypeRef = std::ptr::null_mut();
        let r =
            AXUIElementCopyAttributeValue(sys, focused_name.as_concrete_TypeRef(), &mut focused);
        if r != 0 || focused.is_null() {
            return Err(format!("取焦点元素失败(kError={r})"));
        }
        let focused_el: AXUIElementRef = focused;
        let before = ax_copy(focused_el, KAX_SELECTED_TEXT).unwrap_or_default();
        let val = CFString::new(text);
        let sel_name = CFString::new(KAX_SELECTED_TEXT);
        let r = AXUIElementSetAttributeValue(
            focused_el,
            sel_name.as_concrete_TypeRef(),
            val.as_concrete_TypeRef() as CFTypeRef,
        );
        if r != 0 {
            return Err(format!("AX 写入失败(kError={r})"));
        }
        let after = ax_copy(focused_el, KAX_SELECTED_TEXT).unwrap_or_default();
        if !text.is_empty() && after == before && after != text {
            return Err("AX 写入被目标应用静默忽略".into());
        }
        Ok(())
    }
}

/// 辅助功能权限: quiet=仅查询; 否则弹系统授权窗
pub fn ax_trusted(prompt: bool) -> bool {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
        let val = if prompt {
        core_foundation::boolean::CFBoolean::true_value()
    } else {
        core_foundation::boolean::CFBoolean::false_value()
    };
    let opts = CFDictionary::from_CFType_pairs(&[(key, val)]);
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) }
}

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
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
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
