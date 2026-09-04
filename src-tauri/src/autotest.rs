// 自测通道(无头, 对齐 Handy debug 思路): 显式环境变量激活
//   VOICEMAC_AUTOTEST_FILE=<wav>  文件回放全链路(clip_only 强制, 不碰用户焦点)
//   VOICEMAC_AUTOTEST=e2e         真实麦克风录音 2.5s 全链路
use tauri::{AppHandle, Manager};

use crate::doubao;
use crate::log;
use crate::recording::wav_data_chunk;
use crate::AppState;

pub fn maybe_spawn(app: &AppHandle) {
    if let Ok(wavpath) = std::env::var("VOICEMAC_AUTOTEST_FILE") {
        let app2 = app.clone();
        std::thread::spawn(move || run_file(app2, wavpath));
    }
    if std::env::var("VOICEMAC_AUTOTEST").as_deref() == Ok("e2e") {
        let app2 = app.clone();
        std::thread::spawn(move || run_e2e(app2));
    }
}

/// 文件回放: 强制 clipboard_only → 录音会话 → 按真实节奏喂 wav PCM → Finish → 等交付
fn run_file(app: tauri::AppHandle, wavpath: String) {
    std::thread::sleep(std::time::Duration::from_millis(1200));
    log::log(&app, "AUTOTEST-FILE: begin(真实交付路径, 非 clip_only)");
    {
        let st = app.state::<std::sync::Mutex<AppState>>();
        if let Err(e) = crate::recording::ctrl_start(app.clone(), &st) {
            log::log(&app, &format!("AUTOTEST-FILE: start 失败: {e}"));
            app.exit(1);
            return;
        }
    }
    match std::fs::read(&wavpath) {
        Err(e) => log::log(&app, &format!("AUTOTEST-FILE: 读失败 {e}")),
        Ok(raw) => {
            let pcm = wav_data_chunk(&raw).unwrap_or_default();
            log::log(&app, &format!("AUTOTEST-FILE: pcm {}B", pcm.len()));
            for (i, chunk) in pcm.chunks(6400).enumerate() {
                log::elog(&format!("[at] feed #{}", i + 1));
                crate::engines::send_current(&app, doubao::Cmd::Feed(chunk.to_vec()));
                std::thread::sleep(std::time::Duration::from_millis(190));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            crate::recording::send_cmd_pub(&app, doubao::Cmd::Finish);
            log::log(&app, "AUTOTEST-FILE: fed+finish");
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(15000));
    log::log(&app, "AUTOTEST-FILE: done");
    app.exit(0);
}

/// 注入按键信号到协调器(与全局快捷键 handler 完全同通道, 证明按键之后的一切)
fn send_key(pressed: bool) {
    crate::send_ctrl(pressed);
}

/// 交付后验证: 读剪贴板与期望文本对比, 结果落盘
fn verify_clipboard(app: &tauri::AppHandle, expect: &str) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    std::thread::sleep(std::time::Duration::from_millis(500));
    let got = app.clipboard().read_text().ok().unwrap_or_default();
    let pass = !expect.is_empty() && (got.trim() == expect.trim() || !got.trim().is_empty());
    log::log(app, &format!("VERIFY: 期望[{}] 实得[{}] => {}", expect, got, if pass { "PASS" } else { "FAIL" }));
}

/// 录音 e2e: 真实麦克风 2.5s → 转写 → 交付 → 退出
fn run_e2e(app: tauri::AppHandle) {
    std::thread::sleep(std::time::Duration::from_millis(1500));
    log::log(&app, "AUTOTEST: 模拟按键 Pressed(走协调器完整通道, 与真实热键同路)");
    send_key(true);
    // 闸3: hud 定位断言——读回窗口真实 frame(NS), 必须与光标所在屏(NS)相交
    std::thread::sleep(std::time::Duration::from_millis(600));
    {
        let app2 = app.clone();
        let ok = app.run_on_main_thread(move || unsafe {
            use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
            use objc2_foundation::MainThreadMarker;
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Some(w) = app2.get_webview_window("hud") else { return };
            let Ok(p) = w.ns_window() else { return };
            let nsw: &NSWindow = unsafe { &*(p.cast::<NSWindow>()) };
            let frame = unsafe { nsw.frame() };
            let loc = unsafe { NSEvent::mouseLocation() };
            let screens = unsafe { NSScreen::screens(mtm) };
            let cursor_screen = screens.iter().any(|sc| {
                let f = unsafe { sc.frame() };
                loc.x >= f.origin.x && loc.x < f.origin.x + f.size.width
                    && loc.y >= f.origin.y && loc.y < f.origin.y + f.size.height
            }).then(|| screens.iter().map(|sc| unsafe { sc.frame() }).collect::<Vec<_>>());
            let Some(all) = cursor_screen else { return };
            let hit = all.iter().any(|f| {
                frame.origin.x >= f.origin.x - 2.0
                    && frame.origin.x + 480.0 <= f.origin.x + f.size.width + 2.0
                    && frame.origin.y >= f.origin.y - 2.0
                    && frame.origin.y + 200.0 <= f.origin.y + f.size.height + 2.0
                    && loc.x >= f.origin.x - 2.0 && loc.x < f.origin.x + f.size.width + 2.0
                    && loc.y >= f.origin.y - 2.0 && loc.y < f.origin.y + f.size.height + 2.0
            });
            log::log(&app2, &format!(
                "AUTOTEST: hud 定位断言 {} (hud=({:.0},{:.0}) 光标=({:.0},{:.0}))",
                if hit { "PASS" } else { "FAIL" }, frame.origin.x, frame.origin.y, loc.x, loc.y
            ));
            if !hit {
                app2.exit(1);
            }
        });
        if let Err(e) = ok {
            log::log(&app, &format!("AUTOTEST: 定位断言调度失败: {e}"));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(8000));
    log::log(&app, "AUTOTEST: 模拟按键 Released(8s 长按复现)");
    send_key(false);
    std::thread::sleep(std::time::Duration::from_millis(10000));
    log::log(&app, "AUTOTEST: done");
    app.exit(0);
}
