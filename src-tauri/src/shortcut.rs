//! C54 重构: 键盘快捷键统一模块(照抄 Handy 架构)
//!
//! 引擎 = handy-keys crate(Handy 同款): Manager 线程独占 HotkeyManager,
//! 命令经 mpsc(带同步 response), 事件 try_recv 分发。Fn/纯修饰/精确匹配
//! 全部由 crate 提供——自研 CGEventTap/flags 轮询(key_engine/flags_hotkey)已废弃。
//!
//! 绑定模型: bindings: Map<"transcribe", BindingSet{current: Vec<String>}>
//! 一动作多键; 旧 hotkey/hotkeys 字段自动迁移并入。

use handy_keys::{Hotkey, HotkeyId, HotkeyManager, HotkeyState};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;

// ── Esc 取消热键(录音期间动态注册, global-shortcut 插件; 回调只发信号 C13/C26) ──
use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState as GsState};

use crate::{transcription_coordinator::CoordCmd, CTRL_TX};

pub static ESC_SHORTCUT: Mutex<Option<Shortcut>> = Mutex::new(None);

pub fn register_esc(app: &tauri::AppHandle) {
    if let Ok(esc) = "escape".parse::<Shortcut>() {
        let gs = app.global_shortcut();
        if gs
            .on_shortcut(esc.clone(), |_app, _sc, event| {
                if event.state == GsState::Pressed {
                    let tx = CTRL_TX.lock().unwrap().clone();
                    if let Some(tx) = tx {
                        let _ = tx.send(CoordCmd::Cancel);
                    }
                }
            })
            .is_ok()
        {
            *ESC_SHORTCUT.lock().unwrap() = Some(esc);
        }
    }
}

pub fn unregister_esc(app: &tauri::AppHandle) {
    if let Some(esc) = ESC_SHORTCUT.lock().unwrap().take() {
        let _ = app.global_shortcut().unregister(esc);
    }
}

/// Manager 命令(主线程 → Manager 线程, response 同步等待)
enum ManagerCommand {
    Register {
        hotkey_str: String,
        response: Sender<Result<(), String>>,
    },
    Unregister {
        hotkey_str: String,
        response: Sender<Result<(), String>>,
    },
    UnregisterAll {
        response: Sender<Result<(), String>>,
    },
    Shutdown,
}

pub struct ShortcutState {
    command_sender: Mutex<Sender<ManagerCommand>>,
    thread_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ShortcutState {
    /// 启动 Manager 线程(独占 HotkeyManager; crate 自带事件线程, 我们只做命令+分发)
    pub fn new(send_ctrl: Box<dyn Fn(bool) + Send>) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>();
        let handle = std::thread::spawn(move || Self::manager_thread(cmd_rx, send_ctrl));
        Ok(Self {
            command_sender: Mutex::new(cmd_tx),
            thread_handle: Mutex::new(Some(handle)),
        })
    }

