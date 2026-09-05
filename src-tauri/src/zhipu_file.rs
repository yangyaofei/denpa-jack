// 智谱 GLM-ASR 文件式引擎(第二 ASR 引擎, 兜底/长句)
// 移植自 Swift ZhipuFileEngine: feed 累积 → finish 打包 WAV multipart 上传; 无实时 partial
use crate::doubao::{AsrEvent, Cmd};

pub async fn run_zhipu_session(
    api_key: String,
    _hotwords: Vec<String>, // 智谱 ASR 无热词支持
    mut rx: tokio::sync::mpsc::Receiver<Cmd>,
    emit: impl Fn(AsrEvent),
) {
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
    if pcm.len() < 1600 {
        emit(AsrEvent::Error("没听清(无音频)".into()));
        return;
    }
    let wav = wrap_wav(&pcm);
    crate::log::elog(&format!("[zhipu] wav {}B, 上传中...", wav.len()));
    // C40 诊断: 落盘实际产物, 供 curl 重放对照
    let _ = std::fs::write("../tmp/zhipu_upload.wav", &wav);
    match transcribe(&api_key, wav).await {
        Ok(Some(t)) => {
            crate::log::elog(&format!("[zhipu] result: {t}"));
            emit(AsrEvent::Result(t))
        }
        Ok(None) => {
            crate::log::elog("[zhipu] 转写为空");
            emit(AsrEvent::Error("没听清(转写为空)".into()))
        }
        Err(e) => {
            crate::log::elog(&format!("[zhipu] err: {e}"));
            emit(AsrEvent::Error(e))
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

/// PCM(16k mono i16) → WAV(RIFF 44B 头)
pub fn wrap_wav(pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(&crate::history::wav_header(pcm.len()));
    out.extend_from_slice(pcm);
    out
}
