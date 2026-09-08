// 录音会话(闭包架构): 会话=一次按住→松开的完整生命周期。
// 全部会话状态(采集/命令通道/起点/焦点快照/交接箱)收进 RecordingSession,
// 会话结束=结构体 drop=取消令牌置位=附属物(定时器/音量循环/桥线程)自知作废。
// 无会话级全局 static; stop/abort 共用同一清理路径, 不存在"错误分支跳过清理"。
use tauri::{Emitter, Manager};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc as ttx;

use crate::audio;
use crate::deliver;
use crate::doubao;
use crate::engines;
use crate::overlay::{hide_hud, show_hud};
use crate::pipeline;
use crate::settings::{self, Config};
use crate::shortcut::{register_esc, unregister_esc};
use crate::tray_events::TRAY;
use crate::{log, AppState};

/// 一次录音会话的完整状态——生命周期与按住-松开严格一致
pub struct RecordingSession {
    pub rec: audio::Recording,
    cmd_tx: ttx::Sender<doubao::Cmd>,
    started_at: std::time::Instant,
    focus: deliver::FocusLock,
    gen: u64,
    /// 交接箱: stop 写入, 引擎 Result 到达时 take——会话数据随会话管道流动
    handoff: Arc<Mutex<Option<pipeline::SessionHandoff>>>,
}

impl RecordingSession {
    /// 结束采集, 取出会话全部交接物: 数据 + 引擎命令通道 + 交接箱。
    /// rec 在此 drop → cancel 置位 → 定时器/音量循环/桥线程自知作废
    fn finish(
        self,
        app: &tauri::AppHandle,
        engine: &str,
    ) -> (
        pipeline::SessionHandoff,
        ttx::Sender<doubao::Cmd>,
        Arc<Mutex<Option<pipeline::SessionHandoff>>>,
    ) {
        let duration_ms = self.started_at.elapsed().as_millis() as u64;
        let pcm = self.rec.take_pcm();
        (
            pipeline::SessionHandoff {
                audio_path: None,
                pcm: std::sync::Arc::new(pcm),
                focus: self.focus,
                duration_ms,
                engine: engine.to_string(),
                gen: self.gen,
            },
            self.cmd_tx,
            self.handoff,
        )
    }
}

pub fn budget_hotwords(dict: &[settings::DictEntry]) -> Vec<String> {
    let mut sorted: Vec<&settings::DictEntry> = dict.iter().collect();
    sorted.sort_by(|a, b| b.boost.cmp(&a.boost));
    let mut used = 0usize;
    let mut out = vec![];
    for e in sorted {
        let cost = e.term.chars().count() + 2;
        if used + cost > 100 {
            continue;
        }
        used += cost;
        out.push(e.term.clone());
    }
    out
}

/// 麦克风解析(存储键=CoreAudio DeviceUID 稳定硬件 ID):
/// 优先级按序取第一个在线 uid; 旧配置里存的名字在此迁移为 uid; 都不中回落指定设备/默认
/// 上次命中的麦克风 uid 缓存: 按下→录音的关键路径少一次全枚举(Handy cached_device)
static MIC_CACHE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn resolve_mic(app: &tauri::AppHandle, cfg: &mut Config) -> Option<String> {
    if let Some(uid) = MIC_CACHE.lock().unwrap().clone() {
        // 缓存命中: 不枚举, open 失败由 ctrl_start 的自愈重试全量重解析
        return Some(uid);
    }
    let online = audio::list_mics();
    let mut migrated = false;
    for item in cfg.mic_priority.iter_mut() {
        if let Some((uid, _)) = online.iter().find(|(_, n)| n == item).cloned() {
            *item = uid;
            migrated = true;
        }
    }
    if !cfg.mic_device_uid.is_empty() {
        if let Some((uid, _)) = online.iter().find(|(_, n)| *n == cfg.mic_device_uid).cloned() {
            cfg.mic_device_uid = uid;
            migrated = true;
        }
    }
    if migrated {
        crate::log::elog("[mic] 旧名字配置已迁移为硬件 uid");
        let _ = settings::save_config(app.clone(), cfg.clone());
    }
    let picked = cfg
        .mic_priority
        .iter()
        .find(|uid| online.iter().any(|(u, _)| u == *uid))
        .cloned()
        .or_else(|| {
            if !cfg.mic_device_uid.is_empty() && online.iter().any(|(u, _)| u == &cfg.mic_device_uid) {
                Some(cfg.mic_device_uid.clone())
            } else {
                None
            }
        })
        .or_else(|| start_input_default(&online));
    if let Some(uid) = &picked {
        *MIC_CACHE.lock().unwrap() = Some(uid.clone());
    }
    picked
}

