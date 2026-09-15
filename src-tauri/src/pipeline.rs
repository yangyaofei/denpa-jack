// 后处理链: asr-result → 词典纠错 → 可选 LLM → 剪贴板/粘贴 → 历史 → asr-final
use crate::settings::Config;
use tauri::Emitter;

/// 会话交接数据(由 RecordingSession 产生, 随 Result 事件流入)——会话级状态不进全局
#[derive(Clone, Default)]
pub struct SessionHandoff {
    pub audio_path: Option<String>,
    /// 待落盘 PCM(交付线程异步存盘, 停止路径不阻塞——Handy spawn_blocking 等价)
    pub pcm: std::sync::Arc<Vec<u8>>,
    pub focus: crate::deliver::FocusLock,
    pub duration_ms: u64,
    pub engine: String,
    /// 会话代: 交付前与 CANCELLED_GEN 比对, 被取消的会话跳过粘贴只入历史
    pub gen: u64,
}

/// 转写中取消: abort 时写入被放弃的会话代, 交付前校验(Handy cancel_generation 等价)
pub static CANCELLED_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SESSION_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn post_process(app: tauri::AppHandle, raw: String, ho: Option<SessionHandoff>) {
    // 转写内容属隐私: 仅 debug 构建落日志, release 脱敏(Handy redact_text 等价)
    #[cfg(debug_assertions)]
    crate::log::elog(&format!("[pipeline] begin raw={:.40}", raw));
    #[cfg(not(debug_assertions))]
    crate::log::elog("[pipeline] begin");
    let ho = ho.unwrap_or_default();
    let cfg: Config = match crate::settings::get_config(app.clone()) {
        Ok(c) => c,
        Err(e) => {
            let _ = crate::emit_both(&app, "asr-final", serde_json::json!({"raw": raw, "final": raw, "llm_used": false, "warning": format!("配置读取失败: {e}")}));
            crate::hud_set(|h| h.finished = Some(serde_json::json!({"raw": raw, "final": raw, "llm_used": false, "warning": format!("配置读取失败: {e}")})));
            return;
        }
    };

    if raw.trim().is_empty() {
        let _ = crate::emit_both(&app, "asr-final", serde_json::json!({
            "raw": raw, "final": "", "llm_used": false, "delivered": "none",
            "warning": "未识别到语音内容",
        }));
        crate::hud_set(|h| h.finished = Some(serde_json::json!({"raw": raw, "final": "", "llm_used": false, "delivered": "none", "warning": "未识别到语音内容"})));
        return;
    }

    // pcm 随会话流入: 异步落盘+清理(停止路径不再同步写盘, Handy spawn_blocking 等价)
    if !ho.pcm.is_empty() {
        let app2 = app.clone();
        let pcm = ho.pcm.clone();
        let keep = cfg.keep_audio_count.max(1) as usize;
        std::thread::spawn(move || {
            if let Some(p) = crate::history::save_wav(&app2, &pcm) {
                use tauri::Manager;
                app2.state::<std::sync::Mutex<crate::AppState>>()
                    .lock().unwrap().last_audio = Some(p);
            }
            crate::history::prune_recordings(&app2, keep);
        });
    }

    // 1) 词典纠错(毫秒级, 本地)
    let (dict_text, _matches) = crate::dict::TextCorrector { entries: cfg.dict.clone(), normalizations: cfg.normalizations.iter().map(|r| (r.pattern.clone(), r.replacement.clone())).collect() }.correct(&raw);
    let mut final_text = raw.clone(); // 默认交付=ASR 原文; LLM 成功→润色版, 失败/超时→保持原文
    let mut llm_used = false;
    let mut warning: Option<String> = None;
    let mut history_thinking: Option<String> = None;

    // 2) LLM 润色(可选; 提供tools模型自主决定; 5s 超时/失败降级词典版——永不阻塞交付)
    if cfg.use_llm_correction {
        if let Some(prof) = cfg.llm_profiles.iter().find(|p| p.id == cfg.active_llm_id) {
            let key = if prof.api_key.is_empty() {
                cfg.keys.first().cloned().unwrap_or_default()
            } else {
                prof.api_key.clone()
            };
            if !key.is_empty() {
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
                // 词典注入: 带变体映射(纠错指向性更强, LLM 上下文充足不加预算限制)
                let terms: Vec<String> = cfg
                    .dict
                    .iter()
                    .map(|d| {
                        if d.variants.is_empty() {
                            d.term.clone()
                        } else {
                            format!("{}(误: {})", d.term, d.variants.join("/"))
                        }
                    })
                    .collect();
                let _ = Emitter::emit_to(&app, "hud", "hud-busy", "✦ AI 润色中…");
                crate::hud_set(|h| h.status = "busy".into());
                let mut llm_thinking: Option<String> = None;
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();

                let polished = rt.map(|r| {
                    r.block_on(async {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            crate::llm::polish(&dict_text, &terms, &opts),
                        )
                        .await
                        {
                            Ok(Ok(out)) => {
                                llm_thinking = out.thinking;
                                Some(out.text)
                            }
                            Ok(Err(e)) => {
                                warning = Some(format!("LLM 失败: {e}"));
                                final_text = raw.clone(); // 降级=ASR 原文
                                None
                            }
                            Err(_) => {
                                warning = Some("LLM 超时(60s)".into());
                                final_text = raw.clone(); // 降级=ASR 原文
                                None
                            }
                        }
                    })
                });
                history_thinking = llm_thinking.clone();
                if let Ok(Some(t)) = polished {
                    if !t.trim().is_empty() {
                        final_text = t;
                        llm_used = true;
                    }
                }
            } else {
                warning = Some("LLM 档案缺 Key".into());
            }
        } else {
            warning = Some("未找到启用的 LLM 档案".into());
        }
    }

    // 3) 交付: 剪贴板(+粘贴)
    let _ = Emitter::emit_to(&app, "hud", "hud-busy", "交付中…");
    crate::log::elog(&format!("[pipeline] deliver begin clip_only={}", cfg.clipboard_only));
    let cancelled = ho.gen != 0 && ho.gen == CANCELLED_GEN.load(std::sync::atomic::Ordering::SeqCst);
    let delivered = if cancelled {
        crate::log::elog("[pipeline] 会话已被取消(esc): 只入历史不粘贴");
        "cancelled".to_string()
    } else {
        match deliver(&app, &final_text, cfg.clipboard_only, ho.focus.clone()) {
        Ok(d) => d,
        Err(e) => {
            warning = Some(match warning.take() {
                Some(w) => format!("{w}; 交付失败: {e}"),
                None => format!("交付失败: {e}"),
            });
            "failed".to_string()
        }
        }
    };

    // 4) 历史
    let audio_path = ho.audio_path.clone().unwrap_or_default();
    let rec = crate::history::HistoryRecord {
        llm_thinking: history_thinking.clone(),
        ts: now_local_pub(),
        engine: if ho.engine.is_empty() { cfg.active_asr_id.clone() } else { ho.engine.clone() },
        raw,
        final_text: final_text.clone(),
        llm_used,
        delivered: delivered.clone(),
        audio_path,
        warning: warning.clone(),
        duration_ms: ho.duration_ms,
    };
    crate::history::append(&app, &rec);
    crate::history::enforce_limit(&app, cfg.history_limit);
    crate::log::elog(&format!("[pipeline] delivered={delivered} history appended"));

    let fin_payload = serde_json::json!({
        "raw": rec.raw, "final": final_text, "llm_used": llm_used,
        "delivered": delivered, "warning": warning,
    });
    let _ = crate::emit_both(&app, "asr-final", fin_payload.clone());
    crate::hud_set(|h| { h.status.clear(); h.finished = Some(fin_payload); });
    // 转写态收尾: 托盘复位(三态: 默认/红=录音/黄=转写)
    if let Some(t) = crate::tray_events::TRAY.lock().unwrap().as_ref() {
        t.set_transcribing(false);
    }
}




