// 录音会话(闭包架构): 会话=一次按住→松开的完整生命周期。
// 全部会话状态(采集/命令通道/起点/焦点快照/交接箱)收进 RecordingSession,
// 会话结束=结构体 drop=取消令牌置位=附属物(定时器/音量循环/桥线程)随之失效。
// 无会话级全局 static; stop/abort 共用同一清理路径, 不存在"错误分支跳过清理"。
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc as ttx;

/// 最近一次会话的代际(stop 时写入; 转写中 Esc 兜底取消用——审计#4)
pub(crate) static LAST_GEN: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

use crate::audio;
use crate::delivery;
use crate::engine;
use crate::pipeline;
use crate::config::{self, Config};
use crate::shortcut::{esc_listen_begin, esc_listen_end};
use crate::{log, AppState};

/// 一次录音会话的完整状态——生命周期与按住-松开严格一致
pub struct RecordingSession {
    pub rec: audio::Recording,
    cmd_tx: ttx::Sender<engine::Cmd>,
    started_at: std::time::Instant,
    focus: delivery::FocusLock,
    gen: u64,
    /// 交接箱: stop 写入, 引擎 Result 到达时 take——会话数据随会话管道流动
    handoff: Arc<Mutex<Option<pipeline::SessionHandoff>>>,
    /// 屏幕上下文截图（开关开启时由录音开始的后台线程写入；采集未完成/失败为 None）
    screenshot: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl RecordingSession {
    /// 结束采集, 取出会话全部交接物: 数据 + 引擎命令通道 + 交接箱。
    /// rec 在此 drop → cancel 置位 → 定时器/音量循环/桥线程随之失效
    fn finish(
        self,
        engine: &str,
    ) -> (
        pipeline::SessionHandoff,
        ttx::Sender<engine::Cmd>,
        Arc<Mutex<Option<pipeline::SessionHandoff>>>,
    ) {
        let duration_ms = self.started_at.elapsed().as_millis() as u64;
        // 采集诊断: 每个会话都记"采到多少/峰值多少"——区分"采到静音"与"根本没采到"
        // (此前只有引擎侧日志, 出现 len=0 时无法判断是采集问题还是服务端问题)
        let (cap_total, cap_chunks, cap_peak) = self.rec.stats();
        crate::log::elog(&format!(
            "[audio] 会话采集: bytes={cap_total} chunks={cap_chunks} peak={cap_peak:.4} 按住时长={duration_ms}ms"
        ));
        let pcm = self.rec.take_pcm();
        (
            pipeline::SessionHandoff {
                audio_path: None,
                pcm: std::sync::Arc::new(pcm),
                focus: self.focus,
                duration_ms,
                engine: engine.to_string(),
                gen: self.gen,
                screenshot: self.screenshot.lock().unwrap().clone(),
            },
            self.cmd_tx,
            self.handoff,
        )
    }
}

/// 录音中实时音量 → hud 浮窗; 持会话取消令牌, 会话结束后自动退出。
/// 顺带承担"录音中静音提醒"(用户要求: 没听清不该等松手才说, 过程中就该知道):
/// level 是 RMS 0-1000 定点; 连续约 2.5s(21 帧 × 120ms)低于阈值就提示一次,
/// 之后只要重新出现声音就复位(下一次静音还能再提示)。
fn spawn_level_loop(app: tauri::AppHandle, cancel: Arc<std::sync::atomic::AtomicBool>, level: Arc<std::sync::atomic::AtomicU32>) {
    tauri::async_runtime::spawn(async move {
        let mut silent_frames: u32 = 0;
        let mut warned = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let lv = level.load(std::sync::atomic::Ordering::SeqCst);
            crate::core::session_level(lv);
            // 静音阈值: RMS 定点 < 2。实测该阈值必须贴着 0——
            // 用户的 USB 麦克风(DJI Wireless Mic)电平很低: 正常说话时 peak 只有 0.06,
            // 阈值 10 会在真的说话时也报"还没听到声音"(误报, 实测日志)。
            if lv < 2 {
                silent_frames += 1;
                if silent_frames == 21 && !warned {
                    warned = true;
                    crate::log::log(&app, "录音中静音提醒: 已连续 2.5s 无输入电平");
                    crate::hud::notify(&app, "⚠️ 还没听到声音");
                }
            } else {
                silent_frames = 0;
                warned = false;
            }
        }
    });
}

