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

pub fn append(app: &tauri::AppHandle, rec: &HistoryRecord) {
    if let Ok(mut line) = serde_json::to_string(rec) {
        line.push('\n');
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(history_path(app)) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

pub fn recent(app: &tauri::AppHandle, limit: usize) -> Vec<HistoryRecord> {
    let Ok(raw) = fs::read_to_string(history_path(app)) else { return vec![] };
    let mut out: Vec<HistoryRecord> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    out.reverse();
    out.truncate(limit);
    out
}

/// 保留最近 N 个录音, 删多余
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
    h[21..22].copy_from_slice(&1u16.to_le_bytes()[..1]);
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