/// 设备失效时清缓存(自愈重试路径调用)
pub fn invalidate_mic_cache() {
    *MIC_CACHE.lock().unwrap() = None;
}

fn start_input_default(online: &[(String, String)]) -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    if let Some(d) = cpal::default_host().default_input_device() {
        if let Ok(uid) = d.id() {
            let u = uid.to_string();
            if online.iter().any(|(x, _)| *x == u) {
                return Some(u);
            }
        }
    }
    online.first().map(|(u, _)| u.clone())
}

pub fn resolve_asr(cfg: &Config) -> Result<(settings::AsrProfile, String), String> {
    let profile = cfg
        .asr_profiles
        .iter()
        .find(|p| p.id == cfg.active_asr_id)
        .cloned()
        .or_else(|| cfg.asr_profiles.first().cloned())
        .ok_or("没有 ASR 档案, 请先在设置中添加")?;
    if !["volcengine", "zhipu", "openai"].contains(&profile.provider.as_str()) {
        return Err(format!("未知引擎: {}", profile.provider));
    }
    let key = if profile.api_key.is_empty() {
        cfg.keys.first().cloned().ok_or("没有 API Key")?
    } else {
        profile.api_key.clone()
    };
    Ok((profile, key))
}

/// 录音中实时音量 → hud 浮窗; 持会话取消令牌, 会话结束自知退出
fn spawn_level_loop(app: tauri::AppHandle, cancel: Arc<std::sync::atomic::AtomicBool>, level: Arc<std::sync::atomic::AtomicU32>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let _ = Emitter::emit(&app, "asr-level", level.load(std::sync::atomic::Ordering::SeqCst));
        }
    });
}

/// B5 超长截断: 持会话取消令牌——会话结束(停止/中止)即作废, 不存在睡醒后操作死会话
fn spawn_max_timer(app: tauri::AppHandle, secs: u32, cancel: Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs.max(10) as u64));
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return; // 会话已结束, 本定时器作废
        }
        let st = app.state::<std::sync::Mutex<AppState>>();
        let mut s = st.lock().unwrap();
        if s.session.is_some() {
            log::log(&app, "B5: 录音超长, 自动截断");
            drop(s);
            let _ = ctrl_stop(app.clone(), &st.inner());
            crate::overlay::show_hud_msg(&app, "⚠️ 录音已达上限, 自动截断");
        }
    });
}

/// 会话清理: stop/abort 共用——注销 esc/托盘复位/提示音, 一个不漏
fn session_cleanup(app: &tauri::AppHandle, cue: Option<crate::audio_feedback::Cue>) {
    unregister_esc(app);
    if let Some(t) = TRAY.lock().unwrap().as_ref() {
        t.set_recording(false);
    }
    if let Some(cue) = cue {
        let _ = settings::get_config(app.clone()).map(|c| {
            if c.audio_feedback {
                crate::audio_feedback::play_on_main(app, cue);
            }
        });
    }
}

#[tauri::command]
pub fn recording_start() -> Result<(), String> {
    crate::send_ctrl(true);
    Ok(())
}

#[tauri::command]
pub fn recording_stop() -> Result<(), String> {
    crate::send_ctrl(false);
    Ok(())
}

#[tauri::command]
pub fn recording_abort() -> Result<(), String> {
    crate::send_cancel();
    Ok(())
}

