// 托盘事件与状态刷新(对齐 Handy tray.rs 的事件面)
use tauri::{Emitter, Manager};

use crate::commands::{open_config_file, open_data_dir};
use crate::settings::{self, Config};
use crate::TRAY_TX;
use crate::tray;

pub static TRAY: std::sync::Mutex<Option<tray::MacTray>> = std::sync::Mutex::new(None);

/// 会话状态行文案(core 事实的投影; 图标不参与状态表达——2026-09-18 用户裁决)
fn status_text() -> String {
    if crate::core::session_recording() {
        "状态: 录音中…".to_string()
    } else {
        "状态: 待命".to_string()
    }
}

/// 就地刷新菜单状态行(不重建托盘)。挂在会话生灭的执行点:
/// ctrl_start 尾部 / ctrl_stop / ctrl_abort——从此状态行跟随事实,
/// 不再恒显启动时刻的值(审核 a13/d3 的修复)。
pub fn refresh_status(app: &tauri::AppHandle) {
    let text = status_text();
    let r = app.run_on_main_thread(move || {
        if let Some(t) = TRAY.lock().unwrap().as_ref() {
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                t.set_status_text(mtm, &text);
            }
        }
    });
    if let Err(e) = r {
        crate::log::elog(&format!("[tray] 状态行刷新调度失败: {e}"));
    }
}

pub fn spawn_tray(app: &tauri::AppHandle) -> Result<(), String> {
    // 调用点均在主线程(setup / run_on_main_thread)
    let mtm = objc2::MainThreadMarker::new().ok_or("spawn_tray 不在主线程")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let cfg: Config = settings::get_config(app.clone()).unwrap_or_default();
    let busy = crate::core::session_recording();
    let t = tray::MacTray::new(
        mtm,
        tx.clone(),
        busy,
        &cfg.transcribe_bindings().first().cloned().unwrap_or_default(),
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
                let _ = Emitter::emit_to(app, "main", "config-changed", ());
                let _ = refresh_tray(app);
            }
        }
        "toggle_clip" => {
            if let Ok(mut c) = settings::get_config(app.clone()) {
                c.clipboard_only = !c.clipboard_only;
                let _ = settings::save_config(app.clone(), c);
                let _ = Emitter::emit_to(app, "main", "config-changed", ());
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
            // busy 读 core 事实(转写中 session 为空, 点"录音"=按队列规则开新段, 行为兼容)
            let busy = crate::core::session_recording();
            crate::send_ctrl(!busy);
        }
        "open_settings" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "quit" => {
            crate::log::log(app, "托盘菜单: 退出");
            app.exit(0)
        }
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

// 托盘图标不再随录音/转写切换（恒定一张细线图，历史见 tray.rs 的 MacTray 注释）：
// 原先这里的 set_tray_recording / set_tray_transcribing 两个主线程调度函数已删除。
