pub mod audio;
mod audio_feedback;
mod autotest;
mod commands;
mod deliver;
pub mod dict;
pub mod doubao;
mod engines;
mod history;
pub mod llm;
mod log;
mod openai_realtime;
mod overlay;
mod paste_tx;
mod pipeline;
mod recording;
pub mod settings;
mod shortcut;
#[allow(unused_unsafe)]
mod tray;
mod tray_events;
mod transcription_coordinator;
mod zhipu_file;


use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub struct AppState {
    pub session: Option<crate::recording::RecordingSession>,
    /// 最近一次已保存录音(hud 重试用)——会话产物, 归 AppState 而非全局
    pub last_audio: Option<String>,
    /// 当前引擎命令镜像(转写中 abort / 测试通道用)——会话产物
    pub engine_mirror: Option<tokio::sync::mpsc::Sender<crate::doubao::Cmd>>,
}
impl Default for AppState {
    fn default() -> Self {
        Self { session: None, last_audio: None, engine_mirror: None }
    }
}

pub(crate) static TRAY_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<String>>> = std::sync::Mutex::new(None);
/// 统一入口: 全部录音触发(快捷键/托盘/前端)都发协调器信号——单一真源
pub fn send_ctrl(pressed: bool) {
    let _ = CTRL_TX.lock().unwrap().as_ref()
        .map(|tx| tx.send(transcription_coordinator::CoordCmd::Input { pressed }));
}

pub fn send_cancel() {
    let _ = CTRL_TX.lock().unwrap().as_ref()
        .map(|tx| tx.send(transcription_coordinator::CoordCmd::Cancel));
}

pub(crate) static CTRL_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<transcription_coordinator::CoordCmd>>> = std::sync::Mutex::new(None);

pub fn run() {
    // panic 落盘(打包版 stderr 不可见, 线程静默死亡=日志戛然而止的元凶)
    std::panic::set_hook(Box::new(|info| {
        crate::log::elog(&format!("[panic] {info}"));
    }));
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 二次启动: 唤出设置窗
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            log::log(app.handle(), "setup begin");
            app.set_activation_policy(tauri::ActivationPolicy::Accessory); // 菜单栏常驻, 不占 Dock
            tray_events::spawn_tray(app.handle()).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // 权限引导: 弹系统授权窗 + 提示
            let h = app.handle().clone();
            std::thread::spawn(move || {
                if !deliver::ax_trusted(true) {
                    use tauri::Emitter;
                    let _ = Emitter::emit(&h, "permission-ax", false);
                }
            });
            // hud 浮窗
            let _hud = tauri::WebviewWindowBuilder::new(
                app,
                "hud",
                tauri::WebviewUrl::App("hud.html".into()),
            )
            .title("hud")
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .transparent(true)
            .shadow(false)
            .inner_size(480.0, 200.0)
            .build()?;
            overlay::position_hud_at_cursor(&app.handle().clone());
            // 主快捷键
            let cfg = settings::get_config(app.handle().clone()).unwrap_or_default();
            let s: Shortcut = match cfg.hotkey.shortcut_str().parse() {
                Ok(x) => x,
                Err(_) => "f5".parse().expect("f5 解析失败"),
            };
            log::log(app.handle(), &format!("setup: hotkey={}", cfg.hotkey.shortcut_str()));
            // C13(修正版): 全局快捷键回调在主线程执行, 回调内禁止任何需主线程同步的操作
            // → 只发信号到控制线程, 由它执行 start/stop/abort
            // 协调器线程(Handy 状态机) + 效果执行线程
            let (ctx, crx) = std::sync::mpsc::channel::<transcription_coordinator::CoordCmd>();
            let (fx_tx, fx_rx) = std::sync::mpsc::channel::<transcription_coordinator::Effect>();
            *CTRL_TX.lock().unwrap() = Some(ctx);
            transcription_coordinator::spawn(crx, fx_tx);
            let app_ctrl = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(effect) = fx_rx.recv() {
                    use transcription_coordinator::Effect;
                    let st = app_ctrl.state::<std::sync::Mutex<AppState>>();
                    let r = match effect {
                        Effect::Start => recording::ctrl_start(app_ctrl.clone(), &st),
                        Effect::Stop => recording::ctrl_stop(app_ctrl.clone(), &st),
                        Effect::Abort => recording::ctrl_abort(&app_ctrl, &st),
                    };
                    if let Err(e) = &r {
                        log::log(&app_ctrl, &format!("effect {effect:?} 失败: {e}"));
                    }
                    // Start 结果回执: 失败(麦克风被拒/无Key)时协调器复位——乐观转移对账(Handy on_start_result)
                    if matches!(effect, Effect::Start) {
                        let started = matches!(&r, Ok(()));
                        let _ = CTRL_TX.lock().unwrap().as_ref()
                            .map(|tx| tx.send(transcription_coordinator::CoordCmd::StartResult { started }));
                    }
                }
            });
            let app_hk = app.handle().clone();
            if let Err(e) = app.global_shortcut()
                .on_shortcut(s, move |_app, _sc, event| {
                    log::log(&app_hk, &format!("hotkey event: {:?}", event.state));
                    send_ctrl(event.state == ShortcutState::Pressed);
                }) {
                log::log(app.handle(), &format!("快捷键注册失败: {e}"));
            } else {
                log::log(app.handle(), "快捷键注册成功");
            }
            log::log(app.handle(), "setup done");

            autotest::maybe_spawn(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗=隐藏(菜单栏常驻); 真正退出走托盘菜单
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .manage(std::sync::Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::ping, commands::list_mics, commands::get_active_mic, commands::list_llm_models, commands::llm_selftest, commands::get_default_prompt, commands::ui_log, commands::get_history,
            commands::clear_history, commands::copy_text, commands::get_hotwords,
            recording::recording_start, recording::recording_stop,
            recording::recording_abort, commands::hud_hide, commands::hud_resize, commands::dev_nav,
            commands::open_config_file, commands::open_data_dir,
            commands::rerun_history, commands::retry_last, commands::reapply_hotkey,
            commands::check_permissions,
            commands::autostart_enable, commands::autostart_disable, commands::autostart_status,
            settings::get_config, settings::save_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}