/// B5 超长截断: 持会话取消令牌——会话结束(停止/中止)即失效, 定时器醒来后不会操作已结束的会话。
/// 到期后**只发信号**(send_autostop), 决策与执行统一走协调器→效果线程——
/// 不再直调 ctrl_stop: 直调会绕过协调器, 其 recording 记忆与现实分叉,
/// 超长截断后的下一次按住会被吞掉(哑火一次), 见协调器 AutoStopDue 注释。
fn spawn_max_timer(app: tauri::AppHandle, secs: u32, cancel: Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs as u64));
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return; // 会话已结束, 本定时器作废
        }
        log::log(&app, "B5: 录音超长, 发自动截断信号(走协调器)");
        crate::send_autostop();
        crate::hud::notify(&app, "⚠️ 录音已达上限, 自动截断");
    });
}

/// 会话清理: stop/abort 共用——注销 esc/提示音, 一个不漏
fn session_cleanup(app: &tauri::AppHandle, cue: Option<crate::audio_feedback::Cue>) {
    esc_listen_end();
    if let Some(cue) = cue {
        let _ = config::get_config(app.clone()).map(|c| {
            if c.audio_feedback {
                crate::audio_feedback::play_on_main(app, cue);
            }
        });
    }
}

/// 录音开始的可见反馈: 显示浮窗（放在目标显示器上；与截图共用同一个目标，见 capture_target）。
///
/// 位置规则不变（屏幕底部居中，按 overlay_position），只是"哪块屏"跟随目标窗口，
/// 不再跟随鼠标——多屏时浮窗和截图不会再指向两个不同的东西。
fn show_recording_ui(app: &tauri::AppHandle, display: &crate::overlay::DisplayTarget) {
    crate::hud::recording_started(app, display);
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
    // C54: 录制期间绑定已真注销(suspend), 触发不可达——此守卫防御性保留
    let mut cfg: Config = config::get_config(app.clone())?;
    let (profile, key) = engine::resolve_asr(&cfg)?;
    {
        let st = state.lock().unwrap();
        if st.session.is_some() {
            return Err("已在录音中".into());
        }
    }
    // 权限总检查: 没有麦克风权限就不开录——直接说清缺什么、去哪儿开
    // (否则表现为"看起来在录音, 实际采到全零", 用户实测过这种困惑)
    if !crate::permissions::mic_granted() {
        let msg = match crate::permissions::mic_status_code() {
            0 => "麦克风权限未授予: 系统设置 → 隐私与安全性 → 麦克风",
            1 => "麦克风权限受限: 系统设置 → 隐私与安全性 → 麦克风",
            _ => "麦克风权限被拒绝: 请在 系统设置 → 隐私与安全性 → 麦克风 中允许",
        };
        log::elog(&format!(
            "[perm] 录音被拒绝: 麦克风未授权({})",
            crate::permissions::mic_status_text()
        ));
        crate::hud::notify(&app, msg);
        return Err(msg.into());
    }
    let mic = audio::resolve_mic(&app, &mut cfg);
    let mic_name = mic.clone();
    let (tx, rx) = ttx::channel::<engine::Cmd>(64);
    let (atx, arx) = mpsc::channel::<Vec<u8>>();
    // 音频自愈(Handy recorder.rs): open 失败重解析设备重试一次
    let rec = match audio::start_input(mic.clone(), atx.clone()) {
        Ok(r) => r,
        Err(first_err) => {
            log::elog(&format!("[audio] open 失败: {first_err}, 清缓存重试一次"));
            audio::invalidate_mic_cache();
            let mic = audio::resolve_mic(&app, &mut cfg);
            std::thread::sleep(std::time::Duration::from_millis(120));
            match audio::start_input(mic, atx) {
                Ok(r) => r,
                Err(e) => {
                    let perm = {
                        let m = e.to_lowercase();
                        m.contains("permission") || m.contains("denied") || m.contains("0x80070005")
                    };
                    if perm {
                        let _ = tauri::Emitter::emit_to(&app, "main", "recording-error", serde_json::json!({"error_type": "microphone_permission_denied", "message": e}));
                    }
                    return Err(e);
                }
            }
        }
    };
    // 交接箱: 会话作用域——stop 写, 引擎 Result 读(唯一交接点, 无全局)
    let handoff = Arc::new(Mutex::new(None));
    let focus = delivery::FocusLock::snapshot();
    engine::spawn_session(
        app.clone(),
        profile.provider.clone(),
        key,
        profile.base_url.clone(),
        crate::dict::budget_hotwords(&cfg.dict),
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
            // 审计#2(对齐 Handy transcription.rs:169 无界阻塞保序): 会话通道满=引擎背压,
            // 阻塞等而不是丢帧(丢帧=文本截尾无告警); 会话已死时 send 立即返回 Err
            if cmd_tx.blocking_send(engine::Cmd::Feed(chunk)).is_err() {
                log::elog("[bridge] 引擎通道已关闭, 停止喂入(余量音频丢弃)");
                break;
            }
        }
    });
    let cancel = rec.cancel.clone();
    let level = rec.level_handle();
    let gen = pipeline::SESSION_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    // 屏幕上下文: 按下时探测一次活动目标（焦点窗口 → 指针窗口 → 指针所在屏），
    // 截图与浮窗共用；采集编排在 screen_context(阶段A 第2步下沉), 这里只拿结果槽
    let target = crate::active_target::detect_active_target();
    let shot_slot = crate::screen_context::spawn_session_capture(&app, &target, &cfg.detect_active_target);
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
            screenshot: shot_slot,
        });
    }
    esc_listen_begin();
    // 按下即显示浮窗（截图在后台进行，不参与显示顺序）
    show_recording_ui(&app, &target.display);
    spawn_level_loop(app.clone(), cancel.clone(), level);
    // 自测钩子: VOICEMAC_AUTOTEST_MAX_SECS 覆盖超长上限(不改用户配置),
    // 供 timelimit 模式验证"自动截断→可立即再录"的状态机闭环
    let hook = std::env::var("VOICEMAC_AUTOTEST_MAX_SECS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());
    let max_secs = hook.unwrap_or(cfg.max_recording_seconds);
    if max_secs > 0 {
        // 生产值保留下限防御(防手滑配 1s 截断); 测试钩子显式注入则忠实执行
        let secs = if hook.is_some() { max_secs } else { max_secs.max(10) };
        spawn_max_timer(app.clone(), secs, cancel);
    }
    log::log(&app, &format!("录音开始 engine={} mic={:?}", profile.provider, mic_name));
    crate::tray_events::refresh_status(&app);
    if cfg.audio_feedback {
        crate::audio_feedback::play_on_main(&app, crate::audio_feedback::Cue::Start);
    }
    Ok(())
}

