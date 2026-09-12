//! B50/B51 统一按键引擎: 一个引擎管捕获+触发。前台只发请求+显示。
//!
//! 原子操作:
//! - pause_all / resume_all / is_paused — 常规触发暂停(录制期间防自杀循环)
//! - capture(on_change) — 一次调用持续推送, 迭代器语义: 组合变化才推,
//!   全部抬起后 300ms 内无新按下 → final → 函数返回。
//!
//! 事件源: CGEventTap listenOnly(keyDown/keyUp/flagsChanged), 辅助功能权限。
//! Fn 判定(B51): keycode=63 是 Fn 专用键码, flagsChanged 事件携带它——
//! flags 的 0x800000 是设备相关位, 会被无关动作点亮(实测误触发), 禁止用作判定。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 常规触发暂停(录制期间按快捷键不应开录音)
static PAUSED: AtomicBool = AtomicBool::new(false);

/// tap 线程是否已启动(只启动一次, 常驻)
static TAP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 录制捕获开关(tap 回调里据此决定是否记录; 平时关闭零开销)
static CAPTURE_ON: AtomicBool = AtomicBool::new(false);

/// 当前按下的 keycode 集合(tap 回调线程写, capture 轮询读)
static PRESSED: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Fn 是否按下(B51: keycode=63 事件驱动, 非 flags 位)
static FN_DOWN: Mutex<bool> = Mutex::new(false);

// ── 常规触发(Handy 同构: 精确集合匹配, 单引擎) ──
/// 注册的触发目标(组合字符串, 如 "ctrl+cmd+alt"/"fn"/"ctrl+alt+f5")
static TRIGGER_TARGETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 目标是否被按下(tap 回调置 true → lib 控制线程消费发 Pressed)
static TARGET_HIT: AtomicBool = AtomicBool::new(false);
/// 上轮命中的目标索引(全部抬起时发 Released; -1=无)
static HIT_INDEX: Mutex<i32> = Mutex::new(-1);
/// 触发回调(lib.rs setup 注入, 内部只发信号——C13 回调不阻塞)
static TRIGGER_SEND: Mutex<Option<Box<dyn Fn(bool) + Send>>> = Mutex::new(None);

/// 解析组合字符串 → 语义名集合(与 key_name 输出对齐)
fn combo_parts(s: &str) -> Vec<String> {
    s.split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .map(|p| match p.as_str() {
            "option" => "alt".into(),
            "super" | "meta" => "cmd".into(),
            other => other.into(),
        })
        .collect()
}

/// 修饰键 keycode → 语义名(Handy keycode.rs:256 对齐; key_name 对修饰键无覆盖, 单独映射)
fn modifier_name(kc: i64) -> Option<&'static str> {
    Some(match kc {
        0x37 => "cmd",   // kVK_Command
        0x38 => "shift", // kVK_Shift
        0x3A => "alt",   // kVK_Option
        0x3B => "ctrl",  // kVK_Control
        0x3C => "shift", // kVK_RightShift
        0x3D => "alt",   // kVK_RightOption
        0x3E => "ctrl",  // kVK_RightControl
        _ => return None,
    })
}

/// 精确集合匹配(Handy types/modifiers.rs:142 语义):
/// event 的语义集合必须与目标完全一致——多余修饰位/缺失位都=不匹配。
fn matches_target(event_parts: &[String], target_parts: &[String]) -> bool {
    if event_parts.len() != target_parts.len() {
        return false;
    }
    target_parts.iter().all(|t| event_parts.contains(t))
}

/// 对注册目标做一次完整匹配; 命中返回索引
fn hit_index(event_parts: &[String]) -> Option<usize> {
    TRIGGER_TARGETS
        .lock()
        .unwrap()
        .iter()
        .enumerate()
        .find(|(_, t)| matches_target(event_parts, &combo_parts(t)))
        .map(|(i, _)| i)
}

