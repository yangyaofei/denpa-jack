// 诊断: 裸调 CGEventTapCreate 是否挂起/返回什么
use std::ffi::c_void;
use std::time::Instant;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: usize, ui: *mut c_void) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, on: bool);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: *mut c_void, c_str: *const i8, enc: u32) -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: *mut c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFMachPortCreateRunLoopSource(alloc: *mut c_void, port: *mut c_void, order: i64) -> *mut c_void;
}

const K_CFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;

unsafe extern "C" fn cb(_p: *mut c_void, _t: u32, _e: *mut c_void, _u: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

fn main() {
    println!("start");
    let t = Instant::now();
    let tap = unsafe { CGEventTapCreate(1, 0, 1, (1 << 10) | (1 << 11) | (1 << 12), cb as usize, std::ptr::null_mut()) };
    println!("CGEventTapCreate -> {:?} ({}ms)", tap, t.elapsed().as_millis());
    if tap.is_null() {
        println!("NULL: 无辅助功能权限");
        return;
    }
    let t2 = Instant::now();
    let src = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0) };
    println!("source -> {:?} ({}ms)", src, t2.elapsed().as_millis());
    unsafe {
        CGEventTapEnable(tap, true);
        let mode_c = b"com.apple.runloop.defaultMode\0";
        let mode = CFStringCreateWithCString(std::ptr::null_mut(), mode_c.as_ptr() as *const i8, K_CFSTRING_ENCODING_UTF8);
        println!("CFString mode -> {:?}", mode);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), src, mode);
    }
    println!("added, 冒烟 OK");
}
