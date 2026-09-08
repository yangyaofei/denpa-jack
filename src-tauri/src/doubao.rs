// 豆包流式 ASR 引擎(Rust 移植自 Swift DoubaoEngine)
// 生命线: start → feed → finish → 恰好一次 result/error 事件
// 约束: C16 连接关闭属正常路径由状态判定; C20(废止) definite 不结算——交付只认按键松开; C21 finish 幂等
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::io::Write;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub enum Cmd {
    Feed(Vec<u8>),
    Finish,
    Abort,
}

#[derive(Debug, Clone)]
pub enum AsrEvent {
    Partial(String),
    Result(String),
    Error(String),
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::fast());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

fn frame(header: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut f = header.to_vec();
    f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// 帧解析(自校正: header+可选seq+size+payload 恰好等于帧长, 杜绝越界)
fn parse_frame(data: &[u8]) -> Option<(u8, u8, Vec<u8>)> {
    if data.len() < 8 {
        return None;
    }
    let b1 = data[1];
    let b2 = data[2];
    let typ = (b1 >> 4) & 0xF;
    let flags = b1 & 0xF;
    let comp = b2 & 0xF;
    let extract = |with_seq: bool| -> Option<Vec<u8>> {
        let base = 4 + if with_seq { 4 } else { 0 };
        if data.len() < base + 4 {
            return None;
        }
        let size = u32::from_be_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]])
            as usize;
        if base + 4 + size != data.len() {
            return None;
        }
        Some(data[base + 4..].to_vec())
    };
    let has_seq = flags == 0b0001 || flags == 0b0011;
    let body = extract(has_seq).or_else(|| extract(!has_seq))?;
    Some((typ, comp, body))
}

fn settle(emit: &dyn Fn(AsrEvent), text: &str) {
    crate::log::elog(&format!("[doubao] settle: len={}", text.chars().count()));
    if text.is_empty() {
        emit(AsrEvent::Error("没听清(转写为空)".into()));
    } else {
        emit(AsrEvent::Result(text.to_string()));
    }
}

