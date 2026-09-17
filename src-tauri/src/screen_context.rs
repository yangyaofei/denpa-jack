//! 屏幕上下文：把当前屏幕截图作为纠错上下文传给模型
//!
//! 开关式功能（config `screenshot_context`，默认关）：开启后每次录音开始时采集一张截图，
//! ASR 完成后与转写文本一起发给纠错模型；模型据屏幕内容修正专有名词。
//!
//! 依赖与边界：
//! - 需要"屏幕录制"权限（macOS TCC），未授权时采集失败 → 只记日志、退回纯文本纠错，不阻塞交付
//! - 截图保存在 `<数据目录>/screen_context/`，保留最近若干张，便于事后判读与复现（用户要求留档）
//! - 每次录音采集一张新截图（屏幕内容每次都在变，不做复用/缓存）；图片按 base64 内联进请求
//! - 实测依据见 research-plan/voice-mac-app/day-07-context/report.md
use std::path::{Path, PathBuf};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 截图目录名（数据目录下）
pub const DIR_NAME: &str = "screen_context";
/// 保留最近多少张截图（超出按时间清理最早的）
const KEEP: usize = 200;

pub struct Shot {
    pub path: PathBuf,
    pub bytes: usize,
    pub elapsed_ms: u128,
}

/// 是否已获屏幕录制权限（不弹窗、不触发系统引导）
pub fn preflight() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 申请屏幕录制权限。返回当前是否已授予；首次调用会触发系统引导（用户需到系统设置里勾选，
/// 且通常需要重启应用才生效）。
pub fn request() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

/// 采集屏幕到 `dir`（每次新文件），返回文件路径与大小
pub fn capture(dir: &Path, tag: &str) -> Result<Shot, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("建截图目录失败: {e}"))?;
    let path = dir.join(format!("screen-{}-{tag}.png", now_ms()));
    let t0 = std::time::Instant::now();
    let out = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", "-t", "png"])
        .arg(&path)
        .output()
        .map_err(|e| format!("screencapture 启动失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "screencapture 退出码 {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let bytes = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
    if bytes == 0 {
        return Err("截图文件为空（通常是未授予屏幕录制权限）".into());
    }
    Ok(Shot {
        path,
        bytes,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

/// 读成 base64 data URL（模型请求内联用）
pub fn to_data_url(path: &Path) -> Result<String, String> {
    use base64::Engine;
    let raw = std::fs::read(path).map_err(|e| format!("读截图失败: {e}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    ))
}

/// 只保留最近 `keep` 张截图（按文件名里的时间戳排序）
pub fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("screen-") && name.ends_with(".png") {
                Some((name, e.path()))
            } else {
                None
            }
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let drop_count = files.len() - keep;
    for (_, p) in files.into_iter().take(drop_count) {
        let _ = std::fs::remove_file(p);
    }
}

/// 采集 + 清理（一次调用，供录音开始时后台线程使用）
pub fn capture_and_prune(data_dir: &Path, tag: &str) -> Result<Shot, String> {
    let dir = data_dir.join(DIR_NAME);
    let shot = capture(&dir, tag)?;
    prune(&dir, KEEP);
    Ok(shot)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_has_png_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        std::fs::write(&p, b"hello").unwrap();
        let url = to_data_url(&p).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        assert!(url.ends_with("aGVsbG8="), "base64('hello') 应为 aGVsbG8=：{url}");
    }

    #[test]
    fn data_url_missing_file_errors() {
        assert!(to_data_url(Path::new("/nonexistent/none.png")).is_err());
    }

    #[test]
    fn prune_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        for ts in [100, 200, 300, 400] {
            std::fs::write(dir.path().join(format!("screen-{ts}-a.png")), b"x").unwrap();
        }
        std::fs::write(dir.path().join("other.txt"), b"x").unwrap();
        prune(dir.path(), 2);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["other.txt", "screen-300-a.png", "screen-400-a.png"]);
    }

    #[test]
    fn prune_noop_when_below_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("screen-1-a.png"), b"x").unwrap();
        prune(dir.path(), 5);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn capture_fails_gracefully_on_bad_dir() {
        // 目录不可创建时返回 Err 而不是 panic（调用方据此退回纯文本）
        let r = capture(Path::new("/dev/null/nope"), "t");
        assert!(r.is_err());
    }
}