    fn manager_thread(cmd_rx: Receiver<ManagerCommand>, send_ctrl: Box<dyn Fn(bool) + Send>) {
        crate::log::elog("[shortcut] manager 线程启动");
        let manager = match HotkeyManager::new_with_blocking() {
            Ok(m) => m,
            Err(e) => {
                crate::log::elog(&format!("[shortcut] HotkeyManager 创建失败(需辅助功能): {e}"));
                return;
            }
        };
        let mut id_to_str: HashMap<HotkeyId, String> = HashMap::new();

        loop {
            // 事件分发: 触发/释放 → CTRL 信号(单通道, 无双注册)
            while let Some(event) = manager.try_recv() {
                if let Some(hs) = id_to_str.get(&event.id) {
                    let pressed = event.state == HotkeyState::Pressed;
                    crate::log::elog(&format!("[shortcut] 事件: {hs} pressed={pressed}"));
                    send_ctrl(pressed);
                }
            }
            // 命令处理(10ms 超时回事件循环)
            match cmd_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(ManagerCommand::Register { hotkey_str, response }) => {
                    let r = match hotkey_str.parse::<Hotkey>() {
                        Ok(hk) => match manager.register(hk) {
                            Ok(id) => {
                                id_to_str.insert(id, hotkey_str.clone());
                                crate::log::elog(&format!("[shortcut] 注册: {hotkey_str}"));
                                Ok(())
                            }
                            Err(e) => Err(format!("注册失败: {e}")),
                        },
                        Err(e) => Err(format!("解析失败({hotkey_str}): {e}")),
                    };
                    let _ = response.send(r);
                }
                Ok(ManagerCommand::Unregister { hotkey_str, response }) => {
                    let r = id_to_str
                        .iter()
                        .find(|(_, s)| *s == &hotkey_str)
                        .map(|(id, _)| *id)
                        .ok_or_else(|| format!("未注册: {hotkey_str}"))
                        .and_then(|id| {
                            manager.unregister(id).map_err(|e| format!("注销失败: {e}"))
                        });
                    if r.is_ok() {
                        id_to_str.retain(|_, s| s != &hotkey_str);
                        crate::log::elog(&format!("[shortcut] 注销: {hotkey_str}"));
                    }
                    let _ = response.send(r);
                }
                Ok(ManagerCommand::UnregisterAll { response }) => {
                    // crate 无 unregister_all——按映射逐个注销
                    let mut r = Ok(());
                    for (id, _) in id_to_str.iter() {
                        if let Err(e) = manager.unregister(*id) {
                            r = Err(format!("注销失败: {e}"));
                        }
                    }
                    id_to_str.clear();
                    crate::log::elog("[shortcut] 全部注销(suspend)");
                    let _ = response.send(r);
                }
                Ok(ManagerCommand::Shutdown) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        crate::log::elog("[shortcut] manager 线程退出");
    }

    fn send_cmd(&self, cmd: ManagerCommand) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let wrapped = match cmd {
            ManagerCommand::Register { hotkey_str, .. } => ManagerCommand::Register {
                hotkey_str,
                response: tx,
            },
            ManagerCommand::Unregister { hotkey_str, .. } => ManagerCommand::Unregister {
                hotkey_str,
                response: tx,
            },
            ManagerCommand::UnregisterAll { .. } => ManagerCommand::UnregisterAll { response: tx },
            ManagerCommand::Shutdown => ManagerCommand::Shutdown,
        };
        self.command_sender
            .lock()
            .map_err(|_| "锁失败")?
            .send(wrapped)
            .map_err(|_| "manager 线程已退出")?;
        rx.recv().map_err(|_| "未收到响应")?
    }

    pub fn register(&self, hotkey_str: &str) -> Result<(), String> {
        self.send_cmd(ManagerCommand::Register {
            hotkey_str: hotkey_str.to_string(),
            response: mpsc::channel().0, // 占位, send_cmd 内部重包
        })
    }

    pub fn unregister(&self, hotkey_str: &str) -> Result<(), String> {
        self.send_cmd(ManagerCommand::Unregister {
            hotkey_str: hotkey_str.to_string(),
            response: mpsc::channel().0,
        })
    }

    pub fn unregister_all(&self) -> Result<(), String> {
        self.send_cmd(ManagerCommand::UnregisterAll {
            response: mpsc::channel().0,
        })
    }
}

impl Drop for ShortcutState {
    fn drop(&mut self) {
        if let Ok(sender) = self.command_sender.lock() {
            let _ = sender.send(ManagerCommand::Shutdown);
        }
        if let Ok(mut handle) = self.thread_handle.lock() {
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// 初始化: 从配置注册全部 transcribe 绑定(Manager 单通道, 无双注册)
pub fn init_shortcuts(
    app: &tauri::AppHandle,
    send_ctrl: Box<dyn Fn(bool) + Send>,
) -> Result<(), String> {
    let state = ShortcutState::new(send_ctrl)?;
    let cfg = crate::settings::get_config(app.clone()).unwrap_or_default();
    let mut ok_count = 0;
    for hk in cfg.transcribe_bindings() {
        match state.register(&hk) {
            Ok(()) => ok_count += 1,
            Err(e) => crate::log::elog(&format!("[shortcut] init 注册失败({hk}): {e}")),
        }
    }
    crate::log::elog(&format!("[shortcut] init 完成: {ok_count} 个绑定"));
    app.manage(state);
    Ok(())
}

/// 录制期间挂起全部绑定(真注销, 非标志门闩——不存在忘复位=全挂)
pub fn suspend_all(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app
        .state::<ShortcutState>()
        .inner();
    state.unregister_all()
}

/// 录制结束恢复全部绑定(幂等)
pub fn resume_all(app: &tauri::AppHandle) -> Result<(), String> {
    let cfg = crate::settings::get_config(app.clone()).unwrap_or_default();
    let state = app.state::<ShortcutState>().inner();
    for hk in cfg.transcribe_bindings() {
        match state.register(&hk) {
            Ok(()) => {}
            Err(e) => crate::log::elog(&format!("[shortcut] resume 注册失败({hk}): {e}")),
        }
    }
    Ok(())
}
