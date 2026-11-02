//! 重转编排(R1)：历史条目的后台重算——识别会话驱动在 model，业务(入队投影/
//! 结算回写/条目定位)在本域。批4 从 model/speech/mod.rs 归位(审核病一:
//! rerun 编排住在引擎层)。
//!
//! 用户定则： 重转是后台重算, 不是语音输入重放——不入主链队列交付、不弹 HUD。
use crate::model::{transcribe_session, AsrEvent, Cmd, EngineCtx, HandoffBox};

/// R1: 历史重转会话——与主链路同构(专用 rt + 引擎驱动), 但事件桥不同:
/// Result/NoSpeech/Error **直接回写该历史条目**(词典→可选 LLM→update_by_ts)。
pub fn retranscribe(
    app: tauri::AppHandle,
    provider: String,
    api_key: String,
    base_url: String,
    hotwords: Vec<String>,
    ts: String,
    handoff: HandoffBox,
    rx: tokio::sync::mpsc::Receiver<Cmd>,
) {
    crate::log::elog(&format!("[rerun] spawn provider={provider} key_len={}", api_key.len()));
    // 任务登记: Rerun 段入 core 队列(Transcribing 态, 自带会话不经 worker)。
    // 状态推进全收在本会话: 入队→结算→出队, 投影端(HUD/主窗)从 core 看全程。
    // ts 走显式参数(rerun_ts 字段), audio_path 回归纯语义(wav 路径)——
    // 不再用 R1 时期的 audio_path 双关
    let seg_id = crate::pipeline::segment_queue::submit_rerun(
        &ts,
        handoff.lock().unwrap().clone().unwrap_or_default(),
    );
    let ts_for_settle = ts.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rerun rt");
        // 出队保护: Drop guard 保证任何退出路径(正常/panic——2026-10-25 真机
        // block_on 嵌套 panic 曾让段永挂"重转中")都把段移出队列
        struct RerunOut(u64);
        impl Drop for RerunOut {
            fn drop(&mut self) { crate::pipeline::segment_queue::finish_rerun(self.0); }
        }
        let _out = RerunOut(seg_id);
        rt.block_on(async move {
        let ts = ts_for_settle;
        let app_settle = app.clone();
        let emit = move |ev: AsrEvent| match ev {
            AsrEvent::Partial(_) => {}
            AsrEvent::Result(raw) => {
                drop(handoff.lock().unwrap().take());
                // 结算在独立线程: rerun_settle_ok 里有 tauri::async_runtime::block_on
                // (LLM 调用), 而 emit 运行在引擎 runtime 的 async 线程——
                // 在 async 上下文里 block_on 会 panic "Cannot start a runtime
                // from within a runtime"(tokio 禁止), 真机已炸过一次
                let app2 = app_settle.clone();
                let ts2 = ts.clone();
                std::thread::spawn(move || rerun_settle_ok(&app2, &raw, &ts2));
            }
            AsrEvent::NoSpeech(m) => {
                drop(handoff.lock().unwrap().take());
                let app2 = app_settle.clone();
                let ts2 = ts.clone();
                std::thread::spawn(move || rerun_settle_fail(&app2, &ts2, &format!("重转未识别到内容: {m}")));
            }
            AsrEvent::Error(e) => {
                drop(handoff.lock().unwrap().take());
                let app2 = app_settle.clone();
                let ts2 = ts.clone();
                std::thread::spawn(move || rerun_settle_fail(&app2, &ts2, &format!("重转失败: {e}")));
            }
        };
        let ctx = EngineCtx { api_key, base_url, hotwords };
        transcribe_session(provider, ctx, rx, emit).await;
        // 出队由 _out guard 统一负责(含 panic 路径), 这里不再显式调
        });
    });
}

/// R1: 重转成功结算——词典纠错 → 可选 LLM(失败降级词典版, 与主链路 D1 一致) → 回写条目。
/// 完成后 bump history 版本(前端轮询刷新)。
fn rerun_settle_ok(app: &tauri::AppHandle, raw: &str, ts: &str) {
    let Ok(mut rec) = find_by_ts(app, &ts) else {
        crate::log::elog(&format!("[rerun] 回写失败: 未找到 ts={ts} 的条目"));
        return;
    };
    // 纠错管线与 Live 结算共用同一实现(model::refine); 重转无截图(事后屏幕会误导纠错)
    let cfg = crate::config::get_config(app.clone()).ok();
    let r = tauri::async_runtime::block_on(crate::model::refine(raw, cfg.as_ref(), None, None));
    rec.raw = raw.to_string();
    rec.final_text = r.text;
    rec.llm_used = r.llm_used;
    rec.llm_thinking = r.thinking;
    rec.warning = r.warning.map(|w| format!("重转 {w}, 已用词典纠错版"));
    match crate::pipeline::history::update_by_ts(app, &ts, &rec) {
        Ok(()) => {
            crate::core::bump_history();
        crate::log::elog(&format!("[rerun] 回写完成 ts={ts} llm_used={}", r.llm_used));
        }
        Err(e) => crate::log::elog(&format!("[rerun] 回写失败: {e}")),
    }
}

/// R1: 重转失败/未识别结算——原条目加 warning(原文保留, 用户可再试)
fn rerun_settle_fail(app: &tauri::AppHandle, ts: &str, msg: &str) {
    if let Ok(mut rec) = find_by_ts(app, ts) {
        rec.warning = Some(msg.to_string());
        if let Err(e) = crate::pipeline::history::update_by_ts(app, ts, &rec) {
            crate::log::elog(&format!("[rerun] 回写失败: {e}"));
            return;
        }
        crate::core::bump_history();
    }
    crate::log::elog(&format!("[rerun] {msg} ts={ts}"));
}

fn find_by_ts(app: &tauri::AppHandle, ts: &str) -> Result<crate::pipeline::history::HistoryRecord, String> {
    let path = crate::config::data_dir(app).join("history.jsonl");
    let recs = crate::pipeline::history::read_all_from(&path);
    if let Some(r) = recs.iter().find(|r| r.ts == ts) {
        return Ok(r.clone());
    }
    // 防御性兜底(真机 2026-10-25 出现过"文件里有这行但强类型解析后找不到"):
    // 用 Value 逐行匹配 ts, 绕过 HistoryRecord 字段级解析差异; 命中再转强类型
    if let Ok(raw) = std::fs::read_to_string(&path) {
        for line in raw.lines() {
            if !line.contains(ts) { continue; }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v["ts"].as_str() == Some(ts) {
                    if let Ok(r) = serde_json::from_str::<crate::pipeline::history::HistoryRecord>(line) {
                        crate::log::elog("[rerun] 强类型未命中但 Value 兜底命中(版本迁移期字段差异?)");
                        return Ok(r);
                    }
                }
            }
        }
    }
    let first = recs.first().map(|r| r.ts.clone()).unwrap_or_default();
    let last = recs.last().map(|r| r.ts.clone()).unwrap_or_default();
    crate::log::elog(&format!(
        "[rerun] find_by_ts 未命中: path={} 共{}条 首={} 尾={}",
        path.display(), recs.len(), first, last
    ));
    Err(format!("未找到 ts={ts}"))
}
