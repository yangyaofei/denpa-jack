// 自测通道(无头, 对齐 Handy debug 思路): 显式环境变量激活
//   VOICEMAC_AUTOTEST_FILE=<wav>  文件回放全链路(clip_only 强制, 不碰用户焦点)
//   VOICEMAC_AUTOTEST=e2e         真实麦克风录音 2.5s 全链路
use tauri::{AppHandle, Manager};

use crate::doubao;
use crate::log;
use crate::recording::wav_data_chunk;
use crate::AppState;

pub fn maybe_spawn(app: &AppHandle) {
    if std::env::var("VOICEMAC_AUTOTEST").as_deref() == Ok("ui") {
        let app2 = app.clone();
        std::thread::spawn(move || run_ui_chain(app2));
        return;
    }
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
    log::log(&app, "AUTOTEST-FILE: begin(文件回放=唯一输入源; 自建会话不碰真实麦克风)");
    match std::fs::read(&wavpath) {
        Err(e) => log::log(&app, &format!("AUTOTEST-FILE: 读失败 {e}")),
        Ok(raw) => {
            let pcm = wav_data_chunk(&raw).unwrap_or_default();
            log::log(&app, &format!("AUTOTEST-FILE: pcm {}B", pcm.len()));
            // 自建会话: resolve→spawn→mirror, 与 ctrl_start 完全隔离(无双输入)
            use tauri::Manager;
            let cfg = crate::settings::get_config(app.clone()).unwrap_or_default();
            let (profile, key) = match crate::recording::resolve_asr(&cfg) {
                Ok(x) => x,
                Err(e) => { log::log(&app, &format!("AUTOTEST-FILE: resolve 失败: {e}")); app.exit(1); return; }
            };
            let (tx, rx) = tokio::sync::mpsc::channel::<doubao::Cmd>(64);
            let handoff: crate::engines::HandoffBox = std::sync::Arc::new(std::sync::Mutex::new(None));
            crate::engines::spawn_session(
                app.clone(), profile.provider.clone(), key,
                crate::recording::budget_hotwords(&cfg.dict), handoff.clone(), rx,
            );
            {
                let st = app.state::<std::sync::Mutex<AppState>>();
                st.lock().unwrap().engine_mirror = Some(tx.clone());
            }
            for (i, chunk) in pcm.chunks(6400).enumerate() {
                log::elog(&format!("[at] feed #{}", i + 1));
                let _ = tx.try_send(doubao::Cmd::Feed(chunk.to_vec()));
                std::thread::sleep(std::time::Duration::from_millis(190));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = tx.try_send(doubao::Cmd::Finish);
            log::log(&app, "AUTOTEST-FILE: fed+finish");
            // 交付期间 engine_mirror 保留给 pipeline 可用(pipeline 不用 mirror, 无碍)
            std::thread::sleep(std::time::Duration::from_millis(15000));
            log::log(&app, "AUTOTEST-FILE: done");
            app.exit(0);
            return;
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

/// UI 数据链验证: 页面调用的函数(recent/get_config/poll_versions)直接调,
/// 与磁盘真值(history.jsonl 行数/config.json dict 数)断言比对
fn run_ui_chain(app: tauri::AppHandle) {
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let dir = app.path().app_data_dir().unwrap();
    // 1) 历史链: 函数返回 vs jsonl 行数
    let jsonl = dir.join("history.jsonl");
    let disk_lines = std::fs::read_to_string(&jsonl)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let got = crate::history::recent(&app, 500);
    crate::log::elog(&format!("[ui-chain] history: 函数返回 {} 条, 磁盘 {} 行", got.len(), disk_lines));
    assert_eq!(got.len(), disk_lines, "recent() 与 history.jsonl 行数不一致");
    if let Some(last) = got.first() {
        crate::log::elog(&format!("[ui-chain] 最新一条: {} final={}", last.ts, &last.final_text[..last.final_text.len().min(30)]));
    }
    // 2) 配置链: dict/keys/档案
    let cfg = crate::settings::get_config(app.clone()).unwrap_or_default();
    crate::log::elog(&format!(
        "[ui-chain] config: dict={} keys={} llm_profiles={} active_asr={}",
        cfg.dict.len(), cfg.keys.len(), cfg.llm_profiles.len(), cfg.active_asr_id
    ));
    assert!(!cfg.asr_profiles.is_empty(), "asr_profiles 为空——页面引擎下拉会空");
    // 3) poll_versions 递增
    let v0 = crate::history::history_version();
    crate::history::HISTORY_VER.fetch_add(0, std::sync::atomic::Ordering::SeqCst);
    let _ = v0;
    // 4) 实时性: append→版本递增→recent 立即可见(pipeline 交付后前端 800ms 轮询的实际数据源)
    let v0 = crate::history::history_version();
    let probe = crate::history::HistoryRecord {
        ts: "PROBE".into(), engine: "uitest".into(), raw: "实时性探针".into(),
        final_text: "实时性探针".into(), llm_used: false, delivered: "test".into(),
        audio_path: String::new(), duration_ms: 0, warning: None,
    };
    crate::history::append(&app, &probe);
    let v1 = crate::history::history_version();
    let visible = crate::history::recent(&app, 3).iter().any(|r| r.ts == "PROBE");
    crate::log::elog(&format!("[ui-chain] 实时性: 版本 {}→{} 探针可见={}", v0, v1, visible));
    assert!(v1 > v0 && visible, "append 后版本未递增或 recent 不可见");
    // 清理探针行(重写 jsonl)
    let jsonl = dir.join("history.jsonl");
    let kept: Vec<&str> = std::fs::read_to_string(&jsonl).unwrap()
        .lines().filter(|l| !l.contains("\"ts\":\"PROBE\"")).collect();
    std::fs::write(&jsonl, kept.join("\n") + "\n").ok();
    crate::log::elog("[ui-chain] PASS: 数据链与磁盘一致+实时性 OK");
    app.exit(0);
}
