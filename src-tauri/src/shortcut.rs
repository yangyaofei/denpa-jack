// 全局快捷键注册(对齐 Handy shortcut/): Esc 动态注册/注销; 回调只发信号(C13/C26)
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::{transcription_coordinator::CoordCmd, CTRL_TX};

pub static ESC_SHORTCUT: std::sync::Mutex<Option<Shortcut>> = std::sync::Mutex::new(None);

pub fn register_esc(app: &tauri::AppHandle) {
    if let Ok(esc) = "escape".parse::<Shortcut>() {
        let gs = app.global_shortcut();
        // 专属 handler: 只发信号(C13)
        if gs.on_shortcut(esc.clone(), |_app, _sc, event| {
            if event.state == ShortcutState::Pressed {
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

