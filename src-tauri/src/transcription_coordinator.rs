// 按键→录音的极简映射(用户定则, 替换 Handy 状态机):
//   Pressed  → Idle 时开始录音
//   Released → 录音中则停止并转写
//   Cancel(Esc) → 录音中则丢弃
//   转写中的下一次按下: 直接允许开新录音(交付各自独立, 不互相阻塞)
// 线程模型不变: 快捷键回调只发信号, 本线程执行效果(C13/C26)
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

pub enum CoordCmd {
    Input { pressed: bool },
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
                CoordCmd::Input { pressed } => {
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