pub fn ctrl_start(app: tauri::AppHandle, state: &std::sync::Mutex<AppState>) -> Result<(), String> {
    let mut cfg: Config = settings::get_config(app.clone())?;
    let (profile, key) = resolve_asr(&cfg)?;
    {
        let st = state.lock().unwrap();
        if st.session.is_some() {
            return Err("已在录音中".into());
        }
    }
    let mic = resolve_mic(&app, &mut cfg);
    let mic_name = mic.clone();
    let (tx, rx) = ttx::channel::<doubao::Cmd>(64);
    let (atx, arx) = mpsc::channel::<Vec<u8>>();
    // 音频自愈(Handy recorder.rs): open 失败重解析设备重试一次
    let rec = match audio::start_input(mic.clone(), atx.clone()) {
        Ok(r) => r,
        Err(first_err) => {
            log::elog(&format!("[audio] open 失败: {first_err}, 清缓存重试一次"));
            invalidate_mic_cache();
            let mic = resolve_mic(&app, &mut cfg);
            std::thread::sleep(std::time::Duration::from_millis(120));
            match audio::start_input(mic, atx) {
                Ok(r) => r,
                Err(e) => {
                    let perm = {
                        let m = e.to_lowercase();
                        m.contains("permission") || m.contains("denied") || m.contains("0x80070005")
                    };
                    if perm {
                        let _ = tauri::Emitter::emit(&app, "recording-error", serde_json::json!({"error_type": "microphone_permission_denied", "message": e}));
                    }
                    return Err(e);
                }
            }
        }
    };
    // 交接箱: 会话作用域——stop 写, 引擎 Result 读(唯一交接点, 无全局)
    let handoff = Arc::new(Mutex::new(None));
    let focus = deliver::FocusLock::snapshot();
    engines::spawn_session(
        app.clone(),
        profile.provider.clone(),
        key,
        budget_hotwords(&cfg.dict),
        handoff.clone(),
        rx,
    );
    let cmd_tx = tx.clone();
    let bridge_cancel = rec.cancel.clone();
    std::thread::spawn(move || {
        while let Ok(chunk) = arx.recv() {
            if bridge_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if cmd_tx.try_send(doubao::Cmd::Feed(chunk)).is_err() {
                break;
            }
        }
    });
    let cancel = rec.cancel.clone();
    let level = rec.level_handle();
    let gen = pipeline::SESSION_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    {
        let mut st = state.lock().unwrap();
        st.engine_mirror = Some(tx.clone());
        st.session = Some(RecordingSession {
            rec,
            cmd_tx: tx,
            started_at: std::time::Instant::now(),
            focus,
            gen,
            handoff,
        });
    }
    register_esc(&app);
    show_hud(&app);
    spawn_level_loop(app.clone(), cancel.clone(), level);
    if cfg.max_recording_seconds > 0 {
        spawn_max_timer(app.clone(), cfg.max_recording_seconds, cancel);
    }
    log::log(&app, &format!("录音开始 engine={} mic={:?}", profile.provider, mic_name));
    if cfg.audio_feedback {
        crate::audio_feedback::play_on_main(&app, crate::audio_feedback::Cue::Start);
    }
    Ok(())
}

/// 停止: 无论走哪条分支(正常/太短丢弃), 清理路径唯一且必走
fn profile_name_of(app: &tauri::AppHandle) -> String {
    settings::get_config(app.clone())
        .ok()
        .and_then(|c| c.asr_profiles.iter().find(|p| p.id == c.active_asr_id).cloned())
        .map(|p| p.provider)
        .unwrap_or_default()
}

