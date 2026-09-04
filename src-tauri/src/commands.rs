// Tauri command 层(对齐 Handy commands/): 薄壳, 业务在各自模块
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::audio;
use crate::deliver;
use crate::doubao;
use crate::engines;
use crate::history;
use crate::log;
use crate::overlay::{hide_hud, show_hud_msg};
use crate::pipeline;
use crate::settings::Config;
use tokio::sync::mpsc as ttx;
use crate::recording::{budget_hotwords, resolve_asr};
use crate::settings;

#[tauri::command]
pub fn ping() -> String {
    "pong".into()
}

#[tauri::command]
pub fn list_mics() -> Vec<(String, String)> {
    audio::list_mics()
}

#[tauri::command]
pub fn get_history(app: tauri::AppHandle, limit: usize) -> Vec<history::HistoryRecord> {
    history::recent(&app, limit)
}

#[tauri::command]
pub fn clear_history(app: tauri::AppHandle) -> Result<(), String> {
    let p = history::history_path(&app);
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn copy_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}


#[tauri::command]
pub fn get_hotwords(app: tauri::AppHandle) -> Vec<String> {
    settings::get_config(app).map(|c| budget_hotwords(&c.dict)).unwrap_or_default()
}

/// 麦克风解析: 优先级第一个在线 → 指定设备 → 系统默认

#[tauri::command]
pub fn hud_hide(app: tauri::AppHandle) {
    hide_hud(&app);
}

#[tauri::command]
pub fn dev_nav(app: tauri::AppHandle, section: String) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let js = format!(
            "location.hash='#{}'; window.dispatchEvent(new Event('hashchange'))",
            section
        );
        let _ = w.eval(&js);
    }
}

