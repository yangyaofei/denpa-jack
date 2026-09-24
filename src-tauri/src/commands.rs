// Tauri command 层(对齐 Handy commands/): 薄壳, 业务在各自模块
use tauri::{Emitter, Manager};

use crate::audio;
use crate::deliver;
use crate::doubao;
use crate::engines;
use crate::history;
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
    // 隐藏的唯一入口(hud.rs): 含最短显示守卫与快照清理——策略不在 command 层
    crate::hud::hide(&app);
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
    let dir = crate::settings::data_dir(&app);
    tauri_plugin_opener::open_path(dir.to_str().unwrap_or("."), None::<&str>).map_err(|e| e.to_string())
}

/// 应用版本（关于页显示）。
/// 取自 Cargo.toml 的 version（与 tauri.conf.json / package.json 发布时由 tag 统一注入，三者一致）。
/// 不走前端 `@tauri-apps/api/app` 的 getVersion()——那条路要动态加载一个 JS chunk，
/// 多一层依赖（chunk 加载/权限），一旦失败页面就只剩占位符；这里用我们自己的命令，同一套 invoke 通道。
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// hud 失败重试: 用本次录音(AppState.last_audio)重跑整条管线; 前端控制只重试一次
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
        // 重跑历史音频时不采集屏幕: 截图应当属于"当时那次录音"，事后重跑拿到的是现在的屏幕，会误导纠错
        screenshot: None,
    })));
    engines::spawn_session(app.clone(), profile.provider.clone(), key, profile.base_url.clone(), budget_hotwords(&cfg.dict), handoff, rx);
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
    crate::hud::notify(&app, "… 重新转写中");
    Ok(())
}

/// C54: 绑定变更后重注册(挂起态除外——录制中由 resume_all 统一恢复)
#[tauri::command]
pub fn reapply_hotkey(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let state = app
        .try_state::<crate::shortcut::ShortcutState>()
        .ok_or("快捷键引擎未初始化")?;
    state.inner().unregister_all()?;
    let cfg = settings::get_config(app.clone())?;
    for hk in cfg.transcribe_bindings() {
        state.inner().register(&hk)?;
    }
    Ok(())
}

/// 录制期间挂起全部绑定(真注销)
#[tauri::command]
pub fn suspend_all_bindings(app: tauri::AppHandle) -> Result<(), String> {
    crate::shortcut::suspend_all(&app)
}

/// 录制结束恢复全部绑定(幂等)
#[tauri::command]
pub fn resume_all_bindings(app: tauri::AppHandle) -> Result<(), String> {
    crate::shortcut::resume_all(&app)
}

/// 权限总检查: 返回全部必需权限(麦克风/辅助功能)的当前状态, 缺项时同时推事件给前端
#[tauri::command]
pub fn check_permissions(app: tauri::AppHandle) -> Vec<crate::permissions::PermState> {
    // 列表含必需项与可选项(屏幕录制), 前端按 required 区分展示; 提示只看必需项
    let list = crate::permissions::all();
    if !crate::permissions::missing().is_empty() {
        let _ = Emitter::emit_to(&app, "main", "permission-ax", false);
    }
    let _ = Emitter::emit_to(&app, "main", "permissions", &list);
    list
}

/// 主动请求屏幕录制权限(开关打开时调用; 首次会触发系统引导)
#[tauri::command]
pub fn request_screen_permission() -> bool {
    crate::screen_context::request()
}