/// macOS Fn 专用键码(kVK_Function/kVK_ANSI_Fn=63); flagsChanged 事件携带它
const KEYCODE_FN: i64 = 63;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: usize,
        user_info: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CFMachPortCreateRunLoopSource(
        alloc: *mut std::ffi::c_void,
        port: *mut std::ffi::c_void,
        order: i64,
    ) -> *mut std::ffi::c_void;
    fn CFRunLoopGetCurrent() -> *mut std::ffi::c_void;
    fn CFRunLoopAddSource(rl: *mut std::ffi::c_void, src: *mut std::ffi::c_void, mode: *mut std::ffi::c_void);
    fn CFRunLoopRun();
    fn CGEventGetIntegerValueField(ev: *mut std::ffi::c_void, field: u32) -> i64;
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, on: bool);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: *mut std::ffi::c_void, c_str: *const i8, enc: u32) -> *mut std::ffi::c_void;
}

/// kCFRunLoopDefaultMode 的字符串值(CFStringCreateWithCString 现造 CFStringRef 传入).
/// 坑史(C53): ①传 Rust 字节串裸指针→CF 当 CFStringRef 解引用→CFHash EXC_BREAKPOINT(页面点+即死)
/// ②extern static kCFRunLoopDefaultMode: c_void→c_void 是零大小类型, extern static ZST 的引用
///   地址≠符号地址(垃圾)→CFRunLoopAddSource SIGBUS(实测 probe exit=138)
/// ③extern static 声明为指针类型→仍然 SIGBUS(静态符号引用在本环境不可靠)
/// ④CFStringCreateWithCString 运行时现造→冒烟 PASS(唯一可靠路径)
const K_CFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;

fn default_mode_cfstring() -> *mut std::ffi::c_void {
    unsafe {
        CFStringCreateWithCString(
            std::ptr::null_mut(),
            b"com.apple.runloop.defaultMode\0".as_ptr() as *const i8,
            K_CFSTRING_ENCODING_UTF8,
        )
    }
}

/// kCGEventKeyDown(10) | kCGEventKeyUp(11) | kCGEventFlagsChanged(12)
const TAP_EVENT_MASK: u64 = (1 << 10) | (1 << 11) | (1 << 12);
/// kCGEventGetIntegerValueField 的字段号: 9=keyCode, 8=flags
const FIELD_KEYCODE: u32 = 9;
const FIELD_FLAGS: u32 = 8;
/// 全部抬起后判定 final 的等待窗口
const FINAL_WINDOW_MS: u64 = 300;

/// keycode → 语义名(US 布局 kVK_ANSI 真实码表; 与前端 HotkeyConfig key 值可互转)
fn key_name(kc: i64) -> String {
    // kVK_ANSI 字母键码(不连续!): 0=A 1=S 2=D 3=F 4=H 5=G 6=Z 7=X 8=C 9=V
    // 11=B 12=N 13=M 14=, 15=. 16=/ 17=O(实为'.'? no) —— 完整表见下 match
    let name = match kc {
        0 => "a", 1 => "s", 2 => "d", 3 => "f", 4 => "h", 5 => "g", 6 => "z",
        7 => "x", 8 => "c", 9 => "v", 11 => "b", 12 => "n", 13 => "m",
        18 => "1", 19 => "2", 20 => "3", 21 => "4", 22 => "6", 23 => "5",
        25 => "=", 26 => "9", 27 => "0", 28 => "(", 29 => ";", 31 => "o",
        32 => "u", 34 => "i", 35 => "p", 36 => "enter", 37 => "l", 38 => "j",
        40 => "k", 41 => ";", 43 => ",", 45 => "-", 46 => "n", 47 => ".",
        49 => "space", 50 => "`", 53 => "esc",
        _ => "",
    };
    if !name.is_empty() {
        return name.to_string();
    }
    match kc {
        14 => "e".into(), 15 => "r".into(), 16 => "y".into(), 17 => "t".into(),
        10 => "q".into(), 30 => "=".into(), 33 => "[".into(), 34 => "]".into(),
        39 => "'".into(), 42 => "\\".into(),
        123 => "left".into(), 124 => "right".into(), 125 => "down".into(), 126 => "up".into(),
        44 => "/".into(),
        122 => "f1".into(), 120 => "f2".into(), 99 => "f3".into(), 118 => "f4".into(),
        96 => "f5".into(), 97 => "f6".into(), 98 => "f7".into(), 100 => "f8".into(),
        101 => "f9".into(), 109 => "f10".into(), 103 => "f11".into(), 111 => "f12".into(),
        _ => format!("key{kc}"),
    }
}