/// 停止: 无论走哪条分支(正常/太短丢弃), 清理路径唯一且必走
fn profile_name_of(app: &tauri::AppHandle) -> String {
    config::get_config(app.clone())
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
    // 尾音缓冲(Handy extra_recording_buffer_ms 等价): 可感知取消的分片睡眠, 期间采集继续
    let tail = config::get_config(app.clone()).map(|c| c.extra_tail_ms).unwrap_or(0).min(2000);
    if tail > 0 {
        for _ in 0..(tail / 20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let (ho, cmd_tx, handoff) = sess.finish(&engine);
    // B3 判据=实际音频字节数(默认 0.3s): duration_ms 含引擎/采集启动开销,
    // 200ms 的音频会因 duration 达到 300ms 被误判为有效语音而交付。
    // D5: 下限接配置 min_recording_seconds(16k mono i16le = 32000 B/s),
    // floor 0.05s 与 serde 缺字段保护一致; 旧实现硬编码 9600B, 配置形同摆设。
    let min_secs = config::get_config(app.clone())
        .map(|c| c.min_recording_seconds)
        .unwrap_or(0.3)
        .max(0.05);
    let min_bytes = (min_secs * 32000.0) as usize;
    if ho.pcm.len() < min_bytes {
        // B3 太短: Abort 引擎(关 WS 无交付), 交接箱不写 → post_process 不会跑
        let _ = cmd_tx.try_send(engine::Cmd::Abort);
        session_cleanup(&app, None);
        crate::hud::hide(&app);
        log::log(&app, &format!("录音太短已丢弃 {}ms", ho.duration_ms));
        return Err("录音太短已丢弃".into());
    }
    // 交接箱写入(引擎 Result 到达时 take) + 引擎收尾
    {
        let mut st = state.lock().unwrap();
        st.last_audio = ho.audio_path.clone();
    }
    *LAST_GEN.lock().unwrap() = Some(ho.gen);
    *handoff.lock().unwrap() = Some(ho.clone());
    // 审计#2: Finish 丢失=会话挂死(watchdog 只在 Finish 分支布防)——失败必须告警
    if cmd_tx.blocking_send(engine::Cmd::Finish).is_err() {
        crate::log::elog("[recording] Finish 发送失败: 引擎会话已死, 发 asr-error");
        crate::hud::error(&app, "引擎会话异常中断");
    }
    session_cleanup(&app, Some(crate::audio_feedback::Cue::End));
    log::log(&app, &format!("录音结束 {:.1}s", ho.duration_ms as f64 / 1000.0));
    // 松手 = 告诉 HUD 一个事实:"这一条录完了"。阶段/文案/延时全部由 HUD 自己决定;
    // 这里不再直接改快照, 也不再重复 show 窗口(重复 show 就是用户看到的"瞬间消失再出现")。
    crate::hud::recording_ended(&app);
    crate::tray_events::refresh_status(&app);
    Ok(())
}

pub fn ctrl_abort(app: &tauri::AppHandle, state: &std::sync::Mutex<AppState>) -> Result<(), String> {
    let sess = {
        let mut st = state.lock().unwrap();
        st.session.take()
    };
    // 中止=会话终结: 事实必须复位(core 化后 abort 与 stop 同样有义务清 recording,
    // 否则 Esc 后"在录"残留为真, 投影层(浮窗/托盘)全部显示错——timelimit e2e 断言4 抓获)
    crate::core::session_ended();
    // 转写中 esc: 标记本代被取消——若 Result 已在途, 交付层据此跳过粘贴
    if let Some(s) = sess.as_ref() {
        pipeline::CANCELLED_GEN.store(s.gen, std::sync::atomic::Ordering::SeqCst);
    } else {
        // 审计#4: session 已 take(Result 在途/post_process 已跑)——用最近一代 gen 兜底取消
        if let Some(g) = LAST_GEN.lock().unwrap().take() {
            pipeline::CANCELLED_GEN.store(g, std::sync::atomic::Ordering::SeqCst);
        }
    }
    // 会话 drop=cancel 置位; 无论有无会话, 清理路径一致
    if let Some(s) = sess {
        s.cmd_tx.try_send(engine::Cmd::Abort).ok();
    } else {
        crate::engine::send_current(app, engine::Cmd::Abort);
    }
    session_cleanup(app, None);
    crate::hud::hide(app);
    crate::tray_events::refresh_status(app);
    log::log(app, "录音取消(esc)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // 已搬函数的用例随迁(阶段A 第2步): budget_hotwords→dict.rs, resolve_asr→engine/mod.rs, wav_data_chunk→history.rs
}

#[cfg(test)]
mod gap_tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn min_bytes_floor_never_zero() {
        // D5: 太短判据 = min_recording_seconds × 32000 B/s; default_config() 0.3s → 9600B(与旧硬编码等值);
        // derive Default/serde 缺字段时为 0.0 → floor 0.05s(1600B), 判据永不失效
        assert_eq!(Config::default().min_recording_seconds, 0.0, "derive Default 无默认时长");
        assert_eq!(crate::config::default_config().min_recording_seconds, 0.3);
        assert_eq!((0.3f64 * 32000.0) as usize, 9600);
        assert_eq!((0.0f64.max(0.05) * 32000.0) as usize, 1600, "floor 0.05s");
    }
}