pub fn ctrl_stop(app: tauri::AppHandle, state: &std::sync::Mutex<AppState>) -> Result<(), String> {
    let sess = {
        let mut st = state.lock().unwrap();
        st.session.take()
    };
    let Some(sess) = sess else {
        return Err("未在录音中".into());
    };
    let engine = profile_name_of(&app);
    // 尾音缓冲(Handy extra_recording_buffer_ms 等价): cancel-aware 分片睡, 期间采集继续
    let tail = settings::get_config(app.clone()).map(|c| c.extra_tail_ms).unwrap_or(0).min(2000);
    if tail > 0 {
        for _ in 0..(tail / 20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let (ho, cmd_tx, handoff) = sess.finish(&app, &engine);
    if ho.duration_ms < min_duration_ms(&app) {
        // B3 太短: Abort 引擎(关 WS 无交付), 交接箱不写 → post_process 不会跑
        let _ = cmd_tx.try_send(doubao::Cmd::Abort);
        session_cleanup(&app, None);
        hide_hud(&app);
        log::log(&app, &format!("录音太短已丢弃 {}ms", ho.duration_ms));
        return Err("录音太短已丢弃".into());
    }
    // 交接箱写入(引擎 Result 到达时 take) + 引擎收尾
    {
        let mut st = state.lock().unwrap();
        st.last_audio = ho.audio_path.clone();
    }
    *handoff.lock().unwrap() = Some(ho.clone());
    let _ = cmd_tx.try_send(doubao::Cmd::Finish);
    session_cleanup(&app, Some(crate::audio_feedback::Cue::End));
    log::log(&app, &format!("录音结束 {:.1}s", ho.duration_ms as f64 / 1000.0));
    if let Some(w) = app.get_webview_window("hud") {
        let _ = Emitter::emit(&app, "hud-state", "transcribing");
        let _ = w.show();
    }
    if let Some(t) = TRAY.lock().unwrap().as_ref() {
        t.set_transcribing(true);
    }
    Ok(())
}

pub fn ctrl_abort(app: &tauri::AppHandle, state: &std::sync::Mutex<AppState>) -> Result<(), String> {
    let sess = {
        let mut st = state.lock().unwrap();
        st.session.take()
    };
    // 转写中 esc: 标记本代被取消——若 Result 已在途, 交付层据此跳过粘贴
    if let Some(s) = sess.as_ref() {
        pipeline::CANCELLED_GEN.store(s.gen, std::sync::atomic::Ordering::SeqCst);
    }
    // 会话 drop=cancel 置位; 无论有无会话, 清理路径一致
    if let Some(s) = sess {
        s.cmd_tx.try_send(doubao::Cmd::Abort).ok();
    } else {
        crate::engines::send_current(app, doubao::Cmd::Abort);
    }
    session_cleanup(app, None);
    hide_hud(app);
    log::log(app, "录音取消(esc)");
    Ok(())
}

fn min_duration_ms(app: &tauri::AppHandle) -> u64 {
    settings::get_config(app.clone())
        .map(|c| (c.min_recording_seconds.max(0.05) * 1000.0) as u64)
        .unwrap_or(300)
}

/// 测试通道: 直接对当前引擎发命令(autotest 喂音用; 正常路径 cmd_tx 随会话走)
pub fn send_cmd_pub(app: &tauri::AppHandle, cmd: doubao::Cmd) {
    crate::engines::send_current(app, cmd);
}

/// 取 wav 的 data chunk(自测回放用)
pub fn wav_data_chunk(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 44 || &raw[0..4] != b"RIFF" {
        return None;
    }
    let mut i = 12usize;
    while i + 8 <= raw.len() {
        let id = &raw[i..i + 4];
        let size = u32::from_le_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        if id == b"data" {
            let end = (i + 8 + size).min(raw.len());
            return Some(raw[i + 8..end].to_vec());
        }
        i += 8 + size + (size % 2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_data_chunk_skips_fllr_padding() {
        // C40 回归: 真实录音 wav 有 FLLR 填充块, data 不在 offset 44
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(1000u32).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&(16u32).to_le_bytes());
        w.extend_from_slice(&[0u8; 16]);
        w.extend_from_slice(b"FLLR");
        w.extend_from_slice(&(4044u32).to_le_bytes());
        w.extend_from_slice(&vec![0xEEu8; 4044]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(10u32).to_le_bytes());
        w.extend_from_slice(b"0123456789");
        let pcm = wav_data_chunk(&w).unwrap();
        assert_eq!(pcm, b"0123456789");
    }
    #[test]
    fn wav_data_chunk_rejects_garbage() {
        assert!(wav_data_chunk(b"").is_none());
        assert!(wav_data_chunk(b"RIFFshort").is_none());
        assert!(wav_data_chunk(b"NOTAWAVE........").is_none());
    }
    #[test]
    fn budget_hotwords_respects_100_token_budget() {
        // C8: cost = 字数 + 2; 装不下跳过
        let d = vec![
            crate::settings::DictEntry { term: "谢克数学".into(), variants: vec![], guard_words: vec![], boost: 10 },
            crate::settings::DictEntry { term: "a".repeat(90), variants: vec![], guard_words: vec![], boost: 9 },
            crate::settings::DictEntry { term: "腾讯云".into(), variants: vec![], guard_words: vec![], boost: 5 },
        ];
        let out = budget_hotwords(&d);
        // 92 token 的词装不下跳过, 后面的还能继续装
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("谢克数学"));
    }
    #[test]
    fn resolve_asr_rejects_unknown_provider() {
        let mut cfg = crate::settings::default_config();
        cfg.active_asr_id = "x".into();
        cfg.asr_profiles = vec![crate::settings::AsrProfile {
            id: "x".into(), name: "x".into(), provider: "bogus".into(), api_key: "k".into(), hotwords_enabled: true,
        }];
        assert!(resolve_asr(&cfg).is_err());
    }
    #[test]
    fn resolve_asr_falls_back_to_first_profile() {
        let mut cfg = crate::settings::default_config();
        cfg.active_asr_id = "nope".into();
        cfg.asr_profiles = vec![crate::settings::AsrProfile {
            id: "p1".into(), name: "豆包".into(), provider: "volcengine".into(), api_key: "k1".into(), hotwords_enabled: true,
        }];
        let (p, k) = resolve_asr(&cfg).unwrap();
        assert_eq!(p.id, "p1");
        assert_eq!(k, "k1");
    }
    #[test]
    fn resolve_asr_empty_key_uses_pool() {
        let mut cfg = crate::settings::default_config();
        cfg.keys = vec!["pool-key".into()];
        cfg.asr_profiles = vec![crate::settings::AsrProfile {
            id: "p1".into(), name: "n".into(), provider: "zhipu".into(), api_key: "".into(), hotwords_enabled: true,
        }];
        let (_, k) = resolve_asr(&cfg).unwrap();
        assert_eq!(k, "pool-key");
    }
}
