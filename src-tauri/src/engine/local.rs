// 本地 ASR 引擎（local-asr 网关的 WS 客户端）。
// 网关: research-plan/voice-mac-app/local-asr/gateway.py（mlx-qwen3-asr，
//       未来会拆成项目内的独立 package——用户定则：本地 ASR 与 APP 是两个部分）。
// 协议: OpenAI Realtime 事件面子集 + x-input-transcript.partial 扩展（见网关文件头）。
// 与 openai.rs 的三点差异：
//   1) 音频 16k pcm16 原样直传（网关固定 sample_rate=16000，不需要上采样）；
//   2) partial 是全量替换语义（滚动解码会回改近期文本，与豆包 partial 一致）；
//   3) completed.transcript 是整个会话的最终全量（不是按语音段），直接结算。
// 热词：首帧 append 携带 context（词典 term 顿号连接）——网关把它同时注入
// 流式解码与松手后的二遍全句重解，等于词典层直接进模型。
// 连接契约对齐 doubao：断连/收尾必须恰好一次 Result|NoSpeech|Error。
use super::contract::{AsrEvent, Cmd};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;

/// 本地网关引擎(单元标记: 挂 impl 用)
pub struct LocalGateway;

/// base_url 归一化：空 → 默认本机网关；无协议前缀补 ws://；去尾部斜杠
pub fn normalize_base_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.is_empty() {
        "ws://127.0.0.1:8300".to_string()
    } else if b.starts_with("ws://") || b.starts_with("wss://") {
        b.to_string()
    } else {
        format!("ws://{b}")
    }
}

/// 词典热词 → 网关 context 文本（偏置提示，模型侧对专有名词起效）
fn context_from_hotwords(hotwords: &[String]) -> String {
    let joined: Vec<&str> = hotwords.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if joined.is_empty() {
        String::new()
    } else {
        format!("相关术语：{}", joined.join("、"))
    }
}

