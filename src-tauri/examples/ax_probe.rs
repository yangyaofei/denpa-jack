//! ax_probe —— 输入框可达性属性全扫描（多目标采集模式）。
//!
//! 用法: 运行后 60 秒内, 依次**点击**要采集的输入框(空框/打几个字再采一次均可):
//!   cargo run --release --example ax_probe
//! 焦点元素或其内容长度变化时, 自动 dump 该元素的全部可达性属性原始值。
//! 输出按 `=== dump #N ===` 分段, 供横向对比"真实内容/占位符在各框的标记差异"。
//! 只读不写。

fn main() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut last_key = String::new();
    let mut n = 0;
    while std::time::Instant::now() < deadline {
        unsafe {
            let sys = ax::AXUIElementCreateSystemWide();
            let name = ax::cfstring("AXFocusedUIElement");
            let mut focused: ax::CFTypeRef = std::ptr::null_mut();
            if ax::AXUIElementCopyAttributeValue(sys, name, std::ptr::null(), &mut focused) == 0
                && !focused.is_null()
            {
                let el = focused as ax::AXUIElementRef;
                let role = read_attr(el, "AXRole");
                let val = read_attr(el, "AXValue");
                let domid = read_attr(el, "AXDOMIdentifier");
                // key = 进程+角色+DOM标识+内容长度: 元素变化或内容增减都触发一次采集
                let mut pid: i32 = 0;
                ax::AXUIElementGetPid(el, &mut pid);
                let key = format!("{pid}|{role}|{domid}|{}", val.chars().count());
                if key != last_key {
                    last_key = key.clone();
                    n += 1;
                    println!("=== dump #{n} pid={pid} role={role} domid={domid} value_len={}", val.chars().count());
                    dump(el);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    println!("=== 采集结束, 共 {n} 份 ===");
}

unsafe fn dump(el: ax::AXUIElementRef) {
    let attrs = [
        "AXRole",
        "AXSubrole",
        "AXRoleDescription",
        "AXValue",
        "AXPlaceholderValue",
        "AXNumberOfCharacters",
        "AXSelectedText",
        "AXSelectedTextRange",
        "AXDescription",
        "AXHelp",
        "AXTitle",
        "AXValueDescription",
        "AXEditable",
        // WebKit/Electron 私有: DOM 层信息(class/id 会暴露组件库与标记)
        "AXDOMClassList",
        "AXDOMIdentifier",
        "AXDOMHTMLTag",
    ];
    for a in attrs {
        let v = read_attr_raw(el, a);
        println!("  {a:22} = {}", preview(&v, 200));
    }
    // 属性名全集(发现未知的标记属性)
    let all = read_attr_raw(el, "AXAttributeNames");
    println!("  AXAttributeNames       = {}", preview(&all, 2400));
}

fn preview(s: &str, max: usize) -> String {
    let t = s.replace('\n', "\\n");
    if t.chars().count() > max {
        let cut: String = t.chars().take(max).collect();
        format!("{cut}…")
    } else {
        t
    }
}

unsafe fn read_attr_raw(el: ax::AXUIElementRef, name: &str) -> String {
    let n = ax::cfstring(name);
    let mut out: ax::CFTypeRef = std::ptr::null_mut();
    if ax::AXUIElementCopyAttributeValue(el, n, std::ptr::null(), &mut out) == 0 && !out.is_null() {
        ax::cftype_description(out)
    } else {
        "<无>".to_string()
    }
}

unsafe fn read_attr(el: ax::AXUIElementRef, name: &str) -> String {
    read_attr_raw(el, name)
}

// ── 最小 FFI(独立于主工程, 探针自包含) ──
mod ax {
    use std::ffi::c_void;
    pub type AXUIElementRef = *mut c_void;
    pub type CFTypeRef = *mut c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        pub fn AXUIElementCopyAttributeValue(
            el: AXUIElementRef,
            name: *const c_void,
            range: *const c_void,
            value: *mut CFTypeRef,
        ) -> i32;
        pub fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            is_external: bool,
        ) -> CFTypeRef;
        pub fn CFCopyDescription(cf: CFTypeRef) -> CFTypeRef;
        pub fn CFStringGetCString(cf: CFTypeRef, buffer: *mut u8, buffer_size: isize, encoding: u32) -> bool;
    }
    pub unsafe fn cfstring(s: &str) -> CFTypeRef {
        CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, 0x0800_0100, false)
    }
    pub unsafe fn cftype_description(cf: CFTypeRef) -> String {
        let d = CFCopyDescription(cf);
        let mut buf = vec![0u8; 8192];
        if CFStringGetCString(d, buf.as_mut_ptr(), 8192, 0x0800_0100) {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).trim_matches('"').to_string()
        } else {
            format!("<非字符串对象 {cf:?}>")
        }
    }
}
