// 后处理链: asr-result → 词典纠错 → 可选 LLM → 剪贴板/粘贴 → 历史 → asr-final
use crate::config::Config;

/// 会话交接数据(由 RecordingSession 产生, 随 Result 事件流入)——会话级状态不进全局
#[derive(Clone, Default)]
pub struct SessionHandoff {
    pub audio_path: Option<String>,
    /// 待落盘 PCM(交付线程异步存盘, 停止路径不阻塞——Handy spawn_blocking 等价)
    pub pcm: std::sync::Arc<Vec<u8>>,
    pub focus: crate::delivery::FocusLock,
    pub duration_ms: u64,
    pub engine: String,
    /// 会话代: 交付前与 CANCELLED_GEN 比对, 被取消的会话跳过粘贴只入历史
    pub gen: u64,
    /// 屏幕上下文截图（开关开启时由录音开始的后台线程采集；未开启/采集失败为 None）
    pub screenshot: Option<std::path::PathBuf>,
}

/// 转写中取消: abort 时写入被放弃的会话代, 交付前校验(Handy cancel_generation 等价)
/// 被取消的会话代集合(R2: 多段连续取消——旧单槽 AtomicU64 在"取消A→再录B→再取消B"时
/// 把 A 的标记冲掉, A 照样交付, 审核a-3)。消费即删, 上限 16 防御性截断。
pub static CANCELLED_GENS: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// 标记一代被取消(Esc)。段在队列里被标记 → post_process 开头闸直接短路;
/// 段已开跑 → 交付前闸拦截。
pub fn mark_cancelled(gen: u64) {
    if gen == 0 {
        return; // 0=不参与取消的会话(rerun 构造), 不标记
    }
    let mut g = CANCELLED_GENS.lock().unwrap();
    if !g.contains(&gen) {
        g.push(gen);
    }
    let len = g.len();
    if len > 16 {
        g.drain(0..len - 16);
    }
}

/// 该代是否被取消——**消费即删**: 两道闸谁先到谁消费, 第二道闸不再命中,
/// 集合不会随时间膨胀。
fn take_cancelled(gen: u64) -> bool {
    if gen == 0 {
        return false;
    }
    let mut g = CANCELLED_GENS.lock().unwrap();
    match g.iter().position(|&x| x == gen) {
        Some(pos) => {
            g.remove(pos);
            true
        }
        None => false,
    }
}
pub static SESSION_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 后处理结果 —— 分段队列据此决定"出队"还是"保留为失败段等重试"(见 segment_queue.rs)
pub enum Outcome {
    /// 正常走完(含 LLM 降级成原文, 以及被 esc 取消后只入历史的情况)
    Delivered,
    /// ASR 没识别到语音内容(有 warning, 不算失败——重试也没有意义)
    NoSpeech,
    /// 失败(配置读取失败 / 交付失败等): 队列保留该段, 音频与原文进历史, 可重试。
    /// 带上已产出的纠正稿与 llm_used——历史留痕由队列 worker 统一写(修双写),
    /// 信息不许降级成"只剩 raw"。
    Failed { reason: String, final_text: String, llm_used: bool },
}