/// 打开截图归档目录(事后判读/复现用)
#[tauri::command]
pub fn open_screen_context_dir(app: tauri::AppHandle) -> Result<(), String> {
    let dir = crate::settings::data_dir(&app).join(crate::screen_context::DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    tauri_plugin_opener::open_path(dir.to_str().unwrap_or("."), None::<&str>).map_err(|e| e.to_string())
}

/// 主动请求麦克风权限(未决定时弹系统框; 已拒绝不会弹, 需去系统设置)
#[tauri::command]
pub fn request_microphone() {
    crate::permissions::request_mic();
}

/// 打开系统设置的指定隐私面板(权限引导用; 只允许系统设置 URL)
#[tauri::command]
pub fn open_system_settings(url: String) -> Result<(), String> {
    if !url.starts_with("x-apple.systempreferences:") {
        return Err("仅允许打开系统设置面板".into());
    }
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("打开系统设置失败: {e}"))?;
    Ok(())
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

// 浮窗宽度切换(单栏 480 ↔ 两栏 664)。只改宽度: 高度由窗口固定, 长文本在框内滚动
#[tauri::command]
pub fn hud_resize(app: tauri::AppHandle, width: f64) {
    crate::hud::resize_width(&app, width);
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
    pub thinking: Option<String>,
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
        timeout_secs: cfg.llm_timeout_secs,
        max_tokens: cfg.llm_max_tokens,
        retries: cfg.llm_retries,
    };
    let terms: Vec<String> = cfg.dict.iter().map(|d| d.term.clone()).collect();
    let input = dict_text.clone();
    let t0 = std::time::Instant::now();
    let llm_out = crate::llm::polish(&input, &terms, &opts, None)
        .await
        .map_err(|e| format!("LLM 调用失败: {e}"))?;
    let llm_text = llm_out.text;
    let latency_ms = t0.elapsed().as_millis();
    // tool_used 判定: 看 llm_logs 最新一条响应里有没有 tool_calls
    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = std::path::PathBuf::from(home)
        .join("Library/Application Support")
        .join(crate::settings::APP_ID)
        .join("llm_logs");
    let mut tool_used = false;
    if let Ok(files) = std::fs::read_dir(&log_dir) {
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
        thinking: llm_out.thinking,
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

/// UI v6 已废除独立设置窗: 此命令改为聚焦主窗(托盘"设置"入口)
#[tauri::command]
pub fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    Err("主窗口不存在".into())
}

/// C55 后端录制(用户定则): begin→poll→end(提交/取消), 轮询通道
#[tauri::command]
pub fn capture_begin(app: tauri::AppHandle) -> Result<(), String> {
    crate::shortcut::capture_begin(&app)
}

#[tauri::command]
pub fn capture_poll() -> crate::shortcut::CaptureSnapshot {
    crate::shortcut::capture_poll()
}

#[tauri::command]
pub fn capture_end(app: tauri::AppHandle, confirm: bool, combo: Option<String>) -> Result<(), String> {
    crate::shortcut::capture_end(&app, confirm, combo)
}

/// C54 前端录制模式(Handy 同构): 前端 window keydown/keyup 收集组合,
/// 后端只提供 suspend/resume(真注销/重注册)与 confirm 提交。无 capture 线程、无 Channel、无门闩。
#[tauri::command]
pub fn add_binding(app: tauri::AppHandle, combo: String) -> Result<(), String> {
    let combo = crate::settings::normalize_combo(&combo);
    if combo.is_empty() {
        return Err("组合不能为空".into());
    }
    // 验证: handy-keys Hotkey 必须可解析(支持纯修饰/fn/普通组合)
    combo
        .parse::<handy_keys::Hotkey>()
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
    crate::shortcut::resume_all(&app)?;
    Ok(())
}

/// 删除一个绑定组合; 至少保留一个(Handy: every shortcut should have a value)
#[tauri::command]
pub fn remove_binding(app: tauri::AppHandle, combo: String) -> Result<(), String> {
    let mut cfg = crate::settings::get_config(app.clone())?;
    if let Some(set) = cfg.bindings.get_mut("transcribe") {
        set.current.retain(|c| c != &combo);
        if set.current.is_empty() {
            return Err("至少保留一个快捷键".into());
        }
    }
    crate::settings::save_config(app.clone(), cfg)?;
    reapply_hotkey(app)?;
    Ok(())
}

#[tauri::command]
pub fn hud_poll() -> crate::hud::Snapshot {
    // 前端唯一读接口(快照组装 + 队列派生 + 一次性字段的取后清都在 hud.rs)
    crate::hud::poll()
}

/// 重试队列里最早的一段失败段(浮窗「重试」按钮)
#[tauri::command]
pub fn queue_retry(app: tauri::AppHandle) -> Result<(), String> {
    let id = crate::segment_queue::first_failed_id().ok_or("没有失败段")?;
    crate::segment_queue::retry(&app, id)
}

/// 清空队列(浮窗「清空」按钮; 前端两步确认后调用)
/// 语义: 不转写 ≠ 丢数据 —— 排队段的音频与原文先进历史(delivered="cancelled")
#[tauri::command]
pub fn queue_clear(app: tauri::AppHandle) -> usize {
    crate::segment_queue::clear(&app)
}

/// 关闭失败提示(浮窗「关闭」按钮)
/// 语义: 只是不再提示; 失败段的音频与原文已在历史里(delivered="failed"), 可去主窗重跑
#[tauri::command]
pub fn queue_dismiss() -> usize {
    crate::segment_queue::dismiss_failed()
}

#[derive(serde::Serialize)]
pub struct Versions { pub history: u64, pub config: u64, pub mics: u64 }

#[tauri::command]
pub fn poll_versions() -> Versions {
    Versions {
        history: crate::history::history_version(),
        config: crate::settings::config_version(),
        mics: crate::mic_watch::mic_version(),
    }
}

#[cfg(test)]
mod gap_tests {
    #[test]
    fn poll_versions_reflects_history_and_config_counters() {
        use super::*;
        crate::history::HISTORY_VER.store(7, std::sync::atomic::Ordering::SeqCst);
        crate::settings::CONFIG_VER.store(3, std::sync::atomic::Ordering::SeqCst);
        let v = poll_versions();
        assert_eq!(v.history, 7);
        assert_eq!(v.config, 3);
    }
}
