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
    /// C57: 用 new()(观察者)而非 new_with_blocking()(拦截者)——blocking 会吞命中热键的
    /// 事件, 实测(kbd_probe 对照)在特定时序下导致系统键盘状态跟踪错乱 → 抬起事件偶发缺失
    /// → 纯修饰组合表现为持续按住。我们不需要拦截(热键放行无害), 观察者事件流完整。
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
        let manager = match HotkeyManager::new() {
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

// ── C55 后端组合录制(用户定则): 点一下→后端 capture→轮询读更新→全抬 300ms 定稿 ──
// 引擎= handy-keys KeyboardListener(全键可见, 含 Fn/纯修饰/左右修饰);
// 通道=轮询(capture_poll); 清理= capture_end 无条件 resume(幂等)——无门闩无永挂。

#[derive(Clone, Default, serde::Serialize)]
pub struct CaptureSnapshot {
    pub active: bool,
    /// 当前按住的组合(如 "ctrl+cmd+space")
    pub current: String,
    /// 定稿组合(全部抬起后出现一次, 前端展示后走 capture_end 提交/取消)
    pub final_combo: Option<String>,
    pub error: Option<String>,
}

struct CaptureState {
    snapshot: CaptureSnapshot,
    stop: Option<std::sync::mpsc::Sender<()>>,
}

static CAPTURE: Mutex<Option<CaptureState>> = Mutex::new(None);

/// 开始录制: suspend 全部绑定 + KeyboardListener 收键(独立线程, 10ms 事件循环)
pub fn capture_begin(app: &tauri::AppHandle) -> Result<(), String> {
    let mut g = CAPTURE.lock().unwrap();
    if let Some(st) = g.as_ref() {
        if st.snapshot.active {
            return Ok(()); // 已在录制(幂等)
        }
    }
    let listener = handy_keys::KeyboardListener::new().map_err(|e| format!("监听器创建失败(需辅助功能): {e}"))?;
    suspend_all(app)?;
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    *g = Some(CaptureState {
        snapshot: CaptureSnapshot { active: true, ..Default::default() },
        stop: Some(stop_tx),
    });
    drop(g);

    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut all_up_since: Option<std::time::Instant> = None;
        let mut last_current = String::new();
        loop {
            // 停止信号(capture_end)
            if stop_rx.try_recv().is_ok() {
                break;
            }
            // 收事件: key_down 更新当前组合+清零窗口; key_up 且无剩余按下 → 起 300ms 窗口
            while let Some(ev) = listener.try_recv() {
                if ev.is_key_down {
                    let combo = ev
                        .as_hotkey()
                        .map(|h| h.to_handy_string())
                        .unwrap_or_default();
                    if !combo.is_empty() {
                        last_current = combo.to_ascii_lowercase();
                    }
                    all_up_since = None;
                } else if ev.modifiers.is_empty() && ev.key.is_none() {
                    all_up_since = Some(std::time::Instant::now());
                }
            }
            let now = std::time::Instant::now();
            let mut g = CAPTURE.lock().unwrap();
            if let Some(st) = g.as_mut() {
                // 全部抬起持续 300ms → 定稿(线程退出, 快照留 final 供前端读)
                if !last_current.is_empty() {
                    if let Some(since) = all_up_since {
                        if now.duration_since(since).as_millis() >= 300 {
                            st.snapshot.current = String::new();
                            st.snapshot.final_combo = Some(last_current.clone());
                            break;
                        }
                    }
                }
                st.snapshot.current = last_current.clone();
                // 超时兜底: 120s 未定稿自动取消
                if started.elapsed().as_secs() > 120 {
                    st.snapshot.active = false;
                    st.snapshot.final_combo = None;
                    st.snapshot.error = Some("录制超时已取消".into());
                    break;
                }
            }
            drop(g);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        crate::log::elog("[shortcut] capture 线程退出");
    });
    crate::log::elog("[shortcut] capture 开始");
    Ok(())
}

/// 轮询读快照(前端 200ms 拉一次)
pub fn capture_poll() -> CaptureSnapshot {
    CAPTURE
        .lock()
        .unwrap()
        .as_ref()
        .map(|st| st.snapshot.clone())
        .unwrap_or_default()
}

/// 结束录制: 无条件 resume; confirm=true 时把 combo 写入 bindings
pub fn capture_end(app: &tauri::AppHandle, confirm: bool, combo: Option<String>) -> Result<(), String> {
    {
        let mut g = CAPTURE.lock().unwrap();
        if let Some(st) = g.as_mut() {
            st.snapshot.active = false;
            if let Some(tx) = st.stop.take() {
                let _ = tx.send(());
            }
        }
        *g = None;
    }
    if confirm {
        if let Some(combo) = combo {
            let combo = crate::settings::normalize_combo(&combo);
            if combo.is_empty() {
                return Err("组合为空".into());
            }
            combo
                .parse::<Hotkey>()
                .map_err(|e| format!("组合无法解析({combo}): {e}"))?;
            let mut cfg = crate::settings::get_config(app.clone())?;
            let set = cfg
                .bindings
                .entry("transcribe".into())
                .or_insert(crate::settings::BindingSet { current: vec![] });
            if !set.current.contains(&combo) {
                set.current.push(combo);
            }
            crate::settings::save_config(app.clone(), cfg)?;
        }
    }
    resume_all(app)
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
