//! C51 纯修饰键热键(Ctrl/Cmd/Alt 无字母组合): 轮询当前修饰 flags
//! RegisterEventHotKey/global-shortcut 都要求非修饰键, 纯修饰组合只能这样监听。
//! 40ms 轮询对人手按下(>80ms)足够; 组合精确匹配(flags 恰好等于目标)。

const CTRL: u64 = 0x0004_0000;
const ALT: u64 = 0x0008_0000;
const CMD: u64 = 0x0010_0000;
const SHIFT: u64 = 0x0002_0000;
/// Fn 的 flags 位(0x800000)是设备相关位, 会被无关动作点亮(实测误触发)——禁止用于匹配。
/// 保留常量仅供 flags_to_parts 展示用; 触发判定走 key_engine 的 keycode=63 通道。
#[allow(dead_code)]
const FN: u64 = 0x0080_0000;

pub fn flags_of(s: &str) -> Option<u64> {
    let mut f = 0u64;
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => f |= CTRL,
            "alt" | "option" => f |= ALT,
            "cmd" | "super" | "meta" => f |= CMD,
            "shift" => f |= SHIFT,
            // B51: fn 不再走 flags 位匹配——0x800000 是设备相关位, 会被无关动作点亮
            // (实测 0x800100: 0x100 CapsLock 设备位常亮 + 0x800000 误亮) → 误触发。
            // Fn 的唯一可靠信号 = keycode 63(flagsChanged), 由 key_engine 的 tap 捕获。
            "fn" | "function" | "globe" => return None,
            _ => return None,
        }
    }
    (f != 0).then_some(f)
}

/// 启动轮询线程(仅修饰 flags)。按下=当前 flags 恰为某目标 → Pressed; 离开 → Released。
use std::sync::mpsc::{self, Sender, TryRecvError};

pub static TARGETS_TX: std::sync::Mutex<Option<Sender<Vec<String>>>> = std::sync::Mutex::new(None);

/// 更新纯修饰组合集(保存配置后调用; 无纯修饰条目则清空暂停)
pub fn update_targets(targets: Vec<String>) {
    let _ = TARGETS_TX
        .lock()
        .unwrap()
        .as_ref()
        .map(|tx| tx.send(targets));
}

// ── B49 录制模式(闪电说同款): 引擎捕获下一个修饰组合(含 Fn) ──
static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 引擎捕获结果(存这里, 前端 poll_key_capture 取走)
pub static CAPTURED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 进入录制模式: 轮询线程捕获下一个稳定修饰组合(含 Fn)
pub fn start_recording_mode() {
    *CAPTURED.lock().unwrap() = None;
    RECORDING.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn stop_recording_mode() {
    RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub fn flags_to_parts(f: u64) -> Vec<&'static str> {
    let mut v = vec![];
    if f & CTRL != 0 { v.push("ctrl"); }
    if f & ALT != 0 { v.push("option"); }
    if f & CMD != 0 { v.push("cmd"); }
    if f & 0x0002_0000 != 0 { v.push("shift"); }
    if f & FN != 0 { v.push("fn"); }
    v
}

pub fn spawn(targets: Vec<String>, send: Box<dyn Fn(bool) + Send>) {
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    *TARGETS_TX.lock().unwrap() = Some(tx);
    let mut masks: Vec<u64> = targets.iter().filter_map(|s| flags_of(s)).collect();
    crate::log::elog(&format!("[flags-tap] 启动, 组合数={}", masks.len()));
    std::thread::spawn(move || {
        let mut active = false;
        let mut last_logged = 0u64;
        loop {
            // 40ms 轮询 + 动态接收新 targets(配置变更即生效)
            match rx.try_recv() {
                Ok(new) => {
                    masks = new.iter().filter_map(|s| flags_of(s)).collect();
                    active = false;
                    crate::log::elog(&format!("[flags-tap] targets 更新, 组合数={}", masks.len()));
                }
                Err(TryRecvError::Disconnected) => {}
                Err(TryRecvError::Empty) => {}
            }
            if masks.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
            let flags = current_modifier_flags();
            // 录制模式: 捕获稳定组合(连续 3 帧同值非零=120ms, 过滤按键瞬间的中间态)
            if RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
                static mut LAST_STABLE: u64 = 0;
                static mut STABLE_COUNT: u32 = 0;
                static mut PREV: u64 = 0;
                unsafe {
                    if flags == PREV {
                        STABLE_COUNT += 1;
                    } else {
                        PREV = flags;
                        STABLE_COUNT = 1;
                    }
                    if STABLE_COUNT >= 3 && flags != 0 {
                        let parts = flags_to_parts(flags).join("+");
                        crate::log::elog(&format!("[flags-tap] 录制捕获: {parts}"));
                        *CAPTURED.lock().unwrap() = Some(parts);
                        RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                continue;
            }
            if flags != last_logged {
                crate::log::elog(&format!("[flags-tap] flags={flags:#x}"));
                last_logged = flags;
            }
            // 按位与匹配: 实测 flags 携带杂位(0x29 等, alpha/device 状态), 精确相等永不命中
            let hit = masks.iter().any(|m| (flags & m) == *m);
            if hit && !active {
                active = true;
                crate::log::elog("[flags-tap] Pressed");
                send(true);
            } else if !hit && active {
                active = false;
                crate::log::elog("[flags-tap] Released");
                send(false);
            }
        }
    });
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// 当前硬件修饰键状态(合成事件 flags 恒 0——此前用 CGEvent::new 是错的)
    fn CGEventSourceFlagsState(state_id: u32) -> u64;
}

/// 读当前修饰键 flags(CombinedSessionState=2)
fn current_modifier_flags() -> u64 {
    // kCGEventSourceStateCombinedSessionState=0(含所有进程; 2=仅本 Session——外部进程修饰读不到)
    unsafe { CGEventSourceFlagsState(0) }
}

#[cfg(test)]
mod b49_tests {
    use super::*;
    #[test]
    fn flags_to_parts_covers_all() {
        assert_eq!(flags_to_parts(CTRL), vec!["ctrl"]);
        assert_eq!(flags_to_parts(CTRL | ALT | CMD), vec!["ctrl", "option", "cmd"]);
    }
    #[test]
    fn flags_of_fn_is_none() {
        // B51: fn 走 keycode=63 通道(key_engine), flags 位匹配已废除(误触发根因)
        assert_eq!(flags_of("fn"), None);
        assert_eq!(flags_of("ctrl+cmd"), Some(CTRL | CMD));
    }
    #[test]
    fn shift_flag_value() {
        assert_eq!(flags_of("shift"), Some(SHIFT));
    }
}