pub async fn run_session(
    api_key: String,
    hotwords: Vec<String>,
    mut rx: mpsc::Receiver<Cmd>,
    emit: impl Fn(AsrEvent) + Send + 'static,
) {
    let req_id = uuid::Uuid::new_v4().to_string();
    let req = http::Request::builder()
        .method("GET")
        .uri("wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async")
        .header("Host", "openspeech.bytedance.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", ws_key())
        .header("X-Api-Key", &api_key)
        .header("X-Api-Resource-Id", "volc.seedasr.sauc.duration")
        .header("X-Api-Request-Id", &req_id)
        .header("X-Api-Sequence", "-1")
        .body(())
        .unwrap();

    crate::log::elog("[doubao] connecting...");
    for (k, v) in req.headers() {
        crate::log::elog(&format!("[doubao] hdr {k}: {}", String::from_utf8_lossy(v.as_bytes())));
    }
    let (ws, _) = match tokio_tungstenite::connect_async(req).await {
        Ok(x) => {
            crate::log::elog("[doubao] ws connected");
            x
        }
        Err(e) => {
            emit(AsrEvent::Error(format!("WS 连接失败: {e}")));
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();

    // 全量请求帧(type=full-client ser=json comp=gzip)
    let mut request = json!({
        "user": {"uid": format!("voicemac-{}", uuid::Uuid::new_v4())},
        "audio": {"format": "pcm", "rate": 16000, "bits": 16, "channel": 1},
        "request": {"model_name": "bigmodel", "enable_punc": true, "enable_itn": true, "enable_ddc": true, "enable_nonstream": true}
    });
    if !hotwords.is_empty() {
        // 热词直传: 有就传, 没有就不带 corpus(直传优先级高于词表, 本 app 不用词表)
        let ctx = json!({"hotwords": hotwords.iter().map(|w| json!({"word": w})).collect::<Vec<_>>()});
        request["request"]["corpus"] = json!({"context": ctx.to_string()});
    }
    crate::log::elog(&format!("[doubao] request body: {}", request));
    let full = frame([0x11, 0x10, 0x11, 0x00], &gzip(request.to_string().as_bytes()));
    if sink.send(Message::Binary(full.into())).await.is_err() {
        emit(AsrEvent::Error("WS 请求发送失败".into()));
        return;
    }

    // C37: 会话循环重构——单通道 + timeout_at(替代三路 select)
    // 实测三路 select 在 finish 后连必到的 watchdog 都静默(疑与分支 poll 有关), 改为:
    //   stream 读取拆独立 task 转发进统一通道; 主循环单 recv, watchdog 用 timeout_at 包裹
    // C33: 豆包 2.0 的 partial/definite text 均为会话全量文本——单变量替换, 禁止拼接
    enum Inner {
        Cmd(Cmd),
        Ws(Option<Message>), // None = 流结束/断连
    }
    let latest_text = String::new();
    let mut latest_text = latest_text;
    let mut done = false;
    let mut finish_sent = false;
    let mut fed_once = false;
    let mut watchdog: Option<tokio::time::Instant> = None;

    let (itx, mut irx) = mpsc::channel::<Inner>(64);
    // 外部命令转发
    let itx2 = itx.clone();
    tokio::spawn(async move {
        while let Some(c) = rx.recv().await {
            if itx2.send(Inner::Cmd(c)).await.is_err() {
                break;
            }
        }
    });
    // WS 读取转发(独立 task)
    let itx3 = itx.clone();
    tokio::spawn(async move {
        loop {
            match stream.next().await {
                Some(Ok(m)) => {
                    if itx3.send(Inner::Ws(Some(m))).await.is_err() {
                        break;
                    }
                }
                _ => {
                    let _ = itx3.send(Inner::Ws(None)).await;
                    break;
                }
            }
        }
    });

    loop {
        let got = match watchdog {
            Some(d) => match tokio::time::timeout_at(d, irx.recv()).await {
                Ok(g) => g,
                Err(_) => {
                    crate::log::elog("[doubao] watchdog 触发(收尾超时兜底)");
                    if !done {
                        done = true;
                        let t = latest_text.clone();
                        settle(&emit, &t);
                    }
                    break;
                }
            },
            None => irx.recv().await,
        };
        match got {
            None => break,
            Some(Inner::Cmd(cmd)) => match cmd {
                Cmd::Abort => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
                Cmd::Feed(pcm) => {
                    if !fed_once {
                        fed_once = true;
                        crate::log::elog(&format!("[doubao] first feed {}B", pcm.len()));
                    }
                    if !done && !finish_sent {
                        let f = frame([0x11, 0x20, 0x01, 0x00], &gzip(&pcm));
                        let _ = sink.send(Message::Binary(f.into())).await;
                    }
                }
                Cmd::Finish => {
                    if !done && !finish_sent {
                        finish_sent = true;
                        crate::log::elog("[doubao] finish: 停音频+收尾帧; 等服务端最终结果(3s 兜底)");
                        let f = frame([0x11, 0x22, 0x01, 0x00], &gzip(b""));
                        // C34: 长录音的收尾帧发送可能被 TCP 背压卡死 → 1s 超时
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(1),
                            sink.send(Message::Binary(f.into())),
                        )
                        .await;
                        watchdog = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(3));
                        crate::log::elog("[doubao] finish 分支完成, 进等待");
                    }
                }
            },
            Some(Inner::Ws(msg)) => {
                let Some(m) = msg else {
                    // 流结束/断连(C16: definite 后属正常收尾)
                    crate::log::elog(&format!("[doubao] stream 结束: finish_sent={finish_sent}"));
                    if !done {
                        done = true;
                        if finish_sent {
                            settle(&emit, &latest_text);
                        } else {
                            crate::log::elog("[doubao] 中途断连(未 Finish): 不交付, 报错");
                            emit(AsrEvent::Error("连接中断, 请重试".into()));
                        }
                    }
                    break;
                };
                let Message::Binary(d) = m else { continue };
                let body = match parse_frame(&d) {
                    Some((_, _, b)) => b,
                    None => continue,
                };
                let typ = d[1] >> 4;
                if typ == 0b1111 {
                    // 错误帧: 解析成功取 message, 解析失败也不许静默——契约要求恰好一次 Result|Error
                    let m = serde_json::from_slice::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|obj| obj["message"].as_str().or(obj["error"]["message"].as_str()).map(|s| s.to_string()))
                        .unwrap_or_else(|| format!("错误帧(无法解析): {}", String::from_utf8_lossy(&body)));
                    crate::log::elog(&format!("[doubao] 错误帧: {m}"));
                    if !done {
                        done = true;
                        emit(AsrEvent::Error(m));
                    }
                    break;
                }
                if typ == 0b1001 {
                    if let Ok(obj) = serde_json::from_slice::<serde_json::Value>(&body) {
                        let text = obj["result"]["text"].as_str().unwrap_or("").to_string();
                        let definite = obj["result"]["utterances"].as_array()
                            .map(|u| u.iter().any(|x| x["definite"] == json!(true)))
                            .unwrap_or(false);
                        // C38 诊断: 逐帧打印(定位 finish 后二遍识别的分段语义)
                        crate::log::elog(&format!(
                            "[doubao] 帧 definite={definite} len={} text={}",
                            text.chars().count(),
                            text.chars().take(40).collect::<String>()
                        ));
                        if !text.is_empty() {
                            // C33(存疑): text 全量替换——若服务端分段重发会丢前段, 由诊断日志定夺
                            latest_text = text;
                            emit(AsrEvent::Partial(latest_text.clone()));
                        }
                    }
                }
            }
        }
    }
}

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut d = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    let _ = std::io::Read::read_to_end(&mut d, &mut out);
    out
}

