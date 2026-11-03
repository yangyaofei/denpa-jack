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
    let el = focused_element_info_once_raw()?;
    unsafe {
        let mut pid: i32 = 0;
        if AXUIElementGetPid(el.0, &mut pid) != 0 || pid <= 0 {
            return None;
        }
        Some(FocusInfo {
            pid,
            frame: window_frame_of(el.0),
        })
    }
}

/// 取当前焦点元素并持有引用(给 FocusLock 快照; 探针实测 TextEdit 后台 value 写入生效)
fn focused_element_info_once_raw() -> Option<FocusElement> {
    unsafe {
        let sys = AxGuard(AXUIElementCreateSystemWide());
        if sys.0.is_null() {
            return None;
        }
        let focused = ax_attr_owned(sys.0, KAX_FOCUSED_ELEMENT)?;
        // CFType(Arc 语义) drop 会 release 一次; FocusElement 要自持, 先 retain
        let raw = focused.as_concrete_TypeRef();
        if raw.is_null() {
            return None;
        }
        cf_retain(raw);
        Some(FocusElement(raw))
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

/// 按住热键时的焦点元素引用(AX 快照)——后台写入的目标。
/// CF 对象跨线程持有是官方允许的(retain/release 配对); AXUIElementRef 是裸指针,
/// 需要手动声明 Send(交付线程与录音线程不同)。
pub struct FocusElement(pub(crate) core_foundation::base::CFTypeRef);

unsafe impl Send for FocusElement {}
unsafe impl Sync for FocusElement {}

impl Clone for FocusElement {
    fn clone(&self) -> Self {
        unsafe {
            if !self.0.is_null() {
                cf_retain(self.0);
            }
            FocusElement(self.0)
        }
    }
}

impl Drop for FocusElement {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                cf_release(self.0);
            }
        }
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: *const std::ffi::c_void) -> *const std::ffi::c_void;
}

/// 兼容文件头部已有的 CFRelease 声明(重复声明链接器会合并, 这里只补 Retain)
unsafe fn cf_retain(p: core_foundation::base::CFTypeRef) {
    CFRetain(p);
}
unsafe fn cf_release(p: core_foundation::base::CFTypeRef) {
    CFRelease(p as *mut std::ffi::c_void);
}

/// FocusLock: 录音开始快照, 粘贴前校验(B10); element=焦点元素引用(后台写入用, 2026-11-03)
#[derive(Clone, Default)]
pub struct FocusLock {
    pub bundle: Option<String>,
    /// 按住热键时的焦点输入框元素; None=快照时取不到(不影响原有交付路径)
    pub element: Option<FocusElement>,
    /// 快照元素的进程号——焦点比对的可靠信号(2026-11-03 真机: 目标 App 的
    /// bundle 解析为 "!!none", bundle 比对恒等, "焦点已变"永不触发, 后台写
    /// 分支一次都没进过。pid 从 AX 元素直接拿, 不依赖 lsappinfo)
    pub pid: Option<i32>,
}

impl FocusLock {
    pub fn snapshot() -> Self {
        let element = focused_element_info_once_raw();
        let pid = element.as_ref().and_then(|el| {
            unsafe {
                let mut pid: i32 = 0;
                if AXUIElementGetPid(el.0, &mut pid) == 0 && pid > 0 {
                    Some(pid)
                } else {
                    None
                }
            }
        });
        Self {
            bundle: frontmost_bundle(),
            element,
            pid,
        }
    }
    /// 焦点是否还指向快照时的目标。
    /// 判定优先级(2026-11-03 真机教训: 目标 App bundle 解析为 "!!none", bundle 比对
    /// 恒等导致"焦点已变"永不触发): ①pid 级——快照元素 pid 与当前焦点元素 pid 比对
    /// (全走 AX, 不依赖 lsappinfo); ②bundle 级回退(快照没取到元素时)。
    pub fn still_valid(&self) -> bool {
        if let Some(snap_pid) = self.pid {
            return current_focus_pid() == Some(snap_pid);
        }
        match (&self.bundle, frontmost_bundle()) {
            (Some(a), Some(b)) => a.as_str() == b.as_str(),
            (None, None) => true,
            // 快照拿不到时不拦截(宽松); 当前拿得到但不等 → 拦截
            (None, Some(_)) => true,
            (Some(_), None) => true,
        }
    }
}

/// 当前键盘焦点元素的进程号(单次尝试, 交付路径上不能像探测那样重试 3 次等 90ms)
fn current_focus_pid() -> Option<i32> {
    unsafe {
        let sys = AxGuard(AXUIElementCreateSystemWide());
        if sys.0.is_null() {
            return None;
        }
        let focused = ax_attr_owned(sys.0, KAX_FOCUSED_ELEMENT)?;
        let mut pid: i32 = 0;
        if AXUIElementGetPid(focused.as_concrete_TypeRef(), &mut pid) != 0 || pid <= 0 {
            return None;
        }
        Some(pid)
    }
}
