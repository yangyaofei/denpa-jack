//! C51 纯修饰键热键(Ctrl/Cmd/Alt 无字母组合): 轮询当前修饰 flags
//! RegisterEventHotKey/global-shortcut 都要求非修饰键, 纯修饰组合只能这样监听。
//! 40ms 轮询对人手按下(>80ms)足够; 组合精确匹配(flags 恰好等于目标)。

const CTRL: u64 = 0x0004_0000;
const ALT: u64 = 0x0008_0000;
const CMD: u64 = 0x0010_0000;

pub fn flags_of(s: &str) -> Option<u64> {
    let mut f = 0u64;
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => f |= CTRL,
            "alt" | "option" => f |= ALT,
            "cmd" | "super" | "meta" => f |= CMD,
            "shift" => f |= 0x0002_0000,
            _ => return None,
        }
    }
    (f != 0).then_some(f)
}

/// 启动轮询线程(仅修饰 flags)。按下=当前 flags 恰为某目标 → Pressed; 离开 → Released。
pub fn spawn(targets: Vec<String>, send: Box<dyn Fn(bool) + Send>) {
    let masks: Vec<u64> = targets.iter().filter_map(|s| flags_of(s)).collect();
    if masks.is_empty() {
        return;
    }
    crate::log::elog(&format!("[flags-tap] 启动, 组合数={}", masks.len()));
    std::thread::spawn(move || {
        let mut active = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(40));
            let flags = current_modifier_flags();
            let hit = masks.iter().any(|m| *m == flags);
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

/// 读当前修饰键 flags(合成一个 flagsChanged 事件取其 flags; 无辅助功能权限也可读)
fn current_modifier_flags() -> u64 {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let src = CGEventSource::new(CGEventSourceStateID::CombinedSessionState);
    match src {
        Ok(src) => match CGEvent::new(src) {
            Ok(e) => {
                let f = e.get_flags();
                f.bits()
            }
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}
