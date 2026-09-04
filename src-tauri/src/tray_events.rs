// 托盘事件与状态刷新(对齐 Handy tray.rs 的事件面)
use tauri::{Emitter, Manager};

use crate::commands::{open_config_file, open_data_dir};
use crate::settings::{self, Config};
use crate::{AppState, TRAY_TX};
use crate::tray;

pub static TRAY: std::sync::Mutex<Option<tray::MacTray>> = std::sync::Mutex::new(None);

pub fn spawn_tray(app: &tauri::AppHandle) -> Result<(), String> {
    // 调用点均在主线程(setup / run_on_main_thread)
    let mtm = objc2::MainThreadMarker::new().ok_or("spawn_tray 不在主线程")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let cfg: Config = settings::get_config(app.clone()).unwrap_or_default();
    let busy = app
        .state::<std::sync::Mutex<AppState>>()
        .lock()
        .unwrap()
        .session
        .is_some();
    let t = tray::MacTray::new(
        mtm,
        tx.clone(),
        busy,
        &cfg.hotkey.shortcut_str(),
        cfg.use_llm_correction,
        cfg.clipboard_only,
    )?;
    *TRAY_TX.lock().unwrap() = Some(tx);
    *TRAY.lock().unwrap() = Some(t);
    let app2 = app.clone();
    std::thread::spawn(move || {
        while let Ok(tag) = rx.recv() {
            let ev = match tag.as_str() {
                "1" => "toggle_llm",
                "2" => "toggle_clip",
                "3" => "open_data",
                "7" => "open_dict",
                "4" => "open_settings",
                "5" => "quit",
                "6" => "toggle_record",
                _ => "status",
            };
            handle_tray_event(&app2, ev);
        }
    });
    Ok(())
}

pub fn handle_tray_event(app: &tauri::AppHandle, ev: &str) {
    match ev {
        "toggle_llm" => {
            if let Ok(mut c) = settings::get_config(app.clone()) {
                c.use_llm_correction = !c.use_llm_correction;
                let _ = settings::save_config(app.clone(), c);
                let _ = Emitter::emit(app, "config-changed", ());
                let _ = refresh_tray(app);
            }
        }
        "toggle_clip" => {
            if let Ok(mut c) = settings::get_config(app.clone()) {
                c.clipboard_only = !c.clipboard_only;
                let _ = settings::save_config(app.clone(), c);
                let _ = Emitter::emit(app, "config-changed", ());
                let _ = refresh_tray(app);
            }
        }
        "open_data" => {
            let _ = open_data_dir(app.clone());
        }
        "open_dict" => {
            let _ = open_config_file(app.clone());
        }
        "toggle_record" => {
            // 统一入口: 托盘也发协调器信号(与快捷键同源), 不直调 ctrl_*——单一真源
            let st = app.state::<std::sync::Mutex<AppState>>();
            let busy = st.lock().unwrap().session.is_some();
            crate::send_ctrl(!busy);
        }
        "open_settings" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

pub fn refresh_tray(app: &tauri::AppHandle) -> Result<(), String> {
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        *TRAY.lock().unwrap() = None; // 主线程 Drop → removeStatusItem
        let _ = spawn_tray(&app2);
    })
    .map_err(|e| e.to_string())?;
    Ok(())
}