/// 当前按下集合 → 组合字符串。keycode=63(Fn) 显示为 "fn"; 修饰键直接用 key_name 的键码。
/// 注意: 修饰键(ctrl/cmd/…)也有 keydown/keyup 与各自 keycode, 会以 "keyXX" 兜底名出现——
/// 录制组合时修饰键经 combo 显示为 keyXX 是可接受的中间态; 最终提交由 confirm 转换。
fn combo_string() -> String {
    let pressed = PRESSED.lock().unwrap().clone();
    let mut parts: Vec<String> = pressed.iter().map(|&kc| key_name(kc)).collect();
    if parts.is_empty() && *FN_DOWN.lock().unwrap() {
        parts.push("fn".into());
    }
    parts.join("+")
}

/// tap 回调(listenOnly: 只观察不消费)
unsafe extern "C" fn tap_callback(
    _proxy: *mut std::ffi::c_void,
    event_type: u32,
    event: *mut std::ffi::c_void,
    _user_info: *mut std::ffi::c_void,
) -> *mut std::ffi::c_void {
    let keycode = CGEventGetIntegerValueField(event, FIELD_KEYCODE);
    let mut state_changed = false;
    match event_type {
        10 => {
            // keyDown: 入集合(去重——系统 auto-repeat 不重复计)
            let mut pressed = PRESSED.lock().unwrap();
            if !pressed.contains(&keycode) {
                pressed.push(keycode);
                state_changed = true;
            }
        }
        11 => {
            // keyUp: 出集合(含 Fn 抬起)
            let mut pressed = PRESSED.lock().unwrap();
            let n = pressed.len();
            pressed.retain(|&x| x != keycode);
            if pressed.len() != n {
                state_changed = true;
            }
            if keycode == KEYCODE_FN {
                let mut fn_down = FN_DOWN.lock().unwrap();
                if *fn_down {
                    *fn_down = false;
                    state_changed = true;
                }
            }
        }
        12 => {
            // flagsChanged: 修饰键/Fn。Fn 以 keycode=63 标识(B51: 不看 flags 位,
            // 0x800000 是设备相关位会被无关动作点亮)。修饰键的 down/up 也在此事件:
            // keycode=63+flags 含 fn 位 → fn down; 否则若 keycode==63 → fn up。
            if keycode == KEYCODE_FN {
                let flags = CGEventGetIntegerValueField(event, FIELD_FLAGS) as u64;
                let fn_down = (flags & 0x0000_0008) != 0 || (flags & 0x0080_0000) != 0;
                let mut fn_lock = FN_DOWN.lock().unwrap();
                if *fn_lock != fn_down {
                    *fn_lock = fn_down;
                    state_changed = true;
                }
                drop(fn_lock);
                let mut pressed = PRESSED.lock().unwrap();
                if fn_down && !pressed.contains(&KEYCODE_FN) {
                    pressed.push(KEYCODE_FN);
                    state_changed = true;
                } else if !fn_down {
                    let n = pressed.len();
                    pressed.retain(|&x| x != KEYCODE_FN);
                    if pressed.len() != n {
                        state_changed = true;
                    }
                }
            } else {
                // 修饰键 down/up(FlagsChanged): 也入/出 PRESSED——纯修饰组合靠它成立
                let mut pressed = PRESSED.lock().unwrap();
                let flags = CGEventGetIntegerValueField(event, FIELD_FLAGS) as u64;
                // 修饰键按下判定: 事件 flags 含该键自身的位(Handy reconcile 思路的轻量版)
                let is_down = match modifier_name(keycode) {
                    Some("cmd") => flags & 0x0010_0000 != 0,
                    Some("shift") => flags & 0x0002_0000 != 0,
                    Some("alt") => flags & 0x0008_0000 != 0,
                    Some("ctrl") => flags & 0x0004_0000 != 0,
                    _ => false,
                };
                if is_down && !pressed.contains(&keycode) {
                    pressed.push(keycode);
                    state_changed = true;
                } else if !is_down {
                    let n = pressed.len();
                    pressed.retain(|&x| x != keycode);
                    if pressed.len() != n {
                        state_changed = true;
                    }
                }
            }
        }
        _ => {}
    }

    // 常规触发匹配(录制模式关闭时; 状态有变化才评估)
    if state_changed && !CAPTURE_ON.load(Ordering::SeqCst) {
        evaluate_trigger();
    }
    std::ptr::null_mut()
}