impl<E: Fn(AsrEvent) + Send + 'static> super::AsrEngine<E> for LocalGateway {
async fn run(
    self,
    ctx: super::EngineCtx,
    rx: tokio::sync::mpsc::Receiver<Cmd>,
    emit: E,
) {
    let hotwords = ctx.hotwords;
    let base_url = ctx.base_url;
    let url = format!("{}/v1/realtime", normalize_base_url(&base_url));
    crate::log::elog(&format!("[local-asr] connecting {url}..."));
    let (ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(x) => x,
        Err(e) => {
            emit(AsrEvent::Error(format!(
                "本地 ASR 连接失败（网关起了吗？local-asr/gateway.py）: {e}"
            )));
            return;
        }
    };
    crate::log::elog("[local-asr] connected");
    let (mut sink, stream) = ws.split();
    // 首帧携带的 context（词典热词）；只发一次
    let context = context_from_hotwords(&hotwords);
    let mut first_feed = true;
    let mut finish_sent = false;

    // C37 骨架(与 doubao 共用 driver plumbing): 双输入转单通道, 主循环单 recv,
    // 收尾等待 watchdog(timeout_at)——select 双臂曾是此引擎的结构, 统一后
    // 静默挂死在结构上不可能(同一教训只修一家的问题就此了结)
    use super::driver::Inner;
    let (itx, mut irx) = tokio::sync::mpsc::channel::<Inner<tokio_tungstenite::tungstenite::Message>>(64);
    super::driver::forward_cmds(rx, itx.clone());
    super::driver::forward_ws(stream, itx);
    // Finish(commit) 后等最终结果的窗口: 网关二遍全句重解 30s 口述约 1-2s, 30s 只是异常兜底
    let mut watchdog: Option<tokio::time::Instant> = None;

    loop {
        let got = match watchdog {
            Some(d) => match tokio::time::timeout_at(d, irx.recv()).await {
                Ok(g) => g,
                Err(_) => {
                    emit(AsrEvent::Error("本地 ASR 收尾超时（未收到最终结果）".into()));
                    break;
                }
            },
            None => irx.recv().await,
        };
        match got {
            None => break,
            Some(Inner::Cmd(cmd)) => match cmd {
                Cmd::Abort => {
                    // 中途取消：告诉网关丢弃会话再关（不出结果，与 doubao Abort 同语义）
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"type": "response.cancel"}).to_string().into())).await;
                    let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
                    break;
                }
                Cmd::Feed(pcm16) => {
                    if finish_sent {
                        continue; // 收尾中迟到的分片不再投喂
                    }
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm16);
                    let msg = if first_feed {
                        first_feed = false;
                        json!({"type": "input_audio_buffer.append", "audio": b64, "context": context})
                    } else {
                        json!({"type": "input_audio_buffer.append", "audio": b64})
                    };
                    if sink.send(tokio_tungstenite::tungstenite::Message::Text(msg.to_string().into())).await.is_err() {
                        emit(AsrEvent::Error("本地 ASR 发送失败（连接断开）".into()));
                        break;
                    }
                }
                Cmd::Finish => {
                    crate::log::elog("[local-asr] finish: commit，等最终结果");
                    finish_sent = true;
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"type": "input_audio_buffer.commit"}).to_string().into())).await;
                    watchdog = Some(tokio::time::Instant::now() + Duration::from_secs(super::FINALIZE_TIMEOUT_SECS));
                }
            },
            Some(Inner::Ws(Some(m))) => {
                let tokio_tungstenite::tungstenite::Message::Text(t) = m else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                match v["type"].as_str().unwrap_or("") {
                    // 全量替换：partial.text 就是当前最优全文（不是增量）; 收尾后不再刷
                    "x-input-transcript.partial" => {
                        if let Some(text) = v["text"].as_str() {
                            if !finish_sent {
                                emit(AsrEvent::Partial(text.to_string()));
                            }
                        }
                    }
                    "conversation.item.input_audio_transcription.completed" => {
                        let t = v["transcript"].as_str().unwrap_or("");
                        super::settle(&emit, t);
                        let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
                        return;
                    }
                    "conversation.item.input_audio_transcription.failed" => {
                        let m = v["error"].as_str().unwrap_or("未知错误");
                        emit(AsrEvent::Error(format!("本地 ASR 转写失败: {m}")));
                        let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
                        return;
                    }
                    "error" => {
                        let m = v["message"].as_str().unwrap_or("未知错误");
                        crate::log::elog(&format!("[local-asr] error: {m}"));
                        emit(AsrEvent::Error(m.to_string()));
                        let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
                        return;
                    }
                    _ => {}
                }
            }
            Some(Inner::Ws(None)) => {
                // 断连: 收尾中=结果等不到了; 录音中=连接中断(旧"中途断连"语义)
                if finish_sent {
                    emit(AsrEvent::Error("本地 ASR 收尾中断（连接断开）".into()));
                } else {
                    emit(AsrEvent::Error("本地 ASR 连接中断，请重试".into()));
                }
                break;
            }
        }
    }
}
}

#[cfg(test)]
mod tests {
    use super::{context_from_hotwords, normalize_base_url};

    #[test]
    fn base_url_rules() {
        assert_eq!(normalize_base_url(""), "ws://127.0.0.1:8300");
        assert_eq!(normalize_base_url("  "), "ws://127.0.0.1:8300");
        assert_eq!(normalize_base_url("127.0.0.1:9000"), "ws://127.0.0.1:9000");
        assert_eq!(normalize_base_url("ws://a.b/"), "ws://a.b");
        assert_eq!(normalize_base_url("wss://a.b:9/x/"), "wss://a.b:9/x");
    }

    #[test]
    fn hotwords_context_rules() {
        assert_eq!(context_from_hotwords(&[]), "");
        assert_eq!(context_from_hotwords(&["  ".into()]), "");
        assert_eq!(
            context_from_hotwords(&["谢克数学".into(), "腾讯云".into()]),
            "相关术语：谢克数学、腾讯云"
        );
    }
}
