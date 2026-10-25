// 引擎域: 契约(contract.rs) + 三引擎(doubao/zhipu/local) + 会话调度(本文件)。
// 引擎事件 → 前端事件; 交接箱与会话同生命周期: ctrl_start 建, stop 写入, 引擎 Result take——无全局会话状态
// (D9: OpenAI Realtime 引擎已移除——从未实测且收尾固定等满 30s; 协议子集由本地网关 local.rs 使用)
mod contract;
mod doubao;
/// 内置网关子进程生命周期管理(denpa-asr): 随机端口登记/探活收尸/两段式停止/日志尾部。
/// 与 local.rs(WS 客户端引擎)配对——客户端连的就是这个网关。
pub mod gateway;
mod local;
mod zhipu;

pub use contract::{AsrEvent, Cmd};
use std::sync::{Arc, Mutex};

/// 引擎会话上下文: spawn_session 解析好的输入, 引擎各自取所需
/// (zhipu 无热词支持, local 无鉴权走 base_url——不用的字段以 _ 标注)
pub struct EngineCtx {
    pub api_key: String,
    pub base_url: String,
    pub hotwords: Vec<String>,
}

/// 引擎统一接口(阶段A 接口收敛): 每个引擎一个会话运行器,
/// 生命线 = 连接 → Feed* → Finish → 恰好一次结算(Result | NoSpeech | Error)。
/// 骨架差异(watchdog vs select vs 同步累积)是各引擎实测调过的行为, 阶段B 再统一。
#[allow(async_fn_in_trait)] // 仅 crate 内静态分派, 不需要 Send 自动 trait
pub trait AsrEngine<E: Fn(AsrEvent) + Send + 'static> {
    async fn run(self, ctx: EngineCtx, rx: tokio::sync::mpsc::Receiver<Cmd>, emit: E);
}

/// 对当前引擎发命令: 经 AppState 的会话镜像(转写中 abort / 测试通道用)
pub fn send_current(app: &tauri::AppHandle, cmd: Cmd) {
    use tauri::Manager;
    if let Some(tx) = app
        .state::<std::sync::Mutex<crate::AppState>>()
        .lock()
        .unwrap()
        .engine_mirror
        .as_ref()
    {
        let _ = tx.try_send(cmd);
    }
}

pub type HandoffBox = Arc<Mutex<Option<crate::pipeline::SessionHandoff>>>;

/// 引擎收尾等待窗口(松开后等服务端最终结果的兜底上限)——全引擎统一
pub const FINALIZE_TIMEOUT_SECS: u64 = 30;

/// 豆包收尾等待窗口: Finish 后此秒数内无最终帧则用已收文本结算
pub const DOUBAO_FINALIZE_TIMEOUT_SECS: u64 = 3;

/// 结算契约(恰好一次): 空文本=没听清(NoSpeech, 不是失败), 非空=Result。
/// local 收尾调用此处; doubao 因需记录文本长度日志保留本地实现(见 doubao::settle)。
pub fn settle(emit: &impl Fn(AsrEvent), text: &str) {
    if text.trim().is_empty() {
        emit(AsrEvent::NoSpeech("没听清(转写为空)".into()));
    } else {
        emit(AsrEvent::Result(text.to_string()));
    }
}