fn deliver(app: &tauri::AppHandle, text: &str, clipboard_only: bool, focus: crate::deliver::FocusLock) -> Result<String, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let ax_ok = crate::deliver::ax_trusted(false);
    let focus_ok = focus.still_valid();
    crate::log::elog(&format!(
        "[deliver] ax_trusted={ax_ok} focus_bundle={:?} focus_ok={focus_ok}",
        focus.bundle
    ));
    // 焦点是本 app(设置窗等) → 只复制, 粘贴到自身无意义
    if focus.bundle.as_deref() == Some(crate::settings::APP_ID) {
        app.clipboard().write_text(text.to_string()).map_err(|e| format!("剪贴板写入失败: {e}"))?;
        return Ok("copied-self".into());
    }
    // 1) AX 直写光标(不碰剪贴板; 焦点校验通过才尝试, B10)
    if !clipboard_only && focus_ok {
        match crate::deliver::ax_insert(text) {
            Ok(()) => return Ok("pasted-ax".into()),
            Err(e) => crate::log::elog(&format!("[deliver] ax_insert 失败: {e}")),
        }
    }
    // 2) paste_tx 需要的配置
    let cfg_h: crate::settings::Config = crate::settings::get_config(app.clone()).unwrap_or_default();
    let app2_h = app.clone();
    if clipboard_only {
        // 审计#1(对齐 Handy clipboard.rs:74): 只复制模式也必须真写剪贴板
        use tauri_plugin_clipboard_manager::ClipboardExt;
        app.clipboard().write_text(text.to_string()).map_err(|e| format!("写剪贴板失败: {e}"))?;
        return Ok("copied".into());
    }
    // 3) 焦点已变 → 不粘贴, 防止把文本贴到其它应用(B10)
    if !focus.still_valid() {
        return Ok("focus-changed-copied-only".into());
    }
    // 4) 回执式可靠粘贴(Handy paste_tx 全套): 延迟供数据(lazy promise)+读取回执+安静期恢复+changeCount 守卫
    let text2 = text.to_string();
    let r = app.run_on_main_thread(move || {
        let _ = crate::paste_tx::reliable_paste(&text2, &app2_h, &cfg_h);
    });
    if r.is_err() {
        // 审计#11: 兜底也先写文本再注入(Handy clipboard.rs:74 注入失败路径同样先写)
        use tauri_plugin_clipboard_manager::ClipboardExt;
        let _ = app.clipboard().write_text(text.to_string());
        paste_cmd_v()?;
    }
    Ok("pasted-cmdv".into())
}

/// 模拟 Cmd+V —— CGEvent 直发(线程安全; enigo 内部走 HIToolbox 输入法 API
/// 有主线程 dispatch_assert_queue 断言, 后台线程调用直接 SIGTRAP——C39 崩溃实锤)
fn paste_cmd_v() -> Result<(), String> {
    crate::deliver::cg_cmd_key(0x09) // kVK_ANSI_V
}

pub fn now_local_pub() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // UTC+8 近似格式化(自实现, 不依赖时区库)
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600 + 8, (rem % 3600) / 60, rem % 60); // UTC+8
    let (y, mo, d) = civil_from_days(days);
    let h = h % 24;
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