pub fn post_process(app: tauri::AppHandle, raw: String, ho: SessionHandoff) -> Outcome {
    // 转写内容属隐私: 仅 debug 构建落日志, release 脱敏(Handy redact_text 等价)
    #[cfg(debug_assertions)]
    crate::log::elog(&format!("[pipeline] begin raw={:.40}", raw));
    #[cfg(not(debug_assertions))]
    crate::log::elog("[pipeline] begin");

    // R2 第一道闸(段还没开跑就被 Esc): 直接短路到"cancelled 入历史",
    // 不跑词典/LLM/交付——被取消的段连算都不用算。第二道闸在交付前(LLM 进行中标记的场景)。
    if take_cancelled(ho.gen) {
        // 引擎名: 取消短路发生在 cfg 读取前, 这里单独解析(配置坏了取消语义照样成立)
        let engine_name = crate::config::get_config(app.clone())
            .ok()
            .map(|c| if ho.engine.is_empty() { c.active_asr_id.clone() } else { ho.engine.clone() })
            .unwrap_or_else(|| ho.engine.clone());
        crate::log::elog("[pipeline] 会话已被取消(esc): 队列段短路, 只入历史");
        crate::history::append(&app, &crate::history::HistoryRecord {
            llm_thinking: None,
            ts: now_local_pub(),
            engine: engine_name,
            raw: raw.clone(),
            final_text: String::new(),
            llm_used: false,
            delivered: "cancelled".to_string(),
            audio_path: ho.audio_path.clone().unwrap_or_default(),
            warning: None,
            duration_ms: ho.duration_ms,
        });
        return Outcome::Delivered;
    }
    let cfg: Config = match crate::config::get_config(app.clone()) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("配置读取失败: {e}");
            let payload = serde_json::json!({"raw": raw, "final": raw, "llm_used": false, "warning": msg});
            // 交付完成只"报事实"给 HUD(它自己决定是否采纳——录音中不覆盖当前回显)
            crate::hud::delivered(&app, payload);
            return Outcome::Failed { reason: msg, final_text: raw, llm_used: false };
        }
    };

    if raw.trim().is_empty() {
        let payload = serde_json::json!({
            "raw": raw, "final": "", "llm_used": false, "delivered": "none",
            "warning": "未识别到语音内容",
        });
        crate::hud::delivered(&app, payload);
        return Outcome::NoSpeech;
    }

    // pcm 的落盘与历史写入统一延后到第 4) 步(交付完成后)在子线程执行(D4: 路径回填)

    // 1) 词典纠错(毫秒级, 本地)
    let (dict_text, _matches) = crate::dict::TextCorrector { entries: cfg.dict.clone(), normalizations: cfg.normalizations.iter().map(|r| (r.pattern.clone(), r.replacement.clone())).collect() }.correct(&raw);
    // 默认交付=词典纠错版(D1): LLM 关闭/失败/超时都落到这一档, 不再退回 ASR 原文——
    // 词典是本地毫秒级能力, 没理由在 LLM 缺席时丢弃它。raw 只进历史留痕。
    let mut final_text = dict_text.clone();
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
                        crate::llm::DEFAULT_LLM_PROMPT.to_string()
                    } else {
                        prof.prompt.clone()
                    },
                    thinking: prof.thinking,
                    effort: if prof.effort.is_empty() { "low".into() } else { prof.effort.clone() },
                    timeout_secs: cfg.llm_timeout_secs,
                    max_tokens: cfg.llm_max_tokens,
                    retries: cfg.llm_retries,
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
                // 告诉 HUD "进入 AI 润色阶段"。用户可能已经在录下一段了——
                // 这时 HUD 不采纳(它自己持有"录音中"的状态), 不会打断实时回显(issue #2)。
                crate::hud::polishing(&app);
                let mut llm_thinking: Option<String> = None;
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();

                let polished = rt.map(|r| {
                    r.block_on(async {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            crate::llm::polish(&dict_text, &terms, &opts, ho.screenshot.as_deref()),
                        )
                        .await
                        {
                            Ok(Ok(out)) => {
                                llm_thinking = out.thinking;
                                Some(out.text)
                            }
                            Ok(Err(e)) => {
                                warning = Some(format!("LLM 失败: {e}"));
                                final_text = dict_text.clone(); // 降级=词典纠错版(D1)
                                None
                            }
                            Err(_) => {
                                warning = Some("LLM 超时(60s)".into());
                                final_text = dict_text.clone(); // 降级=词典纠错版(D1)
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
    //    `trailing_newlines` 只改"交付出去的文本"(末尾补换行), 不改历史里的原文——
    //    因为它是"输出形态"而不是"识别结果"。发真回车键是另一回事(会把内容直接提交出去), 不做。
    let out_text = trailing_newlines_text(&final_text, cfg.trailing_newlines);
    crate::log::elog(&format!("[pipeline] deliver begin clip_only={}", cfg.clipboard_only));
    // R2 第二道闸(段开跑后才被 Esc——典型: LLM 进行中按取消): 交付前拦截, 只入历史。
    // 消费即删: 与第一道闸互斥, 先到者消费后另一道不再命中。
    let cancelled = take_cancelled(ho.gen);
    let delivered = if cancelled {
        crate::log::elog("[pipeline] 会话已被取消(esc): 只入历史不粘贴");
        "cancelled".to_string()
    } else {
        match deliver(&app, &out_text, cfg.clipboard_only, ho.focus.clone()) {
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

    // 4) 历史 + 音频落盘(统一在落盘线程): save_wav 的真实路径回填进历史条目(D4)——
    //    旧实现先 append(路径恒空)再异步落盘, 成功条目永远无法"重跑", wav 成孤儿(审核 a-5)。
    //    交付失败**不在此写**: 由队列 worker 的失败留痕(record_failed)统一写(修双写)。
    //    rerun 会话自带 audio_path(原始音频), 保留优先, 不被新落盘覆盖。
    let raw_for_payload = raw.clone();
    if delivered != "failed" {
        let rec = crate::history::HistoryRecord {
            llm_thinking: history_thinking.clone(),
            ts: now_local_pub(),
            engine: if ho.engine.is_empty() { cfg.active_asr_id.clone() } else { ho.engine.clone() },
            raw,
            final_text: final_text.clone(),
            llm_used,
            delivered: delivered.clone(),
            audio_path: ho.audio_path.clone().unwrap_or_default(),
            warning: warning.clone(),
            duration_ms: ho.duration_ms,
        };
        // 停止路径不阻塞(Handy spawn_blocking 等价): 落盘+历史+清理都在子线程,
        // 前端历史刷新本就靠 poll_versions 版本轮询(≤800ms), 用户无感。
        let app2 = app.clone();
        let keep = cfg.keep_audio_count.max(1) as usize;
        let hist_limit = cfg.history_limit;
        let ho_pcm = ho.pcm.clone();
        std::thread::spawn(move || {
            let disk_path = if !ho_pcm.is_empty() {
                let p = crate::history::save_wav(&app2, &ho_pcm);
                if let Some(ref path) = p {
                    use tauri::Manager;
                    app2.state::<std::sync::Mutex<crate::AppState>>()
                        .lock().unwrap().last_audio = Some(path.clone());
                }
                p
            } else {
                None
            };
            let mut rec = rec;
            if rec.audio_path.is_empty() {
                rec.audio_path = disk_path.unwrap_or_default();
            }
            crate::history::append(&app2, &rec);
            crate::history::enforce_limit(&app2, hist_limit);
            if !ho_pcm.is_empty() {
                crate::history::prune_recordings(&app2, keep);
            }
            crate::log::elog(&format!(
                "[pipeline] delivered={} history appended audio={}",
                rec.delivered,
                if rec.audio_path.is_empty() { "-" } else { "ok" }
            ));
        });
    }

    let fin_payload = serde_json::json!({
        "raw": raw_for_payload, "final": final_text, "llm_used": llm_used,
        "delivered": delivered, "warning": warning,
    });
    // 交付结束 = 报事实给 HUD: 它会清掉阶段并带上交付载荷; 若此刻正在录新一段,
    // 它不采纳(不会把新一段的回显清掉——旧实现在这里无条件 clear, 就是用户看到的"内容消失又重新出现")。
    crate::hud::delivered(&app, fin_payload);
    // 托盘复位交给队列统一处理(队列里可能还有别的段在排队/转写)
    if delivered == "failed" {
        Outcome::Failed {
            reason: warning.clone().unwrap_or_else(|| "交付失败".into()),
            final_text,
            llm_used,
        }
    } else {
        Outcome::Delivered
    }
}




fn deliver(app: &tauri::AppHandle, text: &str, clipboard_only: bool, focus: crate::delivery::FocusLock) -> Result<String, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let ax_ok = crate::delivery::ax_trusted(false);
    let focus_ok = focus.still_valid();
    crate::log::elog(&format!(
        "[deliver] ax_trusted={ax_ok} focus_bundle={:?} focus_ok={focus_ok}",
        focus.bundle
    ));
    // D3: 原剪贴板快照必须在第一次写剪贴板之前取——粘贴回执后的"恢复"恢复的是这份,
    // 旧实现在 reliable_paste 内部快照, 读到的是已写入的转写文本, 恢复=没恢复。
    let saved = crate::delivery::paste::ClipboardSnapshot::take(app);
    // 焦点是本 app(设置窗等) → 只复制, 粘贴到自身无意义
    if focus.bundle.as_deref() == Some(crate::config::APP_ID) {
        app.clipboard().write_text(text.to_string()).map_err(|e| format!("剪贴板写入失败: {e}"))?;
        return Ok("copied-self".into());
    }
    // 1) 先无条件写剪贴板 —— 用户定则: 不管走哪条交付路径, 这段文本都必须能拿得到。
    //    修 bug: 以前 AX 直写成功就 return, 剪贴板里什么都没有, 用户事后拿不回转写结果。
    app.clipboard().write_text(text.to_string()).map_err(|e| format!("剪贴板写入失败: {e}"))?;
    // 3-prev) 配置提前取一次(粘贴优先名单 + paste_tx 都要用)
    let cfg_h: crate::config::Config = crate::config::get_config(app.clone()).unwrap_or_default();
    // 2) AX 直写光标(点不碰剪贴板式插入; 焦点校验通过才尝试, B10)。
    //    粘贴优先名单命中 → 跳过 AX(该 App 的 AX 写入会重复输入, 见 settings 字段注释)
    let ax_skip = ax_paste_only(&cfg_h.ax_paste_only_apps, focus.bundle.as_deref());
    if ax_skip {
        crate::log::elog(&format!(
            "[deliver] 粘贴优先名单命中, 跳过 AX 直写: {}",
            focus.bundle.as_deref().unwrap_or("?")
        ));
    }
    if !clipboard_only && focus_ok && !ax_skip {
        match crate::delivery::ax_insert(text) {
            Ok(()) => return Ok("pasted-ax".into()),
            Err(e) => crate::log::elog(&format!("[deliver] ax_insert 失败: {e}")),
        }
    }
    let app2_h = app.clone();
    if clipboard_only {
        // 只复制模式: 上一步已经真写过剪贴板(审计#1 对齐 Handy clipboard.rs:74)
        return Ok("copied".into());
    }
    // 4) 焦点已变 → 不粘贴, 防止把文本贴到其它应用(B10); 文本已在剪贴板, 用户可手动粘贴
    if !focus.still_valid() {
        return Ok("focus-changed-copied-only".into());
    }
    // 5) 回执式可靠粘贴(Handy paste_tx 全套): 延迟供数据(lazy promise)+读取回执+安静期恢复+changeCount 守卫
    let text2 = text.to_string();
    let saved2 = saved;
    let r = app.run_on_main_thread(move || {
        let _ = crate::delivery::paste::reliable_paste(&text2, &app2_h, &cfg_h, saved2);
    });
    if r.is_err() {
        // 审计#11: 兜底也先写文本再注入(Handy clipboard.rs:74 注入失败路径同样先写)
        use tauri_plugin_clipboard_manager::ClipboardExt;
        let _ = app.clipboard().write_text(text.to_string());
        paste_cmd_v()?;
    }
    Ok("pasted-cmdv".into())
}

/// 粘贴优先名单匹配（纯函数, 便于单测）：bundle id 命中即跳过 AX 直写
fn ax_paste_only(apps: &[String], bundle: Option<&str>) -> bool {
    match bundle {
        Some(b) => apps.iter().any(|x| x.trim() == b),
        None => false,
    }
}

/// 模拟 Cmd+V —— CGEvent 直发(线程安全; enigo 内部走 HIToolbox 输入法 API
/// 有主线程 dispatch_assert_queue 断言, 后台线程调用直接 SIGTRAP——C39 崩溃实锤)
fn paste_cmd_v() -> Result<(), String> {
    crate::delivery::cg_cmd_key(0x09) // kVK_ANSI_V
}

pub fn now_local_pub() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    format_epoch_utc8(secs)
}

/// UTC+8 格式化(纯函数, 可测): **先加 8 小时再分解**——旧实现先分解后给小时 +8,
/// UTC 16:00–24:00(北京次日 0–8 点)日期不进位, 历史时间戳少一天(审核 a-8)。
/// 时区固定北京(个人工具, 不引时区库)。
pub fn format_epoch_utc8(secs: i64) -> String {
    let secs = secs + 8 * 3600;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
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

/// 交付文本 = 识别结果 + 末尾 N 个换行（配置 `trailing_newlines`）。
///
/// 只在交付这一步生效：换行是"输出形态"，不是"识别结果"，所以历史记录仍存原文，
/// 从历史重跑时会按当时的配置重新拼一次。
/// 上限 10：每句话都会追加，误配过大会把输入框撑满空行。
fn trailing_newlines_text(text: &str, n: u32) -> String {
    match n.min(10) {
        0 => text.to_string(),
        k => format!("{text}{}", "\n".repeat(k as usize)),
    }
}

#[cfg(test)]
mod tests {
    use super::{ax_paste_only, format_epoch_utc8, mark_cancelled, take_cancelled, trailing_newlines_text, CANCELLED_GENS};

    #[test]
    fn utc8_crosses_midnight_correctly() {
        // 2026-10-24 16:00:00 UTC = 北京 2026-10-25 00:00:00 —— 旧实现日期不进位(少一天)
        let epoch = 1792857600i64; // 2026-10-24T16:00:00Z
        assert_eq!(format_epoch_utc8(epoch), "2026-10-25 00:00:00");
        // 日内不跨日: 2026-10-24 01:10:05 UTC = 北京 09:10:05
        let epoch2 = 1792804205i64; // 2026-10-24T01:10:05Z
        assert_eq!(format_epoch_utc8(epoch2), "2026-10-24 09:10:05");
    }

    #[test]
    fn trailing_newlines_rules() {
        assert_eq!(trailing_newlines_text("你好", 0), "你好", "0 = 不追加");
        assert_eq!(trailing_newlines_text("你好", 1), "你好\n", "1 = 换行一次");
        assert_eq!(trailing_newlines_text("你好", 2), "你好\n\n", "2 = 空一行");
        assert_eq!(
            trailing_newlines_text("你好", 99),
            "你好\n\n\n\n\n\n\n\n\n\n",
            "上限 10"
        );
    }

    #[test]
    fn ax_paste_only_rules() {
        let apps = vec!["com.google.GeminiMacOS".to_string(), "  com.x.y  ".to_string()];
        assert!(ax_paste_only(&apps, Some("com.google.GeminiMacOS")), "精确命中");
        assert!(ax_paste_only(&apps, Some("com.x.y")), "两侧空白应被容忍");
        assert!(!ax_paste_only(&apps, Some("com.apple.Preview")), "未命中");
        assert!(!ax_paste_only(&apps, None), "拿不到 bundle 不拦截");
        assert!(!ax_paste_only(&[], Some("com.google.GeminiMacOS")), "空名单不拦截");
    }

    #[test]
    fn cancelled_gens_consume_once_and_cap() {
        // R2: 取消集合消费即删(两道闸互斥), 容量上限 16 防御性截断, gen=0 不参与
        let mut g = CANCELLED_GENS.lock().unwrap();
        g.clear();
        drop(g);
        mark_cancelled(0);
        assert!(!take_cancelled(0), "gen=0 不参与取消(rerun 会话)");
        mark_cancelled(7);
        assert!(take_cancelled(7), "标记后可消费");
        assert!(!take_cancelled(7), "消费即删: 第二道闸不再命中");
        mark_cancelled(7);
        assert!(!take_cancelled(8), "未标记的代不误伤");
        {
            let mut g = CANCELLED_GENS.lock().unwrap();
            g.clear();
            for i in 1..=20 { g.push(i); }
            drop(g);
        }
        mark_cancelled(21);
        let g = CANCELLED_GENS.lock().unwrap();
        assert_eq!(g.len(), 16, "上限 16: 超出截断最旧");
        assert!(*g.last().unwrap() == 21, "最新标记保留");
    }
}
