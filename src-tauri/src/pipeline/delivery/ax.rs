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
    fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
}

const KAX_SELECTED_TEXT: &str = "AXSelectedText";
const KAX_VALUE: &str = "AXValue";
const KAX_PLACEHOLDER: &str = "AXPlaceholderValue";
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

// ───────── 后台直写探针(2026-11-03 需求验证: 切走焦点后仍写"按热键时"的目标框)─────────

/// 快照目标的持有句柄(pub(crate): autotest 探针跨函数持有; 不出 pipeline 域)
pub struct BgTarget(AxGuard);

/// 快照当前焦点元素(持有引用; 供 8 秒后后台写入)。None=取焦点失败。
pub fn bg_snapshot_focused() -> Option<BgTarget> {
    unsafe {
        let sys = AxGuard(AXUIElementCreateSystemWide());
        if sys.0.is_null() {
            return None;
        }
        let name = CFString::new(KAX_FOCUSED_ELEMENT);
        let mut out: CFTypeRef = std::ptr::null_mut();
        let r = AXUIElementCopyAttributeValue(sys.0, name.as_concrete_TypeRef(), &mut out);
        if r != 0 || out.is_null() {
            return None;
        }
        Some(BgTarget(AxGuard(out)))
    }
}

/// 对快照元素后台写入并回读校验(不激活目标 App, 不碰当前焦点)。
pub fn bg_write_verify(target: &BgTarget, text: &str) -> bool {
    let r = bg_write_report(target, text);
    r.sel_ok || r.value_ok
}

/// 后台写探针详情(区分三条路径各自的返回): AXSelectedText 写 / AXValue 读改写 /
/// 回读信号。诊断"后台直写未生效"到底是哪种失败。
pub struct BgWriteReport {
    pub pid: i32,
    pub sel_set_r: i32,
    pub value_before: Option<String>,
    pub value_set_r: i32,
    pub sel_ok: bool,
    pub value_ok: bool,
}

pub fn bg_write_report(target: &BgTarget, text: &str) -> BgWriteReport {
    unsafe {
        let mut pid: i32 = 0;
        AXUIElementGetPid(target.0 .0, &mut pid);
        let value_before = ax_copy(target.0 .0, KAX_VALUE);
        // 路径1: AXSelectedText 直写
        let sel_name = CFString::new(KAX_SELECTED_TEXT);
        let val = CFString::new(text);
        let sel_set_r = AXUIElementSetAttributeValue(
            target.0 .0,
            sel_name.as_concrete_TypeRef(),
            val.as_concrete_TypeRef() as CFTypeRef,
        );
        // 路径2: AXValue 追加写(读旧值+尾部拼接——非焦点元素拿不到光标位, 追加是最接近的语义)
        let mut value_set_r = -1;
        if let Some(old) = &value_before {
            let merged = format!("{old}{text}");
            let v = CFString::new(&merged);
            let vname = CFString::new(KAX_VALUE);
            value_set_r = AXUIElementSetAttributeValue(
                target.0 .0,
                vname.as_concrete_TypeRef(),
                v.as_concrete_TypeRef() as CFTypeRef,
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        let sel_ok = ax_copy(target.0 .0, KAX_SELECTED_TEXT).map(|s| s.contains(text)).unwrap_or(false);
        let value_ok = ax_copy(target.0 .0, KAX_VALUE).map(|s| s.contains(text)).unwrap_or(false);
        BgWriteReport { pid, sel_set_r, value_before, value_set_r, sel_ok, value_ok }
    }
}

/// 后台追加写入(正式路径, 2026-11-03 需求: 焦点切走后仍写入按热键时快照的输入框)。
/// 形态: 读 AXValue → 尾部拼接 → 写回 → 回读验证。非焦点元素没有选区/光标语义,
/// AXSelectedText 路径多数 App 静默忽略(探针实测 TextEdit 即如此), value 追加是
/// 唯一可靠形态——语音输入"往框里加一段话"通常正是尾部追加语义。
/// Web 输入框陷阱(2026-11-03 真机: OpenCode 输入框): 空框时 AXValue 读到的是
/// placeholder 文本(如"规划、构建或提问…"), 直接拼接会把占位符固化进正文——
/// 读 AXPlaceholderValue 比对, 相等即视为空框直写。
/// 返回 Err(原因) 供交付链回退。
pub fn bg_append_write(el: &crate::capture::FocusElement, text: &str) -> Result<(), String> {
    unsafe {
        let before = ax_copy(el.0, KAX_VALUE).ok_or("后台写: 读目标 AXValue 失败(元素已失效?)")?;
        let placeholder = ax_copy(el.0, KAX_PLACEHOLDER);
        let effective_before = match &placeholder {
            Some(ph) if !ph.is_empty() && *ph == before => "", // 读到的"内容"其实是占位符 → 空框
            _ => before.as_str(),
        };
        let merged = merge_tail(effective_before, text);
        let v = CFString::new(&merged);
        let vname = CFString::new(KAX_VALUE);
        let r = AXUIElementSetAttributeValue(el.0, vname.as_concrete_TypeRef(), v.as_concrete_TypeRef() as CFTypeRef);
        if r != 0 {
            return Err(format!("后台写: AXValue 写入失败(kError={r})"));
        }
        // 回读验证(异步生效给两拍)
        for delay in [60u64, 200u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            if ax_copy(el.0, KAX_VALUE).map(|s| s.contains(text)).unwrap_or(false) {
                return Ok(());
            }
        }
        Err("后台写: 写入返回成功但回读无变化(目标 App 静默忽略)".into())
    }
}

/// 尾部拼接(纯函数): 原文非空且不以换行结尾时补一个换行, 多段语音各占一行;
/// 空框直接写。可测。
fn merge_tail(before: &str, text: &str) -> String {
    if before.is_empty() {
        return text.to_string();
    }
    if before.ends_with('\n') {
        format!("{before}{text}")
    } else {
        format!("{before}\n{text}")
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

#[cfg(test)]
mod bg_tests {
    use super::merge_tail;

    /// 尾部拼接语义: 空框直写 / 非换行结尾补换行(多段各占一行) / 已换行直接接
    #[test]
    fn merge_tail_rules() {
        assert_eq!(merge_tail("", "第一段"), "第一段");
        assert_eq!(merge_tail("已有内容", "第二段"), "已有内容\n第二段");
        assert_eq!(merge_tail("已有内容\n", "第二段"), "已有内容\n第二段");
    }

    /// placeholder 陷阱(2026-11-03 真机): 空框的 AXValue 读到的是占位符文本,
    /// 拼接前必须把"内容==placeholder"归零为空框
    #[test]
    fn placeholder_read_as_content_means_empty() {
        let placeholder = "规划、构建或提问。@ 提及，$ 技能，/ 命令";
        let read_value = placeholder; // Web 输入框空框时 AXValue 返回的就是占位符
        let effective = if read_value == placeholder { "" } else { read_value };
        assert_eq!(merge_tail(effective, "语音文本"), "语音文本", "占位符不得固化进正文");
    }
}