/// Sec-WebSocket-Key: 16 随机字节 base64(用 uuid bytes 凑 16B)
fn ws_key() -> String {
    let u1 = uuid::Uuid::new_v4();
    let u2 = uuid::Uuid::new_v4();
    let mut raw = [0u8; 16];
    raw[..8].copy_from_slice(&u1.as_bytes()[..8]);
    raw[8..].copy_from_slice(&u2.as_bytes()[..8]);
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_no_seq() {
        // type=0b0010(音频) flags=0(无 seq) comp=1(gzip)
        let payload = b"hello".to_vec();
        let f = frame([0x11, 0x20 | 0x01, 0x01, 0x00], &payload);
        let (typ, comp, body) = parse_frame(&f).unwrap();
        assert_eq!(typ, 0x2);
        assert_eq!(comp, 0x1);
        assert_eq!(body, payload);
    }
    #[test]
    fn frame_with_seq() {
        // flags=0b0011(带 4B sequence)
        let mut data = vec![0x11, 0x23, 0x01, 0x00];
        data.extend_from_slice(&7u32.to_be_bytes()); // seq
        data.extend_from_slice(&(5u32).to_be_bytes()); // size
        data.extend_from_slice(b"hello");
        let (typ, comp, body) = parse_frame(&data).unwrap();
        assert_eq!(typ, 0x2);
        assert_eq!(comp, 0x1);
        assert_eq!(body, b"hello".to_vec());
    }
    #[test]
    fn parse_garbage_none() {
        assert!(parse_frame(&[0u8; 4]).is_none());
        assert!(parse_frame(&[]).is_none());
    }
    #[test]
    fn gzip_roundtrip() {
        let d = b"compress me please".to_vec();
        let c = gzip(&d);
        assert_eq!(gunzip(&c), d);
    }
    // C33 回归: latest_text 必须是"替换"语义——已由 run_session 内单变量实现保证,
    // 状态机由集成测试(VOICEMAC_AUTOTEST_FILE 长音频无重复)覆盖
    // C30 回归: uid 每会话唯一——run_session 内 uuid 生成, 由集成测试覆盖
}
