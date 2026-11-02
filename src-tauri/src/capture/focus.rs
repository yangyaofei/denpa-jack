//! capture/focus：前台与焦点感知（从 delivery 归位的"感知世界"能力）。
//!
//! 归位依据（docs/ARCHITECTURE.md）：焦点探测是 capture 性质（感知当前环境），
//! delivery 只留动作（写入/键击）。此前 recording 反向调用 delivery 的探测函数，
//! 方向律修复后依赖才顺：capture → pipeline 单向。
//!
//! 门面（外部只许经 capture::focus 调这些）：
//!   - focus_snapshot() — 快照前台应用（FocusLock::snapshot 的意图名）
//!   - focused_element_info() — 光标所在元素（进程+窗口框）
//!   - ax_trusted(prompt) — 辅助功能权限
//!   - frontmost_bundle() — 前台应用 bundle id
//!
//! 注意：AxGuard 与 ax.rs 各持一份 4 行 RAII（避免跨文件私有依赖，
//! 复制成本低于耦合成本；若未来第三处需要，再提基座）。
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};

type AXUIElementRef = CFTypeRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXIsProcessTrustedWithOptions(opts: core_foundation::dictionary::CFDictionaryRef) -> bool;
    fn AXValueGetValue(value: CFTypeRef, the_type: u32, value_ptr: *mut std::ffi::c_void) -> bool;
    fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
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

fn ax_attr_owned(el: AXUIElementRef, attr: &str) -> Option<CFType> {
    let name = CFString::new(attr);
    let mut out: CFTypeRef = std::ptr::null_mut();
    let r = unsafe { AXUIElementCopyAttributeValue(el, name.as_concrete_TypeRef(), &mut out) };
    if r != 0 || out.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(out) })
}

/// AX 引用 RAII: Create/Copy 规则 +1 的 AXUIElementRef, 离域自动 CFRelease
struct AxGuard(AXUIElementRef);
impl Drop for AxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as *mut std::ffi::c_void) };
        }
    }
}

const KAX_FOCUSED_ELEMENT: &str = "AXFocusedUIElement";
const KAX_WINDOW: &str = "AXWindow";
const KAX_POSITION: &str = "AXPosition";
const KAX_SIZE: &str = "AXSize";

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
