// 交付域·AX 直写(阶段A 第4步从 deliver.rs 分家; 2026-11-02 焦点探测族归位 capture/focus.rs):
// 只留"写入"能力(AX 直写+回读确认); 只读探测(焦点元素/窗口框/权限)在 capture::focus。
// 注: 直写本身需要定位焦点元素, 故此处保留 SystemWide FFI 与 KAX_FOCUSED_ELEMENT。
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

type AXUIElementRef = core_foundation::base::CFTypeRef; // *const c_void

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, val: CFTypeRef) -> i32;
}

const KAX_SELECTED_TEXT: &str = "AXSelectedText";
const KAX_VALUE: &str = "AXValue";
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

// ax_trusted 已归位 capture/focus.rs(权限查询是感知, 不是交付动作)

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
}
