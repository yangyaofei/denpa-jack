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
//!
//! 采集目标 = **最前面那个 App 的前窗**（`screencapture -l <窗口ID>`），不是整屏（2026-10-18 定）。
//! 理由：①我们要的上下文本来就是「你正在打字的那个窗口」；②截别人的窗口，我们的浮窗是另一个
//! 窗口，**永远不会被拍进去**，于是"显示浮窗"不必再等截图；③实测按窗口截 219–571ms
//! （验证工具 research-plan/tmp/winlist.swift）。
//! 保护：单飞（同时最多一个 screencapture）+ 2s 超时（超时杀进程，本次不带图）。
//! 反例留档：此前用整屏截图（p50 220ms、**最长 134.6s**），且浮窗/托盘被排在截图之后，
//! 截图一卡就表现为"按住很久浮窗才出现"。
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
    /// 采集目标窗口 ID；None = 未判定（当前实现要求有窗口，None 只作类型兜底）
    pub window: Option<u32>,
}

/// 截图超时（毫秒）：超时杀进程。整屏截图曾出现 134s 卡死，这个上限保证它永远不拖住任何东西。
const CAPTURE_TIMEOUT_MS: u128 = 2500;

/// 屏幕上的一个窗口（只取定位需要的字段）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinInfo {
    pub id: u32,
    pub pid: i32,
    pub layer: i32,
    /// 逻辑尺寸；0 = 未取到（视为可通过尺寸筛）
    pub w: i64,
    pub h: i64,
}

/// 纯函数：从"按前后顺序"的窗口列表里挑采集目标。
/// 规则：层 0（普通窗口）+ 不是我们自己的进程 + 不是小浮条（≥200×150；尺寸未知时放行）。
pub fn pick_window(wins: &[WinInfo], self_pid: i32) -> Option<WinInfo> {
    wins.iter()
        .find(|w| {
            w.layer == 0
                && w.pid != self_pid
                && (w.w == 0 || w.w >= 200)
                && (w.h == 0 || w.h >= 150)
        })
        .copied()
}

/// 读屏幕上的窗口列表（按前后顺序，最前面在前）
fn on_screen_windows() -> Vec<WinInfo> {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };
    let opts = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let Some(list) = copy_window_info(opts, 0) else {
        return Vec::new();
    };
    let k_num = CFString::new("kCGWindowNumber");
    let k_layer = CFString::new("kCGWindowLayer");
    let k_pid = CFString::new("kCGWindowOwnerPID");
    let k_bounds = CFString::new("kCGWindowBounds");
    let k_w = CFString::new("Width");
    let k_h = CFString::new("Height");
    let mut out = Vec::new();
    for i in 0..list.len() {
        let Some(item) = list.get(i) else { continue };
        let dict: CFDictionary<CFString, core_foundation::base::CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef) };
        let num = |k: &CFString| -> Option<i64> {
            let v = dict.find(k)?;
            v.downcast::<CFNumber>()?.to_i64()
        };
        let (Some(id), Some(layer), Some(pid)) = (num(&k_num), num(&k_layer), num(&k_pid)) else {
            continue;
        };
        let (mut w, mut h) = (0i64, 0i64);
        if let Some(b) = dict.find(&k_bounds) {
            // CFDictionary 不是 ConcreteCFType, 不能 downcast: 直接按裸引用包一层
            let bd: CFDictionary<CFString, core_foundation::base::CFType> = unsafe {
                CFDictionary::wrap_under_get_rule(
                    b.as_concrete_TypeRef() as core_foundation::dictionary::CFDictionaryRef,
                )
            };
            let f = |k: &CFString| -> Option<f64> {
                bd.find(k)?.downcast::<CFNumber>()?.to_f64()
            };
            if let (Some(wd), Some(ht)) = (f(&k_w), f(&k_h)) {
                w = wd as i64;
                h = ht as i64;
            }
        }
        out.push(WinInfo { id: id as u32, pid: pid as i32, layer: layer as i32, w, h });
    }
    out
}

/// 目标窗口：最前面那个非自身 App 的前窗
fn front_target_window(self_pid: i32) -> Option<WinInfo> {
    pick_window(&on_screen_windows(), self_pid)
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

/// screencapture 参数（纯函数，便于单测）：`-x` 不发声、`-o` 不带窗口阴影、`-l<id>` 只截该窗口。
fn capture_args(window_id: u32) -> Vec<String> {
    ["-x", "-o"]
        .iter()
        .map(|s| (*s).to_string())
        .chain(std::iter::once(format!("-l{window_id}")))
        .collect()
}

/// 跑一次 screencapture：带超时（超时杀掉，不留残留进程）。
fn run_screencapture(path: &Path, window_id: u32) -> Result<(), String> {
    let mut child = std::process::Command::new("/usr/sbin/screencapture")
        .args(capture_args(window_id))
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("screencapture 启动失败: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(CAPTURE_TIMEOUT_MS as u64);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("截图超时({CAPTURE_TIMEOUT_MS}ms)"));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return Err(format!("等待截图进程失败: {e}")),
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            use std::io::Read;
            let _ = s.read_to_string(&mut err);
        }
        return Err(format!("screencapture 退出码 {:?}: {}", status.code(), err.trim()));
    }
    Ok(())
}

