// 历史(JSONL, 参照 Swift OutputAndHistory)
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// 历史文件互斥锁: append(Live 交付线程) / update_by_ts(重转结算线程) /
/// enforce_limit(清理) 三个写者 + 多读者并发——无锁时 read-modify-write
/// 交错会 rename 覆盖丢行、读到半行丢解析(真机: "强类型未命中但 Value
/// 兜底命中"每次重转都出现, 即 read_all_from 读到了 append 写一半的行)。
/// 全部读写经此锁串行化(历史文件几 MB, 临界区耗时可忽略)。
static HISTORY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// R1: 按 ts 定位原位更新一条历史记录(JSONL 逐行匹配替换, 原子写回)。
/// 重转回写专用——append 之外唯一的写路径。ts 是毫秒时间戳, 同毫秒两条
/// 概率极低且重转场景只会有一个在途。
pub fn update_by_ts(app: &tauri::AppHandle, ts: &str, new_rec: &HistoryRecord) -> Result<(), String> {
    let _g = HISTORY_LOCK.lock().unwrap();
    let path = history_path(app);
    if !path.exists() {
        return Err("历史文件不存在".into());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut replaced = false;
    let mut out = String::with_capacity(content.len() + 256);
    for line in content.lines() {
        if !replaced && line.contains("\"ts\":\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v["ts"].as_str() == Some(ts) {
                    out.push_str(&serde_json::to_string(new_rec).map_err(|e| e.to_string())?);
                    out.push('\n');
                    replaced = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        return Err(format!("未找到 ts={ts} 的历史条目"));
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn history_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = crate::config::data_dir(app);
    fs::create_dir_all(&dir).ok();
    dir.join("history.jsonl")
}

pub fn recordings_dir(app: &tauri::AppHandle) -> PathBuf {
    let dir = crate::config::data_dir(app);
    let r = dir.join("recordings");
    fs::create_dir_all(&r).ok();
    r
}

/// 条数上限裁剪(读全量保留尾部 limit 条, Handy history_limit)
pub fn enforce_limit(app: &tauri::AppHandle, limit: u64) {
    let _g = HISTORY_LOCK.lock().unwrap();
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

/// 主窗历史刷新信号(转发 core: 计数已收口, 写路径走 core::bump_history)
pub fn history_version() -> u64 {
    crate::core::history_version()
}

pub fn append(app: &tauri::AppHandle, rec: &HistoryRecord) {
    crate::core::bump_history();
    append_record_to(&history_dir(app), rec);
}

/// 追加一条记录到指定目录的 history.jsonl(可测核心)
pub fn append_record_to(dir: &std::path::Path, rec: &HistoryRecord) {
    let _g = HISTORY_LOCK.lock().unwrap();
    let _ = fs::create_dir_all(dir);
    // B45: ts 是前端 x-for 的 :key——同秒两条会 key 冲突导致列表渲染塌缩。
    // 撞秒时回读最后一行, 同 ts 则 +1s 偏移。
    let file = dir.join("history.jsonl");
    if let Ok(raw) = fs::read_to_string(&file) {
        if let Some(last) = raw.lines().rev().find(|l| !l.trim().is_empty()) {
            if let Ok(prev) = serde_json::from_str::<HistoryRecord>(last) {
                if prev.ts == rec.ts {
                    let mut r2 = rec.clone();
                    r2.ts = bump_second(&rec.ts);
                    if let Ok(mut line) = serde_json::to_string(&r2) {
                        line.push('\n');
                        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&file) {
                            let _ = f.write_all(line.as_bytes());
                        }
                    }
                    return;
                }
            }
        }
    }
    if let Ok(mut line) = serde_json::to_string(rec) {
        line.push('\n');
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(dir.join("history.jsonl")) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// 读取指定目录全部记录(时间倒序)。经 HISTORY_LOCK——与写者互斥,
/// 避免读到 append/update 写一半的行(filter_map 会静默丢行)。
pub fn read_all_from(dir: &std::path::Path) -> Vec<HistoryRecord> {
    let _g = HISTORY_LOCK.lock().unwrap();
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

/// 保留最近 N 个 wav(按文件名排序=时间序)——可测核心(仅测试使用)
#[cfg(test)]
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
    crate::config::data_dir(app)
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
/// wav_header 已归位基座 util(批5: zhipu 拼上传体与这里落盘共用同一格式)

pub fn save_wav(app: &tauri::AppHandle, pcm: &[u8]) -> Option<String> {
    let ts = chrono_like_ts();
    let path = recordings_dir(app).join(format!("{ts}.wav"));
    // 头与 PCM 一次写全(44B 头来自基座 util; 旧手写头与 wav_header 函数是重复实现, 批5 合一)
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(&crate::util::wav_header(pcm.len()));
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

/// 取 wav 的 data chunk(自测回放用)——从 recording.rs 归位(阶段A 第2步):
/// WAV 封装(wav_header)与解封装同住。遍历 chunk 跳过 FLLR 填充块(C40 教训:
/// 真实录音 wav 的 data 不在固定 offset 44)。
pub fn wav_data_chunk(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 44 || &raw[0..4] != b"RIFF" {
        return None;
    }
    let mut i = 12usize;
    while i + 8 <= raw.len() {
        let id = &raw[i..i + 4];
        let size = u32::from_le_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        if id == b"data" {
            let end = (i + 8 + size).min(raw.len());
            return Some(raw[i + 8..end].to_vec());
        }
        i += 8 + size + (size % 2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_second_unique_and_monotonic() {
        // 常规: 秒 +1
        assert_eq!(bump_second("2026-10-24 10:30:05"), "2026-10-24 10:30:06");
        // 59 → 伪秒 :60(唯一且排序在后; 旧实现 %60 回退 00, 同秒第三条与第二条撞 ts, B45 复发)
        assert_eq!(bump_second("2026-10-24 10:30:59"), "2026-10-24 10:30:60");
        // 连续撞秒也保持唯一(真实时间戳永远不会是 60+)
        assert_eq!(bump_second("2026-10-24 10:30:60"), "2026-10-24 10:30:61");
    }

    #[test]
    fn config_atomic_write_roundtrip_and_no_tmp_leftover() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        crate::config::write_config_atomic(&p, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":1}");
        // 覆盖写不留 .tmp 残留
        crate::config::write_config_atomic(&p, "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":2}");
        let leftovers: Vec<_> = std::fs::read_dir(d.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "tmp").unwrap_or(false))
            .collect();
        assert!(leftovers.is_empty(), "tmp 残留: {leftovers:?}");
    }

    #[test]
    fn wav_header_valid_pcm16_mono_16k() {
        // C40 回归: 曾因半字节拷贝把 audio_format 写成 0x0101(257) 智谱拒收
        // (wav_header 已归位基座 util, 用例留此随 save_wav 消费方一起防回归)
        let h = crate::util::wav_header(1000);
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
            llm_thinking: None,
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
mod wav_tests {
    use super::wav_data_chunk;

    #[test]
    fn wav_data_chunk_skips_fllr_padding() {
        // C40 回归: 真实录音 wav 有 FLLR 填充块, data 不在 offset 44(随 wav_data_chunk 从 recording.rs 迁来)
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(1000u32).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&(16u32).to_le_bytes());
        w.extend_from_slice(&[0u8; 16]);
        w.extend_from_slice(b"FLLR");
        w.extend_from_slice(&(4044u32).to_le_bytes());
        w.extend_from_slice(&vec![0xEEu8; 4044]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(10u32).to_le_bytes());
        w.extend_from_slice(b"0123456789");
        let pcm = wav_data_chunk(&w).unwrap();
        assert_eq!(pcm, b"0123456789");
    }

    #[test]
    fn wav_data_chunk_rejects_garbage() {
        assert!(wav_data_chunk(b"").is_none());
        assert!(wav_data_chunk(b"RIFFshort").is_none());
        assert!(wav_data_chunk(b"NOTAWAVE........").is_none());
        assert!(wav_data_chunk(b"not a wav file at all....").is_none());
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
            delivered: "pasted-cmdv".into(), audio_path: String::new(), duration_ms: 1000, warning: None, llm_thinking: None,
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

/// "2026-09-12 21:42:09" → "2026-09-12 21:42:10"(仅秒+1, 测试友好)
pub fn bump_second(ts: &str) -> String {
    // 撞秒唯一化: 秒 +1。**不取模**——59 时产生 ":60" 伪秒: 字符串唯一且排序仍在后,
    // 真实时间戳永远不会是 60(旧实现 (n+1)%60 让 59→00 回退, 同秒第三条会与第二条
    // 生成相同 ts, B45 撞秒塌缩复发; 而跨分进位需要完整日历, 不值得)。
    let mut t = ts.to_string();
    if let Some(sec_pos) = t.rfind(':') {
        if let Ok(n) = t[sec_pos + 1..].parse::<u32>() {
            t.replace_range(sec_pos + 1.., &format!("{:02}", n + 1));
        }
    }
    t
}

#[cfg(test)]
mod rerun_tests {
    use super::*;

    #[test]
    fn parse_real_failed_line() {
        // 真机 bug 复现: 2026-10-25 失败段行在 find_by_ts 解析不出来?
        let line = r#"{"ts":"2026-10-25 21:36:53","engine":"","raw":"","final_text":"","llm_used":false,"llm_thinking":null,"delivered":"failed","audio_path":"/x/rec.wav","duration_ms":30997,"warning":"w"}"#;
        let r: HistoryRecord = serde_json::from_str(line).expect("这行必须能解析");
        assert_eq!(r.ts, "2026-10-25 21:36:53");
    }
}
