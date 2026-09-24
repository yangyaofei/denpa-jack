// 本地 ASR 引擎（local-asr 网关的 WS 客户端）。
// 网关: research-plan/voice-mac-app/local-asr/gateway.py（mlx-qwen3-asr 滚动解码，
//       未来会拆成项目内的独立 package——用户定则：本地 ASR 与 APP 是两个部分）。
// 协议: OpenAI Realtime 事件面子集 + x-input-transcript.partial 扩展（见网关文件头）。
// 与 openai_realtime.rs 的三点差异：
//   1) 音频 16k pcm16 原样直传（网关固定 sample_rate=16000，不需要上采样）；
//   2) partial 是全量替换语义（滚动解码会回改近期文本，与豆包 partial 一致）；
//   3) completed.transcript 是整个会话的最终全量（不是按语音段），直接结算。
// 连接契约对齐 doubao：断连/收尾必须恰好一次 Result|NoSpeech|Error。
use crate::doubao::{AsrEvent, Cmd};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;

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

pub async fn run_local_session(
    base_url: String,
    mut rx: tokio::sync::mpsc::Receiver<Cmd>,
    emit: impl Fn(AsrEvent),
) {
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
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Abort) => {
                    // 中途取消：告诉网关丢弃会话再关（不出结果，与 doubao Abort 同语义）
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"type": "response.cancel"}).to_string().into())).await;
                    let _ = sink.close().await;
                    break;
                }
                Some(Cmd::Feed(pcm16)) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm16);
                    let msg = json!({"type": "input_audio_buffer.append", "audio": b64});
                    if sink.send(tokio_tungstenite::tungstenite::Message::Text(msg.to_string().into())).await.is_err() {
                        emit(AsrEvent::Error("本地 ASR 发送失败（连接断开）".into()));
                        break;
                    }
                }
                Some(Cmd::Finish) => {
                    crate::log::elog("[local-asr] finish: commit，等最终结果");
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"type": "input_audio_buffer.commit"}).to_string().into())).await;
                    // 网关实测 commit→final 0.16–0.3s；30s 是异常兜底（与全引擎统一）
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(crate::engines::FINALIZE_TIMEOUT_SECS);
                    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, stream.next()).await {
                        let Ok(m) = msg else { break };
                        let tokio_tungstenite::tungstenite::Message::Text(t) = m else { continue };
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                        match v["type"].as_str().unwrap_or("") {
                            "conversation.item.input_audio_transcription.completed" => {
                                let t = v["transcript"].as_str().unwrap_or("");
                                crate::engines::settle(&emit, t);
                                let _ = sink.close().await;
                                return;
                            }
                            "conversation.item.input_audio_transcription.failed" => {
                                let m = v["error"].as_str().unwrap_or("未知错误");
                                emit(AsrEvent::Error(format!("本地 ASR 转写失败: {m}")));
                                let _ = sink.close().await;
                                return;
                            }
                            "error" => {
                                let m = v["message"].as_str().unwrap_or("未知错误");
                                emit(AsrEvent::Error(m.to_string()));
                                let _ = sink.close().await;
                                return;
                            }
                            _ => {}
                        }
                    }
                    emit(AsrEvent::Error("本地 ASR 收尾超时（未收到最终结果）".into()));
                    let _ = sink.close().await;
                    break;
                }
            },
            msg = stream.next() => {
                let Some(Ok(m)) = msg else {
                    // 未 Finish 的中途断连：报错（Finish 分支自己收尾后 return）
                    crate::log::elog("[local-asr] 中途断连(未 Finish): 报错");
                    emit(AsrEvent::Error("本地 ASR 连接中断，请重试".into()));
                    break;
                };
                let tokio_tungstenite::tungstenite::Message::Text(t) = m else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                match v["type"].as_str().unwrap_or("") {
                    // 全量替换：partial.text 就是当前最优全文（不是增量）
                    "x-input-transcript.partial" => {
                        if let Some(text) = v["text"].as_str() {
                            emit(AsrEvent::Partial(text.to_string()));
                        }
                    }
                    "error" => {
                        let m = v["message"].as_str().unwrap_or("未知错误");
                        crate::log::elog(&format!("[local-asr] error: {m}"));
                        emit(AsrEvent::Error(m.to_string()));
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_base_url;

    #[test]
    fn base_url_rules() {
        assert_eq!(normalize_base_url(""), "ws://127.0.0.1:8300");
        assert_eq!(normalize_base_url("  "), "ws://127.0.0.1:8300");
        assert_eq!(normalize_base_url("127.0.0.1:9000"), "ws://127.0.0.1:9000");
        assert_eq!(normalize_base_url("ws://a.b/"), "ws://a.b");
        assert_eq!(normalize_base_url("wss://a.b:9/x/"), "wss://a.b:9/x");
    }
}