/// ASR 档案解析: 从配置选出本会话引擎与 key——从 recording.rs 归位(阶段A 第2步),
/// "哪个引擎可用什么参数跑"是引擎域的事(engine→config/gateway 单向依赖)。
pub fn resolve_asr(cfg: &crate::config::Config) -> Result<(crate::config::AsrProfile, String), String> {
    // 内置本地引擎: type/name 定死的特化 provider(2026-10-08 用户定则)——
    // 不占档案列表、零配置, 地址来自 gateway 登记的随机端口(联动)
    if cfg.active_asr_id == crate::engine::gateway::BUILTIN_ID {
        let base = crate::engine::gateway::builtin_base_url()?;
        let p = crate::config::AsrProfile {
            id: crate::engine::gateway::BUILTIN_ID.into(),
            name: "内置本地引擎".into(),
            provider: "local".into(),
            api_key: String::new(),
            base_url: base,
        };
        return Ok((p, String::new()));
    }
    let profile = cfg
        .asr_profiles
        .iter()
        .find(|p| p.id == cfg.active_asr_id)
        .cloned()
        .or_else(|| cfg.asr_profiles.first().cloned())
        .ok_or("没有 ASR 档案, 请先在设置中添加")?;
    if !["volcengine", "zhipu", "local"].contains(&profile.provider.as_str()) {
        return Err(format!("未知引擎: {}", profile.provider));
    }
    // 本地/内置网关无鉴权：不需要 Key(空 Key 池也能用)
    if profile.provider == "local" {
        return Ok((profile, String::new()));
    }
    let key = if profile.api_key.is_empty() {
        cfg.keys.first().cloned().ok_or("没有 API Key")?
    } else {
        profile.api_key.clone()
    };
    Ok((profile, key))
}

#[cfg(test)]
mod tests {
    use super::resolve_asr;

    #[test]
    fn resolve_asr_rejects_unknown_provider() {
        let mut cfg = crate::config::default_config();
        cfg.active_asr_id = "x".into();
        cfg.asr_profiles = vec![crate::config::AsrProfile {
            id: "x".into(), name: "x".into(), provider: "bogus".into(), api_key: "k".into(), base_url: String::new(),
        }];
        assert!(resolve_asr(&cfg).is_err());
    }
    #[test]
    fn resolve_asr_falls_back_to_first_profile() {
        let mut cfg = crate::config::default_config();
        cfg.active_asr_id = "nope".into();
        cfg.asr_profiles = vec![crate::config::AsrProfile {
            id: "p1".into(), name: "豆包".into(), provider: "volcengine".into(), api_key: "k1".into(), base_url: String::new(),
        }];
        let (p, k) = resolve_asr(&cfg).unwrap();
        assert_eq!(p.id, "p1");
        assert_eq!(k, "k1");
    }
    #[test]
    fn resolve_asr_empty_key_uses_pool() {
        let mut cfg = crate::config::default_config();
        cfg.keys = vec!["pool-key".into()];
        cfg.asr_profiles = vec![crate::config::AsrProfile {
            id: "p1".into(), name: "n".into(), provider: "zhipu".into(), api_key: "".into(), base_url: String::new(),
        }];
        let (_, k) = resolve_asr(&cfg).unwrap();
        assert_eq!(k, "pool-key");
    }
}

/// R1: 历史重转专用会话——与主链路同构(专用 rt + 引擎 run), 但事件桥不同:
/// Result/NoSpeech/Error **直接回写该历史条目**(词典→可选 LLM→update_by_ts),
/// 不入队、不交付、不碰 HUD(用户定则: 重转是后台重算, 不是语音输入重放)。
pub fn spawn_rerun_session(
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
    let seg_id = crate::segment_queue::enqueue_rerun(
        &ts,
        handoff.lock().unwrap().clone().unwrap_or_default(),
    );
    let ts_for_settle = ts.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rerun rt");
        rt.block_on(async move {
        let ts = ts_for_settle;
        let emit = move |ev: AsrEvent| match ev {
            AsrEvent::Partial(_) => {}
            AsrEvent::Result(raw) => {
                drop(handoff.lock().unwrap().take());
                rerun_settle_ok(&app, &raw, &ts);
            }
            AsrEvent::NoSpeech(m) => {
                drop(handoff.lock().unwrap().take());
                rerun_settle_fail(&app, &ts, &format!("重转未识别到内容: {m}"));
            }
            AsrEvent::Error(e) => {
                drop(handoff.lock().unwrap().take());
                rerun_settle_fail(&app, &ts, &format!("重转失败: {e}"));
            }
        };
        let ctx = EngineCtx { api_key, base_url, hotwords };
        run_engine(provider, ctx, rx, emit).await;
        crate::segment_queue::finish_rerun(seg_id);
        });
    });
}

