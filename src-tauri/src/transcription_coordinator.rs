// 按键→录音的极简映射(用户定则, 替换 Handy 状态机):
//   Pressed  → Idle 时开始录音
//   Released → 录音中则停止并转写
//   Cancel(Esc) → 录音中则丢弃
//   转写中的下一次按下: 直接允许开新录音(交付各自独立, 不互相阻塞)
// 线程模型不变: 快捷键回调只发信号, 本线程执行效果(C13/C26)
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

pub enum CoordCmd {
    Input { pressed: bool, activation: String },
    Cancel,
    /// Start 效果的真实结果(失败时协调器复位, 乐观转移对账)
    StartResult { started: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Start,
    Stop,
    Abort,
}

pub fn spawn(
    rx: Receiver<CoordCmd>,
    effect_tx: Sender<Effect>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut recording = false;
        while let Ok(cmd) = rx.recv() {
            let effect = match cmd {
                CoordCmd::Input { pressed, activation } => {
                    if activation == "toggle" {
                        // 短按切换: Pressed=开/停翻转; Released 不消费(纯修饰组合天然抗 stuck)
                        if pressed {
                            if recording {
                                recording = false;
                                Some(Effect::Stop)
                            } else {
                                recording = true;
                                Some(Effect::Start)
                            }
                        } else {
                            None
                        }
                    } else {
                        // 按住说话(默认): 按下=开始, 松开=结束
                        if pressed {
                            if recording {
                                None
                            } else {
                                recording = true;
                                Some(Effect::Start)
                            }
                        } else if recording {
                            recording = false;
                            Some(Effect::Stop)
                        } else {
                            None
                        }
                    }
                }
                CoordCmd::Cancel => {
                    if recording {
                        recording = false;
                        Some(Effect::Abort)
                    } else {
                        None
                    }
                }
                CoordCmd::StartResult { started } => {
                    // Start 失败: 复位录音态(下次按键可重试), 不产生新效果
                    if !started && recording {
                        recording = false;
                    }
                    None
                }
            };
            if let Some(e) = effect {
                crate::log::elog(&format!("[coord] {e:?}"));
                if effect_tx.send(e).is_err() {
                    return;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn drive(cmds: Vec<CoordCmd>) -> Vec<Effect> {
        let (ctx, crx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        spawn(crx, etx);
        for c in cmds {
            ctx.send(c).unwrap();
        }
        // 给协调线程时间处理
        std::thread::sleep(std::time::Duration::from_millis(120));
        let mut out = vec![];
        while let Ok(e) = erx.try_recv() {
            out.push(e);
        }
        out
    }

    #[test]
    fn idle_press_starts() {
        assert_eq!(drive(vec![CoordCmd::Input { pressed: true, activation: "hold".into() }]), vec![Effect::Start]);
    }

    #[test]
    fn toggle_press_starts_release_ignored() {
        // 短按切换: 按下开始, 松开不结束
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "toggle".into() },
                CoordCmd::Input { pressed: false, activation: "toggle".into() },
            ]),
            vec![Effect::Start]
        );
    }
    #[test]
    fn toggle_double_press_full_cycle() {
        // 短按开始 → 再短按结束转写 → 可重新开始
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "toggle".into() },
                CoordCmd::Input { pressed: false, activation: "toggle".into() },
                CoordCmd::Input { pressed: true, activation: "toggle".into() },
                CoordCmd::Input { pressed: false, activation: "toggle".into() },
                CoordCmd::Input { pressed: true, activation: "toggle".into() },
            ]),
            vec![Effect::Start, Effect::Stop, Effect::Start]
        );
    }
    #[test]
    fn toggle_cancel_aborts() {
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "toggle".into() },
                CoordCmd::Cancel,
            ]),
            vec![Effect::Start, Effect::Abort]
        );
    }
    #[test]
    fn recording_release_stops() {
        assert_eq!(
            drive(vec![CoordCmd::Input { pressed: true, activation: "hold".into() }, CoordCmd::Input { pressed: false, activation: "hold".into() }]),
            vec![Effect::Start, Effect::Stop]
        );
    }
    #[test]
    fn press_while_recording_ignored() {
        // 按住期间 auto-repeat/重复按不产生新 Start
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "hold".into() },
                CoordCmd::Input { pressed: true, activation: "hold".into() },
                CoordCmd::Input { pressed: true, activation: "hold".into() },
            ]),
            vec![Effect::Start]
        );
    }
    #[test]
    fn release_without_recording_noop() {
        assert_eq!(drive(vec![CoordCmd::Input { pressed: false, activation: "hold".into() }]), vec![]);
    }
    #[test]
    fn double_stop_single() {
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "hold".into() },
                CoordCmd::Input { pressed: false, activation: "hold".into() },
                CoordCmd::Input { pressed: false, activation: "hold".into() },
            ]),
            vec![Effect::Start, Effect::Stop]
        );
    }
    #[test]
    fn cancel_aborts_recording() {
        assert_eq!(
            drive(vec![CoordCmd::Input { pressed: true, activation: "hold".into() }, CoordCmd::Cancel]),
            vec![Effect::Start, Effect::Abort]
        );
    }
    #[test]
    fn cancel_when_idle_noop() {
        assert_eq!(drive(vec![CoordCmd::Cancel]), vec![]);
    }
    #[test]
    fn full_cycle_restart() {
        assert_eq!(
            drive(vec![
                CoordCmd::Input { pressed: true, activation: "hold".into() },
                CoordCmd::Input { pressed: false, activation: "hold".into() },
                CoordCmd::Input { pressed: true, activation: "hold".into() },
                CoordCmd::Input { pressed: false, activation: "hold".into() },
            ]),
            vec![Effect::Start, Effect::Stop, Effect::Start, Effect::Stop]
        );
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    // 与 spawn 循环相同的极简规则(按下=Start 仅 Idle, 松开=Stop 仅录音中)
    fn drive(cmds: &[bool]) -> Vec<Effect> {
        let mut recording = false;
        let mut out = Vec::new();
        for pressed in cmds {
            let e = if *pressed {
                if recording { None } else { recording = true; Some(Effect::Start) }
            } else if recording {
                recording = false;
                Some(Effect::Stop)
            } else { None };
            if let Some(e) = e { out.push(e); }
        }
        out
    }

    #[test]
    fn double_press_single_start() {
        assert_eq!(drive(&[true, true]), vec![Effect::Start]);
    }

    #[test]
    fn press_release_press_release_full_cycle() {
        assert_eq!(drive(&[true, false, true, false]), vec![Effect::Start, Effect::Stop, Effect::Start, Effect::Stop]);
    }

    #[test]
    fn rapid_flap_no_duplicate() {
        assert_eq!(drive(&[true, false, true, false, true, true, false]), vec![Effect::Start, Effect::Stop, Effect::Start, Effect::Stop, Effect::Start, Effect::Stop]);
    }

    #[test]
    fn release_first_noop() {
        assert_eq!(drive(&[false, false]), Vec::<Effect>::new());
    }
}
