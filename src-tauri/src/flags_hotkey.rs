//! C51 纯修饰键热键(Ctrl/Cmd/Alt 无字母组合): 轮询当前修饰 flags
//! RegisterEventHotKey/global-shortcut 都要求非修饰键, 纯修饰组合只能这样监听。
//! 40ms 轮询对人手按下(>80ms)足够; 组合精确匹配(flags 恰好等于目标)。

const CTRL: u64 = 0x0004_0000;
const ALT: u64 = 0x0008_0000;
const CMD: u64 = 0x0010_0000;
/// Fn/Globe 键 flags(NX_FUNCTIONKEYMASK): 闪电说/Handy 同款可捕获——单 Fn 即 flags 恰为该值
const FN: u64 = 0x0080_0000;

pub fn flags_of(s: &str) -> Option<u64> {
    let mut f = 0u64;
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => f |= CTRL,
            "alt" | "option" => f |= ALT,
            "cmd" | "super" | "meta" => f |= CMD,
            "shift" => f |= 0x0002_0000,
            "fn" | "function" | "globe" => f |= FN,
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