/// 引擎分派(spawn_session / spawn_rerun_session 共用)
async fn run_engine<E: Fn(AsrEvent) + Send + 'static>(provider: String, ctx: EngineCtx, rx: tokio::sync::mpsc::Receiver<Cmd>, emit: E) {
    match provider.as_str() {
        "zhipu" => zhipu::Zhipu.run(ctx, rx, emit).await,
        // 本地网关(local-asr/gateway.py): 无鉴权, 走 base_url;
        // 热词经首帧 context 注入(网关同时用于流式与二遍全句重解)
        "local" => local::LocalGateway.run(ctx, rx, emit).await,
        _ => doubao::Doubao.run(ctx, rx, emit).await,
    }
}

/// R1: 重转成功结算——词典纠错 → 可选 LLM(失败降级词典版, 与主链路 D1 一致) → 回写条目。
/// 完成后 bump history 版本(前端轮询刷新)。
fn rerun_settle_ok(app: &tauri::AppHandle, raw: &str, ts: &str) {
    let Ok(mut rec) = find_by_ts(app, &ts) else {
        crate::log::elog(&format!("[rerun] 回写失败: 未找到 ts={ts} 的条目"));
        return;
    };
    let cfg = crate::config::get_config(app.clone()).ok();
    let dict = cfg.as_ref().map(|c| crate::dict::TextCorrector {
        entries: c.dict.clone(),
        normalizations: c.normalizations.iter().map(|r| (r.pattern.clone(), r.replacement.clone())).collect(),
    });
    let (dict_text, _) = dict.as_ref().map(|d| d.correct(raw)).unwrap_or((raw.to_string(), Vec::new()));
    let mut final_text = dict_text.clone();
    let mut llm_used = false;
    let mut thinking: Option<String> = None;
    let mut warning: Option<String> = None;
    if let Some(c) = cfg.as_ref().filter(|c| c.use_llm_correction) {
        if let Some(p) = c.llm_profiles.iter().find(|p| p.id == c.active_llm_id).cloned() {
            let key = if p.api_key.is_empty() { c.keys.first().cloned().unwrap_or_default() } else { p.api_key.clone() };
            let opts = crate::llm::LlmOpts {
                provider: p.provider.clone(),
                base_url: p.base_url.clone(),
                model: p.model.clone(),
                api_key: key,
                prompt: if p.prompt.is_empty() { crate::llm::DEFAULT_LLM_PROMPT.to_string() } else { p.prompt.clone() },
                thinking: p.thinking,
                effort: if p.effort.is_empty() { "low".into() } else { p.effort.clone() },
                timeout_secs: c.llm_timeout_secs,
                max_tokens: c.llm_max_tokens,
                retries: c.llm_retries,
            };
            let terms: Vec<String> = c.dict.iter().map(|d| d.term.clone()).collect();
            match tauri::async_runtime::block_on(crate::llm::polish(&dict_text, &terms, &opts, None)) {
                Ok(out) => {
                    final_text = out.text;
                    thinking = out.thinking;
                    llm_used = true;
                }
                Err(e) => warning = Some(format!("重转 LLM 失败, 已交付词典版: {e}")),
            }
        }
    }
    rec.raw = raw.to_string();
    rec.final_text = final_text;
    rec.llm_used = llm_used;
    rec.llm_thinking = thinking;
    rec.warning = warning;
    match crate::history::update_by_ts(app, &ts, &rec) {
        Ok(()) => {
            crate::core::bump_history();
            crate::log::elog(&format!("[rerun] 回写完成 ts={ts} llm_used={llm_used}"));
        }
        Err(e) => crate::log::elog(&format!("[rerun] 回写失败: {e}")),
    }
}

/// R1: 重转失败/未识别结算——原条目加 warning(原文保留, 用户可再试)
fn rerun_settle_fail(app: &tauri::AppHandle, ts: &str, msg: &str) {
    if let Ok(mut rec) = find_by_ts(app, ts) {
        rec.warning = Some(msg.to_string());
        if let Err(e) = crate::history::update_by_ts(app, ts, &rec) {
            crate::log::elog(&format!("[rerun] 回写失败: {e}"));
            return;
        }
        crate::core::bump_history();
    }
    crate::log::elog(&format!("[rerun] {msg} ts={ts}"));
}

