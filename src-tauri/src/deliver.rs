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
    fn AXValueGetValue(value: CFTypeRef, the_type: u32, value_ptr: *mut std::ffi::c_void) -> bool;
    fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
}

// AXValue 的几何类型（AXValueType: kAXValueCGPointType = 1, kAXValueCGSizeType = 2）
const K_AX_VALUE_CG_POINT: u32 = 1;
const K_AX_VALUE_CG_SIZE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct AxPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AxSize {
    width: f64,
    height: f64,
}

/// 取属性值（Copy 规则：调用方持有 +1 引用，CFType 析构时释放）
fn ax_attr_owned(el: AXUIElementRef, attr: &str) -> Option<CFType> {
    let name = CFString::new(attr);
    let mut out: CFTypeRef = std::ptr::null_mut();
    let r = unsafe { AXUIElementCopyAttributeValue(el, name.as_concrete_TypeRef(), &mut out) };
    if r != 0 || out.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(out) })
}

/// 键盘焦点信息：焦点元素所属进程 + （能取到时）它的窗口框
pub struct FocusInfo {
    pub pid: i32,
    pub frame: Option<(f64, f64, f64, f64)>,
}

/// 当前键盘焦点（光标所在）元素：所属进程 + 窗口框。
///
/// 窗口框取不到不要紧——`screen_context::focused_window` 会用 pid 回退到该应用最前面的窗口
/// （Electron 等应用的焦点元素常拿不到 AXWindow）。
/// 最多试 3 次（AX 服务在应用切换瞬间会返回 kAXErrorCannotComplete）。
pub fn focused_element_info() -> Option<FocusInfo> {
    for attempt in 0..3 {
        if let Some(f) = focused_element_info_once() {
            return Some(f);
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    None
}

fn focused_element_info_once() -> Option<FocusInfo> {
    unsafe {
        let _sys = AxGuard(AXUIElementCreateSystemWide());
        if _sys.0.is_null() {
            return None;
        }
        let focused = ax_attr_owned(_sys.0, KAX_FOCUSED_ELEMENT)?;
        let mut pid: i32 = 0;
        if AXUIElementGetPid(focused.as_concrete_TypeRef(), &mut pid) != 0 || pid <= 0 {
            return None;
        }
        Some(FocusInfo {
            pid,
            frame: window_frame_of(focused.as_concrete_TypeRef()),
        })
    }
}

fn window_frame_of(focused: AXUIElementRef) -> Option<(f64, f64, f64, f64)> {
    unsafe {
        let win = ax_attr_owned(focused, KAX_WINDOW)?;
        let pos = ax_attr_owned(win.as_concrete_TypeRef(), KAX_POSITION)?;
        let size = ax_attr_owned(win.as_concrete_TypeRef(), KAX_SIZE)?;
        let mut p = AxPoint { x: 0.0, y: 0.0 };
        let mut s = AxSize { width: 0.0, height: 0.0 };
        if !AXValueGetValue(
            pos.as_concrete_TypeRef(),
            K_AX_VALUE_CG_POINT,
            &mut p as *mut AxPoint as *mut std::ffi::c_void,
        ) {
            return None;
        }
        if !AXValueGetValue(
            size.as_concrete_TypeRef(),
            K_AX_VALUE_CG_SIZE,
            &mut s as *mut AxSize as *mut std::ffi::c_void,
        ) {
            return None;
        }
        if s.width < 1.0 || s.height < 1.0 {
            return None;
        }
        Some((p.x, p.y, s.width, s.height))
    }
}

const KAX_SELECTED_TEXT: &str = "AXSelectedText";
const KAX_VALUE: &str = "AXValue";
const KAX_FOCUSED_ELEMENT: &str = "AXFocusedUIElement";
const KAX_WINDOW: &str = "AXWindow";
const KAX_POSITION: &str = "AXPosition";
const KAX_SIZE: &str = "AXSize";

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

/// AX 直写光标处 + 多信号回读校验。
///
/// 校验方式（2026-10-14 实测修正，原因见下）：
///   SET `AXSelectedText` → 短时多次回读两个信号：
///     ① `AXValue` 含写入文本且发生变化 → 成功
///     ② `AXSelectedText` 含写入文本   → 成功
///   无任何信号 → 判失败（由调用方回退 Cmd+V 粘贴）。
///
/// 为什么不能只看 AXSelectedText：Gemini（com.google.GeminiMacOS）实测——
///   写入返回 r=0 且**已生效**（`AXValue` 能读到写入文本），但插入后选区折叠，
///   `AXSelectedText` 回读为空 → 旧校验误判"静默忽略" → 又回退粘贴一次
///   → **同一段文本被插入两次**（用户报"特定 App 重复输入两次"）。
///   Preview 类应用则是真忽略（两个信号都无变化）→ 判失败 → 粘贴，行为不变。
/// AX 引用 RAII: Create/Copy 规则 +1 的 AXUIElementRef, 离域自动 CFRelease——
/// 旧实现裸指针无释放, 每次交付/探测泄漏一个 AXUIElement(菜单栏常驻进程累积泄漏, 审核a1/a2)
struct AxGuard(AXUIElementRef);
impl Drop for AxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as *mut std::ffi::c_void) };
        }
    }
}

pub fn ax_insert(text: &str) -> Result<(), String> {
    unsafe {
        let _sys = AxGuard(AXUIElementCreateSystemWide());
        if _sys.0.is_null() {
            return Err("AXUIElementCreateSystemWide 失败".into());
        }
        let focused_name = CFString::new(KAX_FOCUSED_ELEMENT);
        let mut focused: CFTypeRef = std::ptr::null_mut();
        let r =
            AXUIElementCopyAttributeValue(_sys.0, focused_name.as_concrete_TypeRef(), &mut focused);
        if r != 0 || focused.is_null() {
            return Err(format!("取焦点元素失败(kError={r})"));
        }
        let _focused = AxGuard(focused as AXUIElementRef);
        let focused_el: AXUIElementRef = _focused.0;
        let before_sel = ax_copy(focused_el, KAX_SELECTED_TEXT).unwrap_or_default();
        let before_val = ax_copy(focused_el, KAX_VALUE).unwrap_or_default();
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
        if text.is_empty() {
            return Ok(());
        }
        // 异步生效的应用需要给一拍; 两次回读覆盖"立即生效"与"延迟生效"两类
        for delay_ms in [40u64, 120u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let after_sel = ax_copy(focused_el, KAX_SELECTED_TEXT).unwrap_or_default();
            if after_sel.contains(text) {
                return Ok(());
            }
            let after_val = ax_copy(focused_el, KAX_VALUE).unwrap_or_default();
            if after_val.contains(text) && after_val != before_val {
                return Ok(());
            }
        }
        // 无信号: 保持旧判据的语义(可读且未变 → 静默忽略), 文案区分两种失败便于排障
        if before_sel == ax_copy(focused_el, KAX_SELECTED_TEXT).unwrap_or_default() {
            Err("AX 写入被目标应用静默忽略".into())
        } else {
            Err("AX 写入无法确认生效(回读无信号)".into())
        }
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