#[tauri::command]
pub fn open_config_file(app: tauri::AppHandle) -> Result<(), String> {
    let p = settings::config_path(&app);
    tauri_plugin_opener::open_path(p.to_str().unwrap_or("."), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_data_dir(app: tauri::AppHandle) -> Result<(), String> {
    let dir = log::app_log_dir(&app).ok_or("目录不可用")?;
    tauri_plugin_opener::open_path(dir.to_str().unwrap_or("."), None::<&str>).map_err(|e| e.to_string())
}

/// hud 失败重试: 用本次录音(AUDIO_PATH)重跑整条管线; 前端控制只重试一次
#[tauri::command]
pub fn retry_last(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let path = app
        .state::<std::sync::Mutex<crate::AppState>>()
        .lock()
        .unwrap()
        .last_audio
        .clone()
        .or_else(|| crate::history::recent(&app, 1).first().map(|r| r.audio_path.clone()).filter(|p| !p.is_empty()))
        .ok_or("没有可重试的录音")?;
    rerun_history(app, path)
}

#[tauri::command]
pub fn rerun_history(app: tauri::AppHandle, audio_path: String) -> Result<(), String> {
    // 重放是长活(音频时长级), 必须后台线程——command 在主线程, 直接跑会冻结 UI
    std::thread::spawn(move || rerun_blocking(app, audio_path));
    Ok(())
}

fn rerun_blocking(app: tauri::AppHandle, audio_path: String) -> Result<(), String> {
    let cfg: Config = settings::get_config(app.clone())?;
    let (profile, key) = resolve_asr(&cfg)?;
    let raw = std::fs::read(&audio_path).map_err(|e| format!("读音频失败: {e}"))?;
    // 剥 WAV 头(44B)取 PCM
    let pcm = if raw.len() > 44 && &raw[0..4] == b"RIFF" { raw[44..].to_vec() } else { raw };
    let (tx, rx) = ttx::channel::<doubao::Cmd>(64);
    // 重跑也是一次会话: 交接箱随会话走, 与主链路同构
    let handoff = std::sync::Arc::new(std::sync::Mutex::new(Some(pipeline::SessionHandoff {
        audio_path: Some(audio_path.clone()),
        pcm: std::sync::Arc::new(Vec::new()),
        focus: deliver::FocusLock::snapshot(),
        duration_ms: 0,
        engine: profile.provider.clone(),
        gen: 0,
    })));
    engines::spawn_session(app.clone(), profile.provider.clone(), key, budget_hotwords(&cfg.dict), handoff, rx);
    use tauri::Manager;
    app.state::<std::sync::Mutex<crate::AppState>>()
        .lock()
        .unwrap()
        .last_audio = Some(audio_path.clone());
    tauri::async_runtime::spawn(async move {
        for chunk in pcm.chunks(6400) {
            if tx.try_send(doubao::Cmd::Feed(chunk.to_vec())).is_err() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(180)).await;
        }
        let _ = tx.try_send(doubao::Cmd::Finish);
    });
    show_hud_msg(&app, "… 重新转写中");
    Ok(())
}

/// 配置变更后重注册快捷键
#[tauri::command]
pub fn reapply_hotkey(app: tauri::AppHandle) -> Result<(), String> {
    let cfg: Config = settings::get_config(app.clone())?;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let s: Shortcut = cfg.hotkey.shortcut_str().parse().map_err(|e| format!("快捷键解析失败: {e}"))?;
    gs.register(s).map_err(|e| format!("注册失败: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn check_permissions(app: tauri::AppHandle) -> bool {
    let ok = deliver::ax_trusted(false);
    if !ok {
        let _ = Emitter::emit(&app, "permission-ax", false);
    }
    ok
}

// ==== 自启动(对齐 Handy autostart) ====
#[tauri::command]
pub fn autostart_enable(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .enable()
        .map_err(|e| format!("开启自启动失败: {e}"))
}

#[tauri::command]
pub fn autostart_disable(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .disable()
        .map_err(|e| format!("关闭自启动失败: {e}"))
}

#[tauri::command]
pub fn autostart_status(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

// 浮窗高度自适应: 文本变多时调档(长文本看全)
#[tauri::command]
pub fn hud_resize(app: tauri::AppHandle, height: f64) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("hud") {
        let _ = w.set_size(tauri::LogicalSize::new(480.0, height.clamp(170.0, 400.0)));
    }
}

/// 当前将使用/已使用的输入设备(uid+name)——设置页"当前输入设备"显示这个。
/// 已录音 → 运行时事实; 未录音 → 按配置预解析(优先级→指定→默认), 同步预填 ACTIVE_MIC
#[tauri::command]
pub fn get_active_mic(app: tauri::AppHandle) -> Option<(String, String)> {
    // 每次都按当前配置解析(优先级→指定→默认): 配置一改, 显示立即跟手
    let mut cfg = crate::settings::get_config(app.clone()).unwrap_or_default();
    let uid = crate::recording::resolve_mic(&app, &mut cfg)?;
    let name = crate::audio::list_mics()
        .into_iter()
        .find(|(u, _)| *u == uid)
        .map(|(_, n)| n)?;
    Some((uid, name))
}

/// 按供应商拉取可用 LLM 模型列表(设置页下拉)
#[tauri::command]
pub async fn list_llm_models(
    app: tauri::AppHandle,
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let key = if api_key.is_empty() {
        crate::settings::get_config(app)
            .ok()
            .and_then(|c| c.keys.first().cloned())
            .unwrap_or_default()
    } else {
        api_key
    };
    crate::log::elog(&format!(
        "[llm-models] 拉取: provider={provider} base_url={base_url} key_len={}",
        key.len()
    ));
    match crate::llm::list_models(&provider, &base_url, &key).await {
        Ok(v) => {
            crate::log::elog(&format!("[llm-models] 成功: {} 个模型", v.len()));
            Ok(v)
        }
        Err(e) => {
            crate::log::elog(&format!("[llm-models] 失败: {e}"));
            Err(e)
        }
    }
}

/// 前端自检/错误日志落盘(打包版 console 不可见——统一走 app.log)
#[tauri::command]
pub fn ui_log(app: tauri::AppHandle, msg: String) {
    crate::log::log(&app, &format!("[ui] {msg}"));
}

/// LLM 链路自检: 固定测试输入 → 词典纠错 → LLM 润色(真实 tool_calls) → 各阶段结果
#[derive(serde::Serialize)]
pub struct LlmSelftestResult {
    dict_text: String,
    llm_text: String,
    tool_used: bool,
    latency_ms: u128,
    error: Option<String>,
}

#[derive(serde::Deserialize, Default)]
pub struct LlmSelftestOverride {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub effort: String,
}

/// selftest: 编辑页传 override(未保存草稿也能测); 链路页不传用 active 档案
#[tauri::command]
pub async fn llm_selftest(
    app: tauri::AppHandle,
    profile: Option<LlmSelftestOverride>,
) -> Result<LlmSelftestResult, String> {
    let cfg = crate::settings::get_config(app.clone())?;
    let raw = "根据配置文件里面我加的 你第一次把把配置不齐。";
    let (dict_text, _) = crate::dict::TextCorrector {
        entries: cfg.dict.clone(),
        normalizations: cfg
            .normalizations
            .iter()
            .map(|r| (r.pattern.clone(), r.replacement.clone()))
            .collect(),
    }
    .correct(raw);

    let prof = match profile {
        Some(o) => crate::settings::LlmProfile {
            id: "selftest".into(),
            name: "selftest".into(),
            provider: if o.provider.is_empty() { "deepseek".into() } else { o.provider },
            base_url: o.base_url,
            model: o.model,
            api_key: o.api_key,
            prompt: o.prompt,
            thinking: o.thinking,
            effort: if o.effort.is_empty() { "low".into() } else { o.effort },
        },
        None => cfg
            .llm_profiles
            .iter()
            .find(|p| p.id == cfg.active_llm_id)
            .cloned()
            .ok_or("未找到启用的 LLM 档案")?,
    };
    let key = if prof.api_key.is_empty() {
        cfg.keys.first().cloned().unwrap_or_default()
    } else {
        prof.api_key.clone()
    };
    if key.is_empty() {
        return Err("LLM 档案缺 Key(且 Key 池为空)".into());
    }
    let opts = crate::llm::LlmOpts {
        provider: prof.provider.clone(),
        base_url: prof.base_url.clone(),
        model: prof.model.clone(),
        api_key: key,
        prompt: if prof.prompt.is_empty() {
            crate::settings::DEFAULT_LLM_PROMPT.to_string()
        } else {
            prof.prompt.clone()
        },
        thinking: prof.thinking,
        effort: if prof.effort.is_empty() { "low".into() } else { prof.effort.clone() },
    };
    let terms: Vec<String> = cfg.dict.iter().map(|d| d.term.clone()).collect();
    let input = dict_text.clone();
    let t0 = std::time::Instant::now();
    let llm_text = crate::llm::polish(&input, &terms, &opts)
        .await
        .map_err(|e| format!("LLM 调用失败: {e}"))?;
    let latency_ms = t0.elapsed().as_millis();
    // tool_used 判定: 看 llm_logs 最新一条响应里有没有 tool_calls
    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = std::path::PathBuf::from(home)
        .join("Library/Application Support/com.yangyaofei.tauri-app/llm_logs");
    let mut tool_used = false;
    if let Ok(mut files) = std::fs::read_dir(&log_dir) {
        let mut paths: Vec<_> = files
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        paths.sort();
        if let Some(latest) = paths.pop() {
            if let Ok(body) = std::fs::read_to_string(&latest) {
                tool_used = body.contains("submit_corrected_text") && body.contains("tool_calls");
            }
        }
    }
    Ok(LlmSelftestResult {
        dict_text,
        llm_text,
        tool_used,
        latency_ms,
        error: None,
    })
}

/// 内置润色 prompt 原文(前端编辑回填用)
#[tauri::command]
pub fn get_default_prompt() -> String {
    crate::settings::DEFAULT_LLM_PROMPT.to_string()
}