fn find_by_ts(app: &tauri::AppHandle, ts: &str) -> Result<crate::history::HistoryRecord, String> {
    let recs = crate::history::read_all_from(&crate::config::data_dir(app).join("history.jsonl"));
    recs.into_iter().find(|r| r.ts == ts).ok_or_else(|| format!("未找到 ts={ts}"))
}

pub fn spawn_session(
    app: tauri::AppHandle,
    provider: String,
    api_key: String,
    base_url: String,
    hotwords: Vec<String>,
    handoff: HandoffBox,
    rx: tokio::sync::mpsc::Receiver<Cmd>,
) {
    crate::log::elog(&format!("[session] spawn provider={provider} key_len={} hotwords={}", api_key.len(), hotwords.len()));
    // C35: 会话跑在专用 tokio runtime 的独立线程——实测挂在 tauri::async_runtime 的任务
    // 会整体停止被 poll(收尾三 arm 含 3s watchdog 全静默), 与 tauri runtime 调度隔离
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("session rt");
        rt.block_on(async move {
        // 统一事件桥: Result 触发后处理(纠错→LLM→交付→历史), 交接数据从会话箱 take
        let emit = move |ev: AsrEvent| {
            let (tag, payload) = match ev {
                AsrEvent::Partial(t) => ("asr-partial", t),
                AsrEvent::Result(t) => {
                    // 分段队列(用户定则): 结果不在这里直接跑后处理, 而是入队, 由队列的单
                    // worker 串行执行 —— 交付顺序恒等于录音顺序; 同时避免并行段互相结算
                    // 全局单槽的粘贴事务(paste_tx::PENDING)导致前一段只进剪贴板。
                    let ho = handoff.lock().unwrap().take();
                    let id = crate::segment_queue::enqueue(&app, t.clone(), ho);
                    crate::log::elog(&format!("[queue] asr-result 入队 id={id} queue_busy={}", crate::segment_queue::busy()));
                    ("asr-result", t)
                }
                AsrEvent::NoSpeech(t) => {
                    // 空转写(没听清/全程没声音): 不是失败 —— 只写一条历史 + 浮窗提示 2.5s 后收起,
                    // 不进队列、不留常驻状态。旧实现(1cbc9d8^ engines.rs:85)就是这个语义；
                    // 队列化时误把它当失败段, 导致每次空录音都在浮窗留一个"识别失败"永久挂着(issue #6)。
                    let ho = handoff.lock().unwrap().take();
                    let app2 = app.clone();
                    let msg = t.clone();
                    std::thread::spawn(move || {
                        let _ = crate::segment_queue::record_no_speech(&app2, ho, &msg);
                    });
                    // 提示走 msg 通道(浮窗按一次性提示显示 2.5s), 不走 err(那是失败条)
                    ("asr-nospeech", t)
                }
                AsrEvent::Error(t) => {
                    // 真失败(连接失败/请求失败/中断): 该段以 Failed 状态入队(先落盘音频 + 写历史,
                    // 保证能重试)。与 NoSpeech 的区别: 这里可能有已识别的文本需要救, 才值得常驻可重试
                    let ho = handoff.lock().unwrap().take();
                    let app2 = app.clone();
                    let msg = t.clone();
                    let id = crate::segment_queue::enqueue_asr_failed(&app2, msg, ho);
                    crate::log::elog(&format!("[queue] asr-error 失败段入队 id={id}"));
                    ("asr-error", t)
                }
            };
            // HUD 唯一入口: 这里只报事实(回显/提示/错误), 显示成什么样由 hud.rs 决定。
            // 旧写法是各层直接改同一份共享快照, 已全部收敛到 hud 模块的接口。
            match tag {
                "asr-partial" => crate::core::session_partial(&payload),
                "asr-nospeech" => crate::hud::no_speech(&app, &payload),
                "asr-error" => crate::hud::error(&app, &payload),
                // asr-result: 队列/阶段由 hud 的 poll 现算, 不需要在这里写状态
                _ => {}
            }
        };
        let ctx = EngineCtx { api_key, base_url, hotwords };
        run_engine(provider, ctx, rx, emit).await;
        });
    });
}
