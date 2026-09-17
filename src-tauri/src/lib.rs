pub mod audio;
mod audio_feedback;
mod autotest;
mod commands;
pub mod crash_log;
mod deliver;
pub mod dict;
pub mod doubao;
mod engines;
mod history;
pub mod llm;
mod log;
mod mic_watch;
mod openai_realtime;
mod overlay;
mod paste_tx;
mod permissions;
mod pipeline;
mod recording;
pub mod screen_context;
mod segment_queue;
mod update;
pub mod settings;
mod shortcut;
#[allow(unused_unsafe)]
mod tray;
mod tray_events;
mod transcription_coordinator;
mod zhipu_file;


use tauri::Manager;
// global-shortcut 插件仅剩 Esc 动态注册使用(见 shortcut.rs), 触发已全走 handy-keys 引擎

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
/// 触发模式全局缓存(save_config 同步更新; 引擎回调无 app handle 也能读; 空=hold)
pub(crate) static ACTIVATION: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// 统一入口: 全部录音触发(快捷键/托盘/前端)都发协调器信号——单一真源
pub fn send_ctrl(pressed: bool) {
    let activation = crate::ACTIVATION.lock().unwrap().clone();
    let _ = CTRL_TX.lock().unwrap().as_ref()
        .map(|tx| tx.send(transcription_coordinator::CoordCmd::Input { pressed, activation }));
}

pub fn send_cancel() {
    let _ = CTRL_TX.lock().unwrap().as_ref()
        .map(|tx| tx.send(transcription_coordinator::CoordCmd::Cancel));
}

/// C43: asr-final 类事件双发(main 历史页 + hud 浮窗), 替代全局广播
/// C43b: Tauri 2 事件通道(Rust→webview)在本机打包版实测不可达(Any/AnyLabel 均不达, emit 返回 Ok)。
/// HUD 改为前端 150ms 轮询拉取(invoke 通道已证可靠)。写入点: 各层在原 emit 处同步写快照。
#[derive(Default, Clone, serde::Serialize)]
pub struct HudSnapshot {
    pub version: u64,
    pub status: String,   // recording | transcribing | busy | "" 
    pub partial: String,
    pub level: u32,
    pub msg: String,      // 一次性提示(显示后清)
    pub finished: Option<serde_json::Value>, // asr-final 载荷(一次性, 取后清)
    pub err: Option<String>,
    /// 左栏: 当前段(录音中=实时文字; 否则=正在转写的那段)——分段队列定则, 见 segment_queue.rs
    pub seg_current: Option<crate::segment_queue::SegView>,
    /// 右栏: 队列里其余段(空 = 右栏不显示, 浮窗保持单栏宽度)
    pub seg_queue: Vec<crate::segment_queue::SegView>,
    /// 是否有未处理的失败段(浮窗存活规则: 只剩失败时 2.5s 后收起)
    pub seg_failed: bool,
}

pub static HUD: std::sync::Mutex<HudSnapshot> = std::sync::Mutex::new(HudSnapshot {
    version: 0, status: String::new(), partial: String::new(), level: 0,
    msg: String::new(), finished: None, err: None,
    seg_current: None, seg_queue: Vec::new(), seg_failed: false,
});

pub fn hud_set(f: impl FnOnce(&mut HudSnapshot)) {
    let mut g = HUD.lock().unwrap();
    f(&mut g);
    g.version += 1;
}

pub fn emit_both(app: &tauri::AppHandle, event: &str, payload: serde_json::Value) {
    use tauri::Emitter;
    let _ = Emitter::emit_to(app, "main", event, payload.clone());
    let _ = Emitter::emit_to(app, "hud", event, payload);
}

pub(crate) static CTRL_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<transcription_coordinator::CoordCmd>>> = std::sync::Mutex::new(None);

pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    // 自测模式跳过单实例插件：单实例的判重基于应用名（不是 bundle id），
    // 会让"独立 bundle id 的测试副本"在正式实例运行时被转发参数后立刻退出，导致自测无法进行。
    // （见 docs/SPEC.md 自测通道一节：VOICEMAC_AUTOTEST=update 用副本验证更新链路）
    let builder = {
        let autotest = std::env::var("VOICEMAC_AUTOTEST").is_ok();
        if autotest {
            builder
        } else {
            builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                // 二次启动: 唤出主窗
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }))
        }
    };
    builder
        .plugin(tauri_plugin_clipboard_manager::init())
        // 内置更新：自己下载并替换自身，绕开浏览器下载带来的 quarantine（见 docs/SPEC.md「应用内更新」）
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            log::log(app.handle(), "setup begin");
            app.set_activation_policy(tauri::ActivationPolicy::Accessory); // 菜单栏常驻, 不占 Dock
            // 预热数据目录缓存（history/recordings/llm_logs/app.log 都从它派生）
            log::log(app.handle(), &format!("数据目录: {}", crate::settings::data_dir(app.handle()).display()));
            // 死亡诊断: 会话文件(判定上次是否正常运行结束) + 崩溃报告扫描 + 心跳
            crate::crash_log::begin_session(&crate::settings::data_dir(app.handle()));
            crate::crash_log::spawn_heartbeat();
            if let Err(e) = mic_watch::start() {
                log::log(app.handle(), &format!("mic_watch 注册失败: {e}"));
            }
            tray_events::spawn_tray(app.handle()).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // 分段转写队列 worker: 松开热键后的后处理全部进队列串行执行
            // (规则与理由见 segment_queue.rs 顶部注释: 顺序保证 + 不丢段 + 失败不阻塞)
            segment_queue::start(app.handle().clone());
            // 权限总检查: 一次性查清全部必需权限(麦克风 + 辅助功能)
            // 缺项 → 落日志 + 推给前端 + 把主窗拉出来, 让用户第一眼就看到"缺什么、会怎样"
            let h = app.handle().clone();
            std::thread::spawn(move || {
                // 自测模式（VOICEMAC_AUTOTEST=*）不弹任何系统授权框：
                // 测试常常在非交互环境下跑（也可能用独立 bundle id 的副本），弹窗只会留下无人处理的气泡
                let autotest = std::env::var("VOICEMAC_AUTOTEST").is_ok();
                // 麦克风未决定 → 弹系统授权框; 已拒绝不会再弹(需去系统设置)
                if !autotest && crate::permissions::mic_status_code() == 0 {
                    crate::permissions::request_mic();
                    std::thread::sleep(std::time::Duration::from_millis(800));
                }
                // 辅助功能: 带引导提示的检查(AXIsProcessTrustedWithOptions(prompt))
                let ax = deliver::ax_trusted(!autotest);
                let missing = crate::permissions::missing();
                if missing.is_empty() {
                    log::log(&h, "[perm] 权限齐备: 麦克风 + 辅助功能");
                } else {
                    log::log(
                        &h,
                        &format!(
                            "[perm] 缺失权限 {:?} (麦克风={}, 辅助功能={})",
                            missing,
                            crate::permissions::mic_status_text(),
                            ax
                        ),
                    );
                    use tauri::Emitter;
                    let _ = Emitter::emit_to(&h, "main", "permissions", &crate::permissions::all());
                    crate::hud_set(|s| s.msg = format!("缺少权限: {}", missing.join("、")));
                    if let Some(w) = h.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
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
            // C55: 快捷键注册全部由 shortcut::init_shortcuts 统一完成(bindings 单一真相)
            // C13(修正版): 引擎回调只发信号到控制线程, 由它执行 start/stop/abort
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
            // C51 多热键: 全部注册, 任一触发
            // C54 统一引擎(Handy 同构): handy-keys crate 单通道注册+触发, 无双注册
            shortcut::init_shortcuts(&app.handle().clone(), Box::new(send_ctrl))
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // 触发模式缓存初始化(hold/toggle)
            {
                let cfg0 = settings::get_config(app.handle().clone()).unwrap_or_default();
                *crate::ACTIVATION.lock().unwrap() = cfg0.activation;
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
            recording::recording_abort, commands::hud_hide, commands::hud_poll, commands::poll_versions, commands::hud_resize, commands::dev_nav, commands::app_version,
            commands::queue_retry, commands::queue_clear, commands::queue_dismiss,
            update::update_check, update::update_install,
            commands::open_config_file, commands::open_data_dir,
            commands::rerun_history, commands::retry_last, commands::open_settings_window, commands::reapply_hotkey,
            commands::add_binding, commands::remove_binding, commands::suspend_all_bindings, commands::resume_all_bindings,
            commands::capture_begin, commands::capture_poll, commands::capture_end,
            commands::check_permissions, commands::request_microphone, commands::open_system_settings,
            commands::request_screen_permission, commands::open_screen_context_dir,
            commands::autostart_enable, commands::autostart_disable, commands::autostart_status,
            settings::get_config, settings::save_config,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // 退出路径留痕: 没有这行日志时, "应用消失" 就分不清是正常退出还是崩溃
            tauri::RunEvent::ExitRequested { code, .. } => {
                log::log(app, &format!("ExitRequested code={code:?}"));
            }
            tauri::RunEvent::Exit => {
                crate::crash_log::end_session(&crate::settings::data_dir(app));
            }
            _ => {}
        });
}