/// 采集目标窗口到 `dir`（每次新文件），返回文件路径与大小。
/// 目标 = 最前面那个 App 的前窗（排除我们自己的窗口，见模块注释）。
/// 保护：单飞（同时最多一个）+ 2.5s 超时 + **超时/失败重试一次**——实测窗口首次采集可能 >2s
/// （冷启动/窗口未被合成过），第二次通常 <300ms，不做重试会白丢一次上下文图。
/// 本函数整体跑在采集线程里，没有任何调用方等待它，所以慢/失败都不影响转写与交付。
pub fn capture(dir: &Path, tag: &str) -> Result<Shot, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CAPTURING: AtomicBool = AtomicBool::new(false);
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            CAPTURING.store(false, Ordering::SeqCst);
        }
    }
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return Err("上一次截图还在进行，跳过本次".into());
    }
    let _guard = Guard;

    std::fs::create_dir_all(dir).map_err(|e| format!("建截图目录失败: {e}"))?;
    let self_pid = std::process::id() as i32;
    let win = front_target_window(self_pid)
        .ok_or("未找到可采集的目标窗口（前台应用没有普通窗口？）")?;
    let path = dir.join(format!("screen-{}-{tag}.png", now_ms()));
    let t0 = std::time::Instant::now();
    let mut last_err = String::new();
    for _ in 1..=2 {
        match run_screencapture(&path, win.id) {
            Ok(()) => {
                let bytes = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
                if bytes == 0 {
                    last_err = "截图文件为空（通常是未授予屏幕录制权限）".into();
                    continue;
                }
                return Ok(Shot {
                    path,
                    bytes,
                    elapsed_ms: t0.elapsed().as_millis(),
                    window: Some(win.id),
                });
            }
            Err(e) => last_err = format!("{e} (窗口 id={} pid={} {}x{})", win.id, win.pid, win.w, win.h),
        }
    }
    Err(format!("截图失败(已重试一次): {last_err}"))
}

/// 读成 base64 data URL（模型请求内联用）。
/// 同时返回可核验的指纹：字节数、data URL 字符数、sha256 前 16 位——日志里用它证明
/// "发出去的 base64 内容 = 磁盘上这张截图"。`shasum -a 256 <文件>` 可对照。
pub struct InlineImage {
    pub data_url: String,
    pub bytes: usize,
    pub sha256_short: String,
}

pub fn inline_image(path: &Path) -> Result<InlineImage, String> {
    use base64::Engine;
    let raw = std::fs::read(path).map_err(|e| format!("读截图失败: {e}"))?;
    let bytes = raw.len();
    let sha256_short = sha256_short_of(&raw);
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&raw)
    );
    Ok(InlineImage { data_url, bytes, sha256_short })
}

/// data URL 的前 32 个字符（形如 `data:image/png;base64,iVBORw0KG…`）——日志里让人一眼看到
/// "确实是一个内联 data URL 而不是文件路径"。
pub fn data_url_prefix(data_url: &str) -> String {
    data_url.chars().take(32).collect()
}

/// 原始字节的 sha256 前 16 位十六进制
pub fn sha256_short_of(raw: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(raw);
    let hex = format!("{:x}", h.finalize());
    hex[..16].to_string()
}

/// 读成 base64 data URL（模型请求内联用）
pub fn to_data_url(path: &Path) -> Result<String, String> {
    Ok(inline_image(path)?.data_url)
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

    /// 指纹要和标准 sha256 一致（外部可用 `shasum -a 256` 对照核验）
    #[test]
    fn sha256_short_matches_known_digest() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(sha256_short_of(b"hello"), "2cf24dba5fb0a30e");
        assert_eq!(sha256_short_of(b""), "e3b0c44298fc1c14");
    }

    /// inline_image 的三项指纹互相自洽：字节数=文件大小、data URL 前缀正确、sha 与文件内容一致
    #[test]
    fn inline_image_reports_verifiable_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("shot.png");
        let raw = b"\x89PNG\r\n\x1a\nfake";
        std::fs::write(&p, raw).unwrap();
        let img = inline_image(&p).unwrap();
        assert_eq!(img.bytes, raw.len());
        assert_eq!(img.sha256_short, sha256_short_of(raw));
        assert!(img.data_url.starts_with("data:image/png;base64,"));
        let prefix = data_url_prefix(&img.data_url);
        assert_eq!(prefix.chars().count(), 32);
        assert!(prefix.starts_with("data:image/png;base64,iVBOR"), "前缀应露出内联 png 特征: {prefix}");
        assert!(img.data_url.chars().count() > img.bytes, "base64 应比原始字节长");
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

    #[test]
    fn capture_args_targets_window() {
        // 只截目标窗口: -x 不发声, -o 不带阴影, -l<id> 指定窗口
        assert_eq!(capture_args(93116), vec!["-x", "-o", "-l93116"]);
    }

    /// 窗口筛选: 层 0(普通窗口) + 排除自身 + 跳过小浮条; 顺序即前后层级
    #[test]
    fn pick_window_rules() {
        let w = |id, pid, layer, ww, hh| WinInfo { id, pid, layer, w: ww, h: hh };
        // 自身窗口在最前 → 跳过, 取目标 App 的前窗
        let list = vec![w(1, 999, 0, 800, 600), w(2, 42, 0, 1200, 800)];
        assert_eq!(pick_window(&list, 999).map(|x| x.id), Some(2));
        // 非 0 层(浮层/面板)不算
        let list = vec![w(1, 42, 25, 1200, 800), w(2, 42, 0, 1200, 800)];
        assert_eq!(pick_window(&list, 999).map(|x| x.id), Some(2));
        // 小浮条跳过
        let list = vec![w(1, 42, 0, 120, 40), w(2, 42, 0, 900, 700)];
        assert_eq!(pick_window(&list, 999).map(|x| x.id), Some(2));
        // 尺寸未知(0)放行
        let list = vec![w(1, 42, 0, 0, 0)];
        assert_eq!(pick_window(&list, 999).map(|x| x.id), Some(1));
        // 只有自身窗口 → 没有目标
        assert_eq!(pick_window(&[w(1, 999, 0, 800, 600)], 999).map(|x| x.id), None);
    }
}