/// 触发评估: 当前按下集合 → 语义名集合 → 精确匹配注册目标。
/// 命中→TARGET_HIT=true+回调(true); 从命中离开→回调(false)。PAUSED 时跳过。
fn evaluate_trigger() {
    if is_paused() {
        return;
    }
    let pressed = PRESSED.lock().unwrap().clone();
    let fn_down = *FN_DOWN.lock().unwrap();
    let mut parts: Vec<String> = pressed
        .iter()
        .filter_map(|&kc| {
            if kc == KEYCODE_FN {
                None
            } else {
                modifier_name(kc).map(|s| s.to_string()).or_else(|| Some(key_name(kc)))
            }
        })
        .collect();
    if fn_down {
        parts.push("fn".into());
    }
    parts.sort();
    parts.dedup();

    let mut hit = HIT_INDEX.lock().unwrap();
    match hit_index(&parts) {
        Some(idx) => {
            if *hit != idx as i32 {
                *hit = idx as i32;
                TARGET_HIT.store(true, Ordering::SeqCst);
                crate::log::elog(&format!("[key-engine] trigger Pressed: {}", parts.join("+")));
                if let Some(send) = TRIGGER_SEND.lock().unwrap().as_ref() {
                    send(true);
                }
            }
        }
        None => {
            if *hit >= 0 {
                crate::log::elog("[key-engine] trigger Released");
                *hit = -1;
                if let Some(send) = TRIGGER_SEND.lock().unwrap().as_ref() {
                    send(false);
                }
            }
        }
    }
}

/// 注册常规触发目标+回调(lib.rs setup 调用一次; reapply 时更新目标)
pub fn set_trigger_targets(targets: Vec<String>, send: Box<dyn Fn(bool) + Send>) {
    *TRIGGER_TARGETS.lock().unwrap() = targets.clone();
    *TRIGGER_SEND.lock().unwrap() = Some(send);
    crate::log::elog(&format!("[key-engine] 触发目标 {} 个: {}", targets.len(), targets.join(" | ")));
}

/// 更新目标(保留回调; reapply_hotkey 用)
pub fn update_trigger_targets(targets: Vec<String>) {
    *TRIGGER_TARGETS.lock().unwrap() = targets;
    crate::log::elog("[key-engine] 触发目标已更新");
}

/// 启动 tap 线程(应用启动时调用一次, 常驻 RunLoop)
pub fn spawn_tap() -> Result<(), String> {
    if TAP_ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }
    std::thread::spawn(|| unsafe {
        crate::log::elog("[key-engine] tap 线程启动");
        // kCGSessionEventTap=1, kCGHeadInsertEventTap=0, kCGEventTapOptionListenOnly=1
        let tap = CGEventTapCreate(
            1,
            0,
            1,
            TAP_EVENT_MASK,
            tap_callback as usize,
            std::ptr::null_mut(),
        );
        if tap.is_null() {
            crate::log::elog("[key-engine] CGEventTapCreate 失败(需辅助功能权限)");
            return;
        }
        let source = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, default_mode_cfstring());
        CGEventTapEnable(tap, true);
        TAP_ACTIVE.store(true, Ordering::SeqCst);
        crate::log::elog("[key-engine] CGEventTap 就绪");
        CFRunLoopRun();
    });
    Ok(())
}

