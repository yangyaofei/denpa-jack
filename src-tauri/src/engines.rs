// 会话包装: 按 provider 分派引擎(豆包流式/智谱文件式), 引擎事件 → 前端事件。
// 交接箱与会话同生命周期: ctrl_start 建, stop 写入, 引擎 Result take——无全局会话状态
use crate::doubao;
use std::sync::{Arc, Mutex};

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
pub const FINALIZE_TIMEOUT_SECS: u64 = 30;

/// 豆包收尾等待窗口: Finish 后此秒数内无最终帧则用已收文本结算
pub const DOUBAO_FINALIZE_TIMEOUT_SECS: u64 = 3;

/// 结算契约(恰好一次): 空文本=没听清(NoSpeech, 不是失败), 非空=Result。
/// OpenAI Realtime 收尾调用此处; doubao 因需记录文本长度日志保留本地实现(见 doubao::settle)。
pub fn settle(emit: &impl Fn(doubao::AsrEvent), text: &str) {
    if text.trim().is_empty() {
        emit(doubao::AsrEvent::NoSpeech("没听清(转写为空)".into()));
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
                    // 分段队列(用户定则): 结果不在这里直接跑后处理, 而是入队, 由队列的单
                    // worker 串行执行 —— 交付顺序恒等于录音顺序; 同时避免并行段互相结算
                    // 全局单槽的粘贴事务(paste_tx::PENDING)导致前一段只进剪贴板。
                    let ho = handoff.lock().unwrap().take();
                    let id = crate::segment_queue::enqueue(&app, t.clone(), ho);
                    crate::log::elog(&format!("[queue] asr-result 入队 id={id} queue_busy={}", crate::segment_queue::busy()));
                    ("asr-result", t)
                }
                doubao::AsrEvent::NoSpeech(t) => {
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
                doubao::AsrEvent::Error(t) => {
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
                "asr-partial" => crate::hud::partial(&app, &payload),
                "asr-nospeech" => crate::hud::no_speech(&app, &payload),
                "asr-error" => crate::hud::error(&app, &payload),
                // asr-result: 队列/阶段由 hud 的 poll 现算, 不需要在这里写状态
                _ => {}
            }
        };
        match provider.as_str() {
            "zhipu" => crate::zhipu_file::run_zhipu_session(api_key, hotwords, rx, emit).await,
            "openai" => crate::openai_realtime::run_openai_session(api_key, hotwords, rx, emit).await,
            _ => doubao::run_session(api_key, hotwords, rx, emit).await,
        }
        });
    });
}
