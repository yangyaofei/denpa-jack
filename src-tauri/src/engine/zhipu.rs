// 智谱 GLM-ASR 文件式引擎(第二 ASR 引擎, 兜底/长句)
// 移植自 Swift ZhipuFileEngine: feed 累积 → finish 打包 WAV multipart 上传; 无实时 partial
use super::contract::{AsrEvent, Cmd};

/// 智谱引擎(单元标记: 挂 impl 用)
pub struct Zhipu;

impl<E: Fn(AsrEvent) + Send + 'static> super::AsrEngine<E> for Zhipu {
async fn run(
    self,
    ctx: super::EngineCtx,
    mut rx: tokio::sync::mpsc::Receiver<Cmd>,
    emit: E,
) {
    let api_key = ctx.api_key;
    let _hotwords = ctx.hotwords; // 智谱 ASR 无热词支持
    let mut pcm: Vec<u8> = vec![];
    let mut n_chunks = 0usize;
    // 收集直到 Finish/Abort
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Feed(chunk) => {
                n_chunks += 1;
                pcm.extend_from_slice(&chunk);
            }
            Cmd::Finish => {
                crate::log::elog(&format!("[zhipu] finish, chunks={n_chunks} pcm={}B", pcm.len()));
                break;
            }
            Cmd::Abort => return,
        }
    }
    if n_chunks > 0 && pcm.is_empty() {
        crate::log::elog("[zhipu] 有 chunk 但 pcm 为空!");
    }
    // D2 金标: 太短/转写空/服务端无语音都是"用户没说话"(NoSpeech)——不是失败,
    // 不进失败段不占队列; Error 只留给真故障(上传失败/响应异常), 那才值得重试。
    if pcm.len() < 1600 {
        emit(AsrEvent::NoSpeech("没听清(无音频)".into()));
        return;
    }
    let wav = wrap_wav(&pcm);
    crate::log::elog(&format!("[zhipu] wav {}B, 上传中...", wav.len()));
    match transcribe(&api_key, wav).await {
        Ok(Some(t)) => {
            crate::log::elog(&format!("[zhipu] result: {t}"));
            emit(AsrEvent::Result(t))
        }
        Ok(None) => {
            crate::log::elog("[zhipu] 转写为空");
            emit(AsrEvent::NoSpeech("没听清(转写为空)".into()))
        }
        Err(e) => {
            crate::log::elog(&format!("[zhipu] err: {e}"));
            if e.contains("no audio segment") {
                // 服务端判定无语音段: 同属"没说话", 金标 NoSpeech
                emit(AsrEvent::NoSpeech("没听清(无语音)".into()))
            } else {
                emit(AsrEvent::Error(e))
            }
        }
    }
}
}

async fn transcribe(api_key: &str, wav: Vec<u8>) -> Result<Option<String>, String> {
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .text("model", "glm-asr")
        .part("file", part);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post("https://open.bigmodel.cn/api/paas/v4/audio/transcriptions")
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("上传失败: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("响应异常: {e}"))?;
    if !status.is_success() {
        return Err(format!("智谱 ASR HTTP {status}: {body}"));
    }
    // 响应: {"id":..,"request_id":..,"text":"..."} 或 segments 结构
    let text = body["text"].as_str().map(|s| s.to_string()).unwrap_or_default();
    if json_to_str(&body).trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(text))
}

fn json_to_str(v: &serde_json::Value) -> String {
    v["text"].as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AsrEngine;

    #[tokio::test]
    async fn short_audio_is_nospeech_not_error() {
        // D2 金标: 太短(用户没说话)必须 NoSpeech——不是 Error, 不进失败段不占队列
        let (tx, rx) = tokio::sync::mpsc::channel::<Cmd>(8);
        tx.try_send(Cmd::Feed(vec![0u8; 800])).unwrap(); // 800B < 1600B 下限
        tx.try_send(Cmd::Finish).unwrap();
        drop(tx);
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let g2 = got.clone();
        let emit = move |ev: AsrEvent| g2.lock().unwrap().push(ev);
        super::Zhipu
            .run(super::super::EngineCtx { api_key: String::new(), base_url: String::new(), hotwords: vec![] }, rx, emit)
            .await;
        let g = got.lock().unwrap();
        assert!(matches!(g[0], AsrEvent::NoSpeech(ref t) if t.contains("无音频")));
    }
}

/// PCM(16k mono i16) → WAV(RIFF 44B 头)
pub fn wrap_wav(pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(&crate::history::wav_header(pcm.len()));
    out.extend_from_slice(pcm);
    out
}