/// 录制捕获: 阻塞直至 final。
/// 推送规则(对齐用户裁决):
/// - 组合**变化才**调 on_change(按住不动不推)
/// - 全部抬起后 300ms 内无新按下 → 返回最终组合
/// - 用户一直不松手就一直在过程中(实时提示条由 on_change 驱动)
pub fn capture(on_change: &dyn Fn(String)) -> String {
    // 复位状态
    PRESSED.lock().unwrap().clear();
    *FN_DOWN.lock().unwrap() = false;
    CAPTURE_ON.store(true, Ordering::SeqCst);
    crate::log::elog("[key-engine] capture 开始");

    let mut last_pushed = String::new();
    let mut all_up_since: Option<std::time::Instant> = None;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let combo = combo_string();
        let pressed_count = PRESSED.lock().unwrap().len();
        let fn_down = *FN_DOWN.lock().unwrap();
        let any_down = pressed_count > 0 || fn_down;

        if any_down {
            if combo != last_pushed {
                on_change(combo.clone());
                last_pushed = combo;
            }
            all_up_since = None;
        } else if !last_pushed.is_empty() {
            // 全部抬起: 300ms 窗口内无新按下 → final
            let since = *all_up_since.get_or_insert(std::time::Instant::now());
            if since.elapsed().as_millis() as u64 >= FINAL_WINDOW_MS {
                CAPTURE_ON.store(false, Ordering::SeqCst);
                crate::log::elog(&format!("[key-engine] final: {last_pushed}"));
                return last_pushed;
            }
        }
        // last_pushed 为空且无按下: 等用户开始按
    }
}

pub fn pause_all() {
    PAUSED.store(true, Ordering::SeqCst);
}

pub fn resume_all() {
    PAUSED.store(false, Ordering::SeqCst);
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_name_letters_and_digits() {
        assert_eq!(key_name(0), "a");
        assert_eq!(key_name(6), "z"); // kVK_ANSI_Z=6(非连续映射)
        assert_eq!(key_name(1), "s");
        assert_eq!(key_name(18), "1");
        assert_eq!(key_name(27), "0");
    }

    #[test]
    fn key_name_specials() {
        assert_eq!(key_name(49), "space");
        assert_eq!(key_name(36), "enter");
        assert_eq!(key_name(96), "f5");
        assert_eq!(key_name(111), "f12");
    }

    #[test]
    fn key_name_unknown_gives_keycode_fallback() {
        assert_eq!(key_name(999), "key999");
    }

    #[test]
    fn pause_toggle() {
        pause_all();
        assert!(is_paused());
        resume_all();
        assert!(!is_paused());
    }

    #[test]
    fn matches_target_exact_semantics() {
        // Handy matches 语义: 多余修饰=不匹配, 顺序无关, 数量必须相等
        let target = combo_parts("ctrl+cmd+alt");
        assert!(matches_target(&combo_parts("alt+ctrl+cmd"), &target));
        assert!(!matches_target(&combo_parts("ctrl+cmd"), &target)); // 少一个
        assert!(!matches_target(&combo_parts("ctrl+cmd+alt+shift"), &target)); // 多一个
        assert!(!matches_target(&combo_parts("ctrl+alt"), &target));
        // fn 目标
        let fn_t = combo_parts("fn");
        assert!(matches_target(&combo_parts("fn"), &fn_t));
        assert!(!matches_target(&combo_parts("fn+ctrl"), &fn_t));
    }

    #[test]
    fn modifier_name_covers_all() {
        assert_eq!(modifier_name(0x37), Some("cmd"));
        assert_eq!(modifier_name(0x38), Some("shift"));
        assert_eq!(modifier_name(0x3A), Some("alt"));
        assert_eq!(modifier_name(0x3B), Some("ctrl"));
        assert_eq!(modifier_name(0x3D), Some("alt"));
        assert_eq!(modifier_name(0x3F), None); // Fn 走 FN_DOWN, 不是修饰映射
    }

    #[test]
    fn combo_parts_normalizes_aliases() {
        assert_eq!(combo_parts("option+meta+f5"), vec!["alt", "cmd", "f5"]);
        assert_eq!(combo_parts(""), Vec::<String>::new());
    }
}
