// OpenAI Realtime 流式转写引擎(gpt-4o-mini-transcribe)
// 协议: WSS JSON 事件; 音频 pcm16(16k 上采样 24k); 按键定则——Finish 立即结算累积文本
use crate::doubao::{AsrEvent, Cmd};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;

pub async fn run_openai_session(
    api_key: String,
    _hotwords: Vec<String>, // OpenAI 转写无热词参数(可用 prompt 但 realtime 会话不支持)
    mut rx: tokio::sync::mpsc::Receiver<Cmd>,
    emit: impl Fn(AsrEvent),
) {
    let url = "wss://api.openai.com/v1/realtime?model=gpt-4o-mini-transcribe";
    let req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("OpenAI-Beta", "realtime=v1")
        .body(())
        .unwrap();
    crate::log::elog("[openai-rt] connecting...");
    let (ws, _) = match tokio_tungstenite::connect_async(req).await {
        Ok(x) => x,
        Err(e) => {
            emit(AsrEvent::Error(format!("OpenAI Realtime 连接失败: {e}")));
            return;
        }
    };
    crate::log::elog("[openai-rt] connected");
    let (mut sink, mut stream) = ws.split();

    // 会话配置: 纯转写(modalities=text), pcm16
    let session = json!({
        "type": "session.update",
        "session": {
            "modalities": ["text"],
            "input_audio_format": "pcm16",
            "turn_detection": null
        }
    });
    if sink.send(tokio_tungstenite::tungstenite::Message::Text(session.to_string().into())).await.is_err() {
        emit(AsrEvent::Error("session.update 发送失败".into()));
        return;
    }

    let mut transcript = String::new(); // 累积(VAD 分段各自 completed)
    let mut done = false;
    let mut last_partial = String::new();

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Abort) => {
                    let _ = sink.close().await;
                    break;
                }
                Some(Cmd::Feed(pcm16)) => {
                    if done {
                        continue;
                    }
                    // 16k → 24k 上采样(线性插值, i16le)
                    let up = upsample_16k_to_24k(&pcm16);
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&up);
                    let msg = json!({"type": "input_audio_buffer.append", "audio": b64});
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(msg.to_string().into())).await;
                }
                Some(Cmd::Finish) => {
                    if done {
                        continue;
                    }
                    done = true;
                    crate::log::elog("[openai-rt] finish: 按键定则——立即结算");
                    let _ = sink.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"type": "input_audio_buffer.commit"}).to_string().into())).await;
                    // 给服务端一个短窗口收最后一段 completed(转写通常 <1.5s), 到时或收到即结算
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(crate::engines::FINALIZE_TIMEOUT_SECS);
                    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, stream.next()).await {
                        let Ok(m) = msg else { break };
                        let tokio_tungstenite::tungstenite::Message::Text(t) = m else { continue };
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                        match v["type"].as_str().unwrap_or("") {
                            "conversation.item.input_audio_transcription.completed" => {
                                if let Some(seg) = v["transcript"].as_str() {
                                    transcript.push_str(seg);
                                }
                            }
                            "response.audio_transcript.delta" => {
                                if let Some(d) = v["delta"].as_str() {
                                    last_partial.push_str(d);
                                }
                            }
                            "error" => break,
                            _ => {}
                        }
                    }
                    let t = if !transcript.trim().is_empty() {
                        transcript
                    } else {
                        last_partial
                    };
                    settle(&emit, &t);
                    let _ = sink.close().await;
                    break;
                }
            },
            msg = stream.next() => {
                let Some(Ok(m)) = msg else {
                    // 契约对齐 doubao: 断连必须恰好一次 Result|Error。
                    // Finish 分支自己收尾后 break(done=true), 此处到达即未 Finish 的中途断连
                    if !done {
                        done = true;
                        crate::log::elog("[openai-rt] 中途断连(未 Finish): 报错");
                        emit(AsrEvent::Error("连接中断, 请重试".into()));
                    }
                    break;
                };
                let tokio_tungstenite::tungstenite::Message::Text(t) = m else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                match v["type"].as_str().unwrap_or("") {
                    "conversation.item.input_audio_transcription.completed" => {
                        if let Some(seg) = v["transcript"].as_str() {
                            transcript.push_str(seg);
                            emit(AsrEvent::Partial(transcript.clone()));
                        }
                    }
                    "response.audio_transcript.delta" => {
                        if let Some(d) = v["delta"].as_str() {
                            last_partial.push_str(d);
                            emit(AsrEvent::Partial(format!("{}{}", transcript, last_partial)));
                        }
                    }
                    "input_audio_buffer.speech_started" => {
                        last_partial.clear();
                    }
                    "error" => {
                        let m = v["error"]["message"].as_str().unwrap_or("未知错误");
                        crate::log::elog(&format!("[openai-rt] error: {m}"));
                        if !done {
                            done = true;
                            emit(AsrEvent::Error(m.to_string()));
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn settle(emit: &impl Fn(AsrEvent), text: &str) {
    if text.trim().is_empty() {
        emit(AsrEvent::Error("没听清(转写为空)".into()));
    } else {
        emit(AsrEvent::Result(text.to_string()));
    }
}

/// 16k mono i16le → 24k(线性插值, 每样本 1.5 倍)
fn upsample_16k_to_24k(pcm: &[u8]) -> Vec<u8> {
    let n = pcm.len() / 2;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        samples.push(i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]));
    }
    let mut out = Vec::with_capacity((n as f64 * 1.5) as usize * 2);
    for i in 0..n.saturating_sub(1) {
        let a = samples[i] as i32;
        let b = samples[i + 1] as i32;
        out.extend_from_slice(&(a as i16).to_le_bytes());
        out.extend_from_slice(&(((a * 2 + b) / 3) as i16).to_le_bytes()); // 1.5x: 0.5 + 1.0 位置
    }
    if n > 0 {
        out.extend_from_slice(&samples[n - 1].to_le_bytes());
    }
    out
}
