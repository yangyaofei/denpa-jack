// Tauri command 层(对齐 Handy commands/): 薄壳, 业务在各自模块
use tauri::AppHandle;

use crate::audio;
use crate::delivery;
use crate::engine;
use crate::history;
use crate::pipeline;
use crate::config::Config;
use tokio::sync::mpsc as ttx;
use crate::dict::budget_hotwords;
use crate::config;

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
    config::get_config(app).map(|c| budget_hotwords(&c.dict)).unwrap_or_default()
}

/// 麦克风解析: 优先级第一个在线 → 指定设备 → 系统默认

#[tauri::command]
pub fn hud_hide(app: tauri::AppHandle) {
    // 隐藏的唯一入口(hud.rs): 含最短显示守卫与快照清理——策略不在 command 层
    crate::hud::hide(&app);
}

#[tauri::command]
pub fn open_config_file(app: tauri::AppHandle) -> Result<(), String> {
    let p = config::config_path(&app);
    tauri_plugin_opener::open_path(p.to_str().unwrap_or("."), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_data_dir(app: tauri::AppHandle) -> Result<(), String> {
    let dir = crate::config::data_dir(&app);
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

/// R1: 历史条目重转——后台重算回写条目, 不入队/不交付/不弹 HUD。
/// 音频按 180ms/6400B 节奏重放给引擎(与主链路同构), 结算在
/// engine::spawn_rerun_session 的事件桥里(词典→可选 LLM→update_by_ts)。
#[tauri::command]
pub fn rerun_history(app: tauri::AppHandle, ts: String, audio_path: String) -> Result<(), String> {
    if audio_path.is_empty() {
        return Err("该条目没有保留音频, 无法重转".into());
    }
    std::thread::spawn(move || rerun_blocking(app, ts, audio_path));
    Ok(())
}

/// 失败段重试（queue_retry）专用: 全链路重放**含交付**——用户点重试就是
/// 要结果出来。与 R1 的历史页"重转"(后台重算回写, 不交付)是两个语义,
/// 共用重放节奏, 不共用结算通道。
pub(crate) fn replay_for_delivery(app: tauri::AppHandle, audio_path: String) -> Result<(), String> {
    replay_impl(app, None, audio_path)
}

fn rerun_blocking(app: tauri::AppHandle, ts: String, audio_path: String) -> Result<(), String> {
    replay_impl(app, Some(ts), audio_path)
}

/// 重放公共骨架: 读 wav → 剥头 → 引擎会话 → 180ms/6400B 节奏 Feed → Finish。
/// locate_ts=Some ⇒ R1 重转(spawn_rerun_session, 队列可见, 回写不交付);
/// locate_ts=None ⇒ 失败段重试(spawn_session, 走主链路入队交付)。
fn replay_impl(app: tauri::AppHandle, locate_ts: Option<String>, audio_path: String) -> Result<(), String> {
    let cfg: Config = config::get_config(app.clone())?;
    let (profile, key) = crate::engine::resolve_asr(&cfg)?;
    let raw = std::fs::read(&audio_path).map_err(|e| format!("读音频失败: {e}"))?;
    // 剥 WAV 头取 PCM: 复用 wav_data_chunk(跳过 FLLR 填充块)——旧 raw[44..]
    // 假设会把 fmt 头当 PCM 喂引擎(C40 教训的重演, 审核a-9)
    let pcm = crate::history::wav_data_chunk(&raw).unwrap_or_else(|| raw.clone());
    let (tx, rx) = ttx::channel::<engine::Cmd>(64);
    let handoff = std::sync::Arc::new(std::sync::Mutex::new(Some(pipeline::SessionHandoff {
        audio_path: Some(audio_path.clone()),
        pcm: std::sync::Arc::new(Vec::new()),
        focus: delivery::FocusLock::snapshot(),
        duration_ms: 0,
        engine: profile.provider.clone(),
        gen: 0,
        // 重放不采集屏幕: 截图属于"当时那次录音", 事后拿到的是现在的屏幕会误导纠错
        screenshot: None,
    })));
    match locate_ts {
        Some(ts) => engine::spawn_rerun_session(app, profile.provider, key, profile.base_url.clone(), budget_hotwords(&cfg.dict), ts, handoff, rx),
        None => engine::spawn_session(app, profile.provider, key, profile.base_url.clone(), budget_hotwords(&cfg.dict), handoff, rx),
    }
    // 重放节奏与真实录音相当(200ms 分片×~0.9), 过快会塞爆通道或触发流式限速
    tauri::async_runtime::spawn(async move {
        for chunk in pcm.chunks(6400) {
            if tx.try_send(engine::Cmd::Feed(chunk.to_vec())).is_err() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(180)).await;
        }
        let _ = tx.try_send(engine::Cmd::Finish);
    });
    Ok(())
}

/// C54: 绑定变更后重注册(挂起态除外——录制中由 resume_all 统一恢复)。
/// command 注册已删(前端零调用); 保存配置处(use_config)是唯一消费者。
pub(crate) fn reapply_hotkey(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let state = app
        .try_state::<crate::shortcut::ShortcutState>()
        .ok_or("快捷键引擎未初始化")?;
    state.inner().unregister_all()?;
    let cfg = config::get_config(app.clone())?;
    for hk in cfg.transcribe_bindings() {
        state.inner().register(&hk)?;
    }
    Ok(())
}

/// 录制期间挂起全部绑定(真注销)——command 壳已删, Rust 侧直接调 shortcut::suspend_all
/// 权限总检查: 返回全部必需权限(麦克风/辅助功能)的当前状态, 缺项时同时推事件给前端
#[tauri::command]
pub fn check_permissions(_app: AppHandle) -> Vec<crate::permissions::PermState> {
    // 列表含必需项与可选项(屏幕录制), 前端按 required 区分展示; 提示只看必需项
    // (事件广播通道 C43b 不可达且无监听者: 返回值就是前端数据源)
    crate::permissions::all()
}

/// 主动请求屏幕录制权限(开关打开时调用; 首次会触发系统引导)
#[tauri::command]
pub fn request_screen_permission() -> bool {
    crate::screen_context::request()
}

/// 打开截图归档目录(事后判读/复现用)
#[tauri::command]
pub fn open_screen_context_dir(app: tauri::AppHandle) -> Result<(), String> {
    let dir = crate::config::data_dir(&app).join(crate::screen_context::DIR_NAME);
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
    let mut cfg = crate::config::get_config(app.clone()).unwrap_or_default();
    let uid = crate::audio::resolve_mic(&app, &mut cfg)?;
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
        crate::config::get_config(app)
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

/// LLM 链路自检(薄壳): 业务在 llm::selftest——词典纠错+真实调用+tool_used 判定
#[derive(serde::Serialize)]
pub struct LlmSelftestResult {
    dict_text: String,
    llm_text: String,
    pub thinking: Option<String>,
    tool_used: bool,
    latency_ms: u128,
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
    let cfg = crate::config::get_config(app.clone())?;
    let o = profile.map(|o| crate::config::LlmProfile {
        id: "selftest".into(),
        name: "selftest".into(),
        provider: if o.provider.is_empty() { "deepseek".into() } else { o.provider },
        base_url: o.base_url,
        model: o.model,
        api_key: o.api_key,
        prompt: o.prompt,
        thinking: o.thinking,
        effort: if o.effort.is_empty() { "low".into() } else { o.effort },
    });
    let r = crate::llm::selftest(&cfg, o).await?;
    Ok(LlmSelftestResult {
        dict_text: r.dict_text,
        llm_text: r.llm_text,
        thinking: r.thinking,
        tool_used: r.tool_used,
        latency_ms: r.latency_ms,
    })
}

/// 内置润色 prompt 原文(前端编辑回填用)
#[tauri::command]
pub fn get_default_prompt() -> String {
    crate::llm::DEFAULT_LLM_PROMPT.to_string()
}

/// UI v6 已废除独立设置窗: 托盘"设置"入口直接聚焦主窗(tray_events.rs 内联实现)
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
    let combo = crate::config::normalize_combo(&combo);
    if combo.is_empty() {
        return Err("组合不能为空".into());
    }
    // 验证: handy-keys Hotkey 必须可解析(支持纯修饰/fn/普通组合)
    combo
        .parse::<handy_keys::Hotkey>()
        .map_err(|e| format!("组合无法解析({combo}): {e}"))?;
    let mut cfg = crate::config::get_config(app.clone())?;
    let set = cfg
        .bindings
        .entry("transcribe".into())
        .or_insert(crate::config::BindingSet { current: vec![] });
    if !set.current.contains(&combo) {
        set.current.push(combo);
    }
    crate::config::save_config(app.clone(), cfg)?;
    crate::shortcut::resume_all(&app)?;
    Ok(())
}

/// 删除一个绑定组合; 至少保留一个(Handy: every shortcut should have a value)
#[tauri::command]
pub fn remove_binding(app: tauri::AppHandle, combo: String) -> Result<(), String> {
    let mut cfg = crate::config::get_config(app.clone())?;
    if let Some(set) = cfg.bindings.get_mut("transcribe") {
        set.current.retain(|c| c != &combo);
        if set.current.is_empty() {
            return Err("至少保留一个快捷键".into());
        }
    }
    crate::config::save_config(app.clone(), cfg)?;
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
pub struct Versions {
    pub history: u64,
    pub config: u64,
    pub mics: u64,
    /// 是否在录(core 事实投影)——主窗试音按钮跟随热键/托盘录音的同步源
    pub recording: bool,
    /// R1: 进行中的历史重转 ts 列表(前端三态"重算中"的数据源)
    pub reruns: Vec<String>,
    /// 在途段一览(Live cur + 队列 + Rerun)——主窗状态条渲染, 数据同 HUD 投影
    pub queue: Vec<SegLite>,
}

#[tauri::command]
pub fn poll_versions() -> Versions {
    // 队列投影(主窗状态条): 在途段一览, 段数最多个位数, 800ms 轮询带上无压力
    let (cur, queue) = crate::segment_queue::views();
    let mut segs: Vec<SegLite> = Vec::new();
    if let Some(c) = cur {
        segs.push(SegLite { state: c.state, source: c.source, text: c.text });
    }
    for s in queue {
        segs.push(SegLite { state: s.state, source: s.source, text: s.text });
    }
    Versions {
        history: crate::core::history_version(),
        config: crate::core::config_version(),
        mics: crate::core::mic_list_version(),
        recording: crate::core::session_recording(),
        reruns: crate::core::rerun_active(),
        queue: segs,
    }
}

/// 轮询附带的轻量段视图(主窗状态条渲染用)
#[derive(serde::Serialize, Clone)]
pub struct SegLite {
    pub state: String,
    pub source: String,
    pub text: String,
}

// ── 内置本地网关(特化 provider: 随机端口联动, 零配置; 外部网关档案不受此管) ──

#[derive(serde::Serialize)]
pub struct LocalSvcStatus {
    /// stopped | starting | running | failed(从未就绪即退) | crashed(就绪后退)
    pub state: String,
    pub port: u16,
    pub elapsed_secs: Option<u64>,
    pub exit: Option<String>,
    pub tail: Vec<String>, // 网关 stdout/stderr 尾部(\r/ANSI 已归一, 官方输出全保留)
}

#[tauri::command]
pub fn local_service_status(app: tauri::AppHandle) -> LocalSvcStatus {
    let s = crate::engine::gateway::snapshot();
    let state = if s.healthy {
        "running".to_string()
    } else if s.alive {
        "starting".to_string()
    } else if s.exit.is_some() {
        if s.healthy_ever { "crashed" } else { "failed" }.to_string()
    } else {
        "stopped".to_string()
    };
    LocalSvcStatus {
        state,
        port: s.port,
        elapsed_secs: s.started_at.map(|t| t.elapsed().as_secs()),
        exit: s.exit.clone(),
        tail: crate::engine::gateway::log_tail(&app, 80),
    }
}

#[tauri::command]
pub fn local_service_start(app: tauri::AppHandle) -> Result<String, String> {
    let cfg = crate::config::get_config(app.clone())?;
    match crate::engine::gateway::start(&app, &cfg) {
        Ok(info) => Ok(info),
        Err(e) if e.starts_with("already-starting") => Ok(e), // 不是失败, 是"已在启动中"
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn local_service_stop() -> Result<String, String> {
    if crate::engine::gateway::stop() {
        Ok("stopped".into())
    } else {
        Ok("not-ours".into()) // 登记处没有自有子进程(外部手动实例 APP 不动)
    }
}

#[cfg(test)]
mod gap_tests {
    #[test]
    fn poll_versions_reflects_history_and_config_counters() {
        use super::*;
        // 计数已收口 core: bump 后 poll_versions 必须读到新值
        crate::core::bump_history();
        crate::core::bump_config();
        let v = poll_versions();
        assert!(v.history >= 1);
        assert!(v.config >= 1);
    }
}
