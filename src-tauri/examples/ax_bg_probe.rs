// 后台 AX 直写可行性探针(2026-11-03 用户需求: 切走焦点后仍写入按热键时快照的输入框)
// 步骤: TextEdit 前台取焦点元素 → 切到 Finder → 对该元素后台写 AXSelectedText → 回读验证
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

type AXUIElementRef = CFTypeRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, val: CFTypeRef) -> i32;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
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

fn ax_str(el: AXUIElementRef, attr: &str) -> Option<String> {
    let v = ax_attr_owned(el, attr)?;
    v.downcast_into::<CFString>().map(|s| s.to_string())
}

fn run(_cmd: &str) {
    let _ = std::process::Command::new("osascript").args(["-e", _cmd]).output();
}

fn main() {
    // 0. 权限自检
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let val = core_foundation::boolean::CFBoolean::false_value();
    let opts = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(key, val)]);
    let trusted = unsafe { ax_trusted_raw(opts.as_concrete_TypeRef()) };
    println!("AX 权限: {trusted}");
    if !trusted {
        println!("探针无辅助功能权限, 无法继续(需在 系统设置>隐私>辅助功能 授权终端)");
        return;
    }

    // 1. 用 open 打开文本文件(不依赖 Apple Events 自动化权限), TextEdit 前台可编辑
    std::fs::write("/tmp/axtest.txt", "TARGET-BOX-123").unwrap();
    let _ = std::process::Command::new("open").args(["-a", "TextEdit", "/tmp/axtest.txt"]).output();
    std::thread::sleep(std::time::Duration::from_millis(2500));

    // 2. 前台取 TextEdit 焦点元素
    let sys = unsafe { AXUIElementCreateSystemWide() };
    // 诊断: 当前前台是谁 + focused 元素错误码
    // 2.5 确保前台稳定在 TextEdit(等待用户视觉确认窗口已开)
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let mut probe_out: CFTypeRef = std::ptr::null_mut();
    let probe_r = unsafe {
        let name = CFString::new("AXFocusedUIElement");
        AXUIElementCopyAttributeValue(sys, name.as_concrete_TypeRef(), &mut probe_out)
    };
    println!("AXFocusedUIElement err={probe_r} null={}", probe_out.is_null());
    if probe_r != 0 || probe_out.is_null() {
        println!("取焦点失败, 退出");
        return;
    }
    let focused = unsafe { CFType::wrap_under_create_rule(probe_out) };
    let el = focused.as_concrete_TypeRef();
    let before = ax_str(el, "AXValue").unwrap_or_default();
    println!("TextEdit 焦点元素当前值: {before:?}");

    // 3. 等待窗口: 打印倒计时, 用户有时间切到任意别的地方(真机协作验证)
    println!(">>> 请在 8 秒内切换到别的 App/窗口 <<<");
    for i in (1..=8).rev() {
        println!(">>> {i} ...");
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    // 4. 后台写: 对快照元素设 AXSelectedText(替换全部内容)
    let sel = CFString::new("AXSelectedText");
    let new_text = CFString::new("BACKGROUND-WRITE-OK");
    let r = unsafe { AXUIElementSetAttributeValue(el, sel.as_concrete_TypeRef(), new_text.as_concrete_TypeRef() as CFTypeRef) };
    println!("后台 AXSelectedText 写入返回: {r} (0=成功)");
    std::thread::sleep(std::time::Duration::from_millis(600));

    // 5. 回读验证(仍后台)
    let after = ax_str(el, "AXValue").unwrap_or_default();
    let sel_now = ax_str(el, "AXSelectedText").unwrap_or_default();
    println!("后台回读 AXValue: {after:?} / AXSelectedText: {sel_now:?}");
    let ok = after.contains("BACKGROUND-WRITE-OK") || sel_now.contains("BACKGROUND-WRITE-OK");
    println!("{}", if ok { "结论: 后台直写可行 ✓" } else { "结论: 后台直写未生效 ✗" });

    // 6. 不自动清理: 窗口留给用户肉眼确认写入结果
    unsafe { CFRelease(sys as *mut _) };
}

unsafe fn ax_trusted_raw(opts: core_foundation::dictionary::CFDictionaryRef) -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(opts: core_foundation::dictionary::CFDictionaryRef) -> bool;
    }
    AXIsProcessTrustedWithOptions(opts)
}
