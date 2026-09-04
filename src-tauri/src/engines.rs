// 会话包装: 按 provider 分派引擎(豆包流式/智谱文件式), 引擎事件 → 前端事件。
// 交接箱与会话同生命周期: ctrl_start 建, stop 写入, 引擎 Result take——无全局会话状态
use crate::doubao;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// 对当前引擎发命令: 经 AppState 的会话镜像(转写中 abort / 测试通道用)
pub fn send_current(app: &tauri::AppHandle, cmd: doubao::Cmd) {
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
pub const FINALIZE_TIMEOUT_SECS: u64 = 3;

/// 结算契约(恰好一次): 空文本=没听清错误, 非空=Result——三引擎共用, 只写一次
pub fn settle(emit: &impl Fn(doubao::AsrEvent), text: &str) {
    if text.trim().is_empty() {
        emit(doubao::AsrEvent::Error("没听清(转写为空)".into()));
    } else {
        emit(doubao::AsrEvent::Result(text.to_string()));
    }
}

pub fn spawn_session(
    app: tauri::AppHandle,
    provider: String,
    api_key: String,
    hotwords: Vec<String>,
    handoff: HandoffBox,
    rx: tokio::sync::mpsc::Receiver<doubao::Cmd>,
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
        let emit = move |ev: doubao::AsrEvent| {
            let (tag, payload) = match ev {
                doubao::AsrEvent::Partial(t) => ("asr-partial", t),
                doubao::AsrEvent::Result(t) => {
                    // C36: 交付管线是同步阻塞活(AX/CGEvent/文件IO), 专用线程跑;
                    // FinishGuard 等价: panic 也兜底 asr-final, HUD 不卡死(Handy actions.rs:36-48)
                    let ho = handoff.lock().unwrap().take();
                    let app2 = app.clone();
                    let t2 = t.clone();
                    std::thread::spawn(move || {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            crate::pipeline::post_process(app2.clone(), t2.clone(), ho)
                        }));
                        if let Err(p) = r {
                            let msg = p
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| p.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "未知 panic".into());
                            crate::log::elog(&format!("[pipeline] panic: {msg}"));
                            let _ = tauri::Emitter::emit(&app2, "asr-final", serde_json::json!({
                                "raw": t2, "final": "", "llm_used": false,
                                "warning": format!("内部错误: {msg}")
                            }));
                        }
                    });
                    ("asr-result", t)
                }
                doubao::AsrEvent::Error(t) => {
                    // 失败也入历史(空文本+warning+音频路径): 重试链闭合(Handy actions.rs:858)
                    let app2 = app.clone();
                    let msg = t.clone();
                    std::thread::spawn(move || {
                        use tauri::Manager;
                        let audio_path = app2
                            .state::<std::sync::Mutex<crate::AppState>>()
                            .lock().unwrap().last_audio.clone().unwrap_or_default();
                        let rec = crate::history::HistoryRecord {
                            ts: crate::pipeline::now_local_pub(),
                            engine: String::new(),
                            raw: String::new(),
                            final_text: String::new(),
                            llm_used: false,
                            delivered: "none".into(),
                            audio_path,
                            warning: Some(msg),
                            duration_ms: 0,
                        };
                        crate::history::append(&app2, &rec);
                    });
                    ("asr-error", t)
                }
            };
            let _ = Emitter::emit(&app, tag, payload);
        };
        match provider.as_str() {
            "zhipu" => crate::zhipu_file::run_zhipu_session(api_key, hotwords, rx, emit).await,
            "openai" => crate::openai_realtime::run_openai_session(api_key, hotwords, rx, emit).await,
            _ => doubao::run_session(api_key, hotwords, rx, emit).await,
        }
        });
    });
}
