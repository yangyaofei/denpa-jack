// 历史(JSONL, 参照 Swift OutputAndHistory)
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub ts: String,
    pub engine: String,
    pub raw: String,
    pub final_text: String,
    pub llm_used: bool,
    #[serde(default)]
    pub llm_thinking: Option<String>,
    pub delivered: String, // clipboard+paste | clipboard
    pub audio_path: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub fn history_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().expect("app_data_dir 不可用");
    fs::create_dir_all(&dir).ok();
    dir.join("history.jsonl")
}

pub fn recordings_dir(app: &tauri::AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().expect("app_data_dir 不可用");
    let r = dir.join("recordings");
    fs::create_dir_all(&r).ok();
    r
}

/// 条数上限裁剪(读全量保留尾部 limit 条, Handy history_limit)
pub fn enforce_limit(app: &tauri::AppHandle, limit: u64) {
    if limit == 0 {
        return; // 0 = 不限
    }
    let path = history_path(app);
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() as u64 <= limit {
        return;
    }
    let keep: Vec<&str> = lines[lines.len() - limit as usize..].to_vec();
    let _ = std::fs::write(&path, keep.join("\n") + "\n");
}

/// 主窗历史刷新信号(事件通道不可达的轮询替代, 与 HUD 同模式)
pub static HISTORY_VER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn history_version() -> u64 {
    HISTORY_VER.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn append(app: &tauri::AppHandle, rec: &HistoryRecord) {
    HISTORY_VER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    append_record_to(&history_dir(app), rec);
}

/// 追加一条记录到指定目录的 history.jsonl(可测核心)
pub fn append_record_to(dir: &std::path::Path, rec: &HistoryRecord) {
    let _ = fs::create_dir_all(dir);
    if let Ok(mut line) = serde_json::to_string(rec) {
        line.push('\n');
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(dir.join("history.jsonl")) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// 读取指定目录全部记录(时间倒序)
pub fn read_all_from(dir: &std::path::Path) -> Vec<HistoryRecord> {
    let Ok(raw) = fs::read_to_string(dir.join("history.jsonl")) else { return vec![] };
    let mut out: Vec<HistoryRecord> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    out.reverse();
    out
}

pub fn recent(app: &tauri::AppHandle, limit: usize) -> Vec<HistoryRecord> {
    let mut out = read_all_from(&history_dir(app));
    out.truncate(limit);
    out
}

/// 保留最近 N 个 wav(按文件名排序=时间序)——可测核心
pub fn prune_wavs_in(dir: &std::path::Path, keep: usize) {
    let mut wavs: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "wav").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    wavs.sort();
    if wavs.len() > keep {
        for p in &wavs[..wavs.len() - keep] {
            let _ = fs::remove_file(p);
        }
    }
}

/// 保留最近 N 个录音, 删多余
fn history_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

pub fn prune_recordings(app: &tauri::AppHandle, keep: usize) {
    let dir = recordings_dir(app);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    while files.len() > keep {
        let oldest = files.remove(0);
        fs::remove_file(oldest).ok();
    }
}

/// PCM(16k mono i16) → WAV 写盘
/// 标准 WAV 44B 头(16k mono i16le)——落盘与上传共用, 只写一次
pub fn wav_header(data_len: usize) -> [u8; 44] {
    let mut h = vec![0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    let total = (36 + data_len) as u32;
    h[4..8].copy_from_slice(&total.to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&16000u32.to_le_bytes());
    h[28..32].copy_from_slice(&32000u32.to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&(data_len as u32).to_le_bytes());
    let mut out = [0u8; 44];
    out.copy_from_slice(&h);
    out
}

pub fn save_wav(app: &tauri::AppHandle, pcm: &[u8]) -> Option<String> {
    let ts = chrono_like_ts();
    let path = recordings_dir(app).join(format!("{ts}.wav"));
    let mut out = vec![];
    let sample_rate = 16000u32;
    let data_len = pcm.len() as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    fs::write(&path, out).ok()?;
    path.to_str().map(|s| s.to_string())
}

fn chrono_like_ts() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("rec_{now}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_valid_pcm16_mono_16k() {
        // C40 回归: 曾因半字节拷贝把 audio_format 写成 0x0101(257) 智谱拒收
        let h = wav_header(1000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(&h[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16); // fmt 块长
        assert_eq!(u16::from_le_bytes(h[20..22].try_into().unwrap()), 1); // PCM!
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 1); // mono
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 32000); // byte rate
        assert_eq!(u16::from_le_bytes(h[32..34].try_into().unwrap()), 2); // block align
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 16); // bits
        assert_eq!(&h[36..40], b"data");
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 1000);
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 1000);
    }
    #[test]
    fn history_roundtrip() {
        let r = HistoryRecord {
            ts: "2026-09-05 12:00:00".into(),
            engine: "zhipu".into(),
            raw: "些克数学".into(),
            final_text: "谢克数学".into(),
            llm_used: true,
            delivered: "pasted-ax".into(),
            audio_path: "/tmp/a.wav".into(),
            duration_ms: 1200,
            warning: None,
        };
        let line = serde_json::to_string(&r).unwrap();
        let back: HistoryRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back.final_text, "谢克数学");
        assert_eq!(back.duration_ms, 1200);
    }
}

#[cfg(test)]
mod fs_tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> (std::path::PathBuf, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        (d.path().to_path_buf(), d)
    }

    #[test]
    fn append_and_recent_roundtrip_real_file() {
        let (dir, _g) = tmpdir();
        let rec = HistoryRecord {
            ts: "2026-01-01 00:00:00".into(), engine: "volcengine".into(),
            raw: "原文".into(), final_text: "终稿".into(), llm_used: true,
            delivered: "pasted-cmdv".into(), audio_path: String::new(), duration_ms: 1000, warning: None,
        };
        append_record_to(&dir, &rec);
        append_record_to(&dir, &rec);
        let all = read_all_from(&dir);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn prune_oldest_by_filename_order() {
        let (dir, _g) = tmpdir();
        for i in 0..5 {
            fs::write(dir.join(format!("2026010{i}-000000.wav")), b"x").unwrap();
        }
        prune_wavs_in(&dir, 2);
        let left = fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, 2, "应只留最新 2 个");
    }
}
