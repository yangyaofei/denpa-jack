// 引擎域: 契约(contract.rs) + 三引擎(doubao/zhipu/local) + 会话调度(本文件)。
// 引擎事件 → 前端事件; 交接箱与会话同生命周期: ctrl_start 建, stop 写入, 引擎 Result take——无全局会话状态
// (D9: OpenAI Realtime 引擎已移除——从未实测且收尾固定等满 30s; 协议子集由本地网关 local.rs 使用)
mod contract;
mod doubao;
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
/// "哪个引擎可用什么参数跑"是引擎域的事(engine→settings/local_service 单向依赖)。
pub fn resolve_asr(cfg: &crate::config::Config) -> Result<(crate::config::AsrProfile, String), String> {
    // 内置本地引擎: type/name 定死的特化 provider(2026-10-08 用户定则)——
    // 不占档案列表、零配置, 地址来自 local_service 登记的随机端口(联动)
    if cfg.active_asr_id == crate::local_service::BUILTIN_ID {
        let base = crate::local_service::builtin_base_url()?;
        let p = crate::config::AsrProfile {
            id: crate::local_service::BUILTIN_ID.into(),
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
        match provider.as_str() {
            "zhipu" => zhipu::Zhipu.run(ctx, rx, emit).await,
            // 本地网关(local-asr/gateway.py): 无鉴权, 走 base_url;
            // 热词经首帧 context 注入(网关同时用于流式与二遍全句重解)
            "local" => local::LocalGateway.run(ctx, rx, emit).await,
            _ => doubao::Doubao.run(ctx, rx, emit).await,
        }
        });
    });
}
