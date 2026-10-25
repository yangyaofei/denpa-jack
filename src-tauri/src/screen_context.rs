//! 屏幕上下文：把当前窗口/屏幕截图作为纠错上下文传给模型
//!
//! 目标选项（config `detect_active_target`，默认 `active_window`）：
//! - `off`            不截图
//! - `active_window`  截"主窗口"：键盘焦点窗口（光标所在）→ 没有则指针下的窗口；
//!                    都没有则回退截显示器（`[ctx]` 日志说明）
//! - `active_display` 截"光标所在显示器"整屏；过滤器里排除本应用自己的窗口（浮窗不入镜）
//!
//! 主窗口与显示器的探测在 `active_target::detect_active_target`（与浮窗定位共用一次结果）。
//!
//! 实现：ScreenCaptureKit（进程内系统 API，macOS 14+）：
//!   SCShareableContent 枚举 → SCContentFilter → SCScreenshotManager.captureImage → ImageIO 写 PNG。
//! 不再调用 `screencapture` 命令行：外部进程不可控（实测最长一次卡 134.6s，把浮窗显示也一起拖住）。
//! 窗口前后顺序取 CoreGraphics 窗口列表（文档保证"从前往后"）；ScreenCaptureKit 只负责截。
//!
//! 依赖与边界：
//! - 需要"屏幕录制"权限（macOS TCC），未授权时采集失败 → 只记日志、退回纯文本纠错，不阻塞交付
//! - 截图保存在 `<数据目录>/screen_context/`，保留最近若干张，便于事后判读与复现（用户要求留档）
//! - 每次录音采集一张新截图（屏幕内容每次都在变，不做复用/缓存）；图片按 base64 内联进请求
//! - 实测依据见 research-plan/voice-mac-app/day-07-context/report.md
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::{CFRetained, CFString, CFURL};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
    SCWindow,
};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const std::ffi::c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const std::ffi::c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const std::ffi::c_void, idx: isize) -> *const std::ffi::c_void;
    fn CFDictionaryGetValue(
        dict: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    fn CFNumberGetValue(
        number: *const std::ffi::c_void,
        the_type: isize,
        value_ptr: *mut std::ffi::c_void,
    ) -> bool;
    fn CFStringGetCString(
        string: *const std::ffi::c_void,
        buffer: *mut i8,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    // 与 overlay.rs/deliver.rs 保持同一签名（否则 clashing_extern_declarations）
    fn CFRelease(cf: *mut std::ffi::c_void);
}

/// CFString 的 UTF-8 编码常量（CFStringEncodings 里 kCFStringEncodingUTF8）
const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

/// CGWindowListCopyWindowInfo 选项（本机只需这两个）
const K_CG_WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
/// CFNumberGetValue 的目标类型（CoreFoundation 里值恒为 13；请求什么类型都会做转换）
const K_CF_NUMBER_DOUBLE_TYPE: isize = 13;

/// 截图目录名（数据目录下）
pub const DIR_NAME: &str = "screen_context";
/// 保留最近多少张截图（超出按时间清理最早的）
const KEEP: usize = 200;
/// 窗口合格线：小于这个尺寸的窗口不作为上下文目标（弹窗/工具面板等）
const MIN_WIN_W: f64 = 200.0;
const MIN_WIN_H: f64 = 150.0;
/// SCK 回调等待上限（枚举一次、截图一次，各给 5s；内部是异步回调 + 主线程等结果）
const SCK_TIMEOUT: Duration = Duration::from_secs(5);

/// 截图目标（config `detect_active_target` 的解析结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotTarget {
    Off,
    ActiveWindow,
    ActiveDisplay,
}

/// 解析配置值：`off` / `active_display` 明确匹配；其余（含 `active_window` 与未知值）
/// 都按默认 `active_window` 处理（与"旧键忽略、新键缺省 active_window"的约定一致）。
pub fn parse_target(s: &str) -> ShotTarget {
    match s.trim() {
        "off" => ShotTarget::Off,
        "active_display" => ShotTarget::ActiveDisplay,
        _ => ShotTarget::ActiveWindow,
    }
}

/// 日志用：目标名（与配置值同名）
pub fn target_name(t: ShotTarget) -> &'static str {
    match t {
        ShotTarget::Off => "off",
        ShotTarget::ActiveWindow => "active_window",
        ShotTarget::ActiveDisplay => "active_display",
    }
}

pub struct Shot {
    pub path: PathBuf,
    pub bytes: usize,
    pub elapsed_ms: u128,
    /// 采集目标的描述（日志用）：`window(12345 标题)` / `display(2 id=1)`
    pub desc: String,
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

// ---------------------------------------------------------------------------
// 窗口列表与挑选（纯逻辑部分可单测）
// ---------------------------------------------------------------------------

/// 一个窗口的判定信息（来自 CoreGraphics 窗口列表，顺序=从前往后）
#[derive(Debug, Clone, PartialEq)]
pub struct WinInfo {
    pub id: u32,
    pub pid: i32,
    pub layer: i64,
    /// 窗口标题（无权限/无标题时为空串）
    pub title: String,
    /// (x, y, w, h)，全局显示空间、点为单位（y 向下）
    pub frame: (f64, f64, f64, f64),
}

impl WinInfo {
    fn eligible(&self, our_pid: i32) -> bool {
        self.layer == 0
            && self.pid != our_pid
            && self.frame.2 >= MIN_WIN_W
            && self.frame.3 >= MIN_WIN_H
    }

    fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.frame.0
            && p.0 <= self.frame.0 + self.frame.2
            && p.1 >= self.frame.1
            && p.1 <= self.frame.1 + self.frame.3
    }
}

/// 光标命中的窗口（纯函数，便于单测）：从前往后，第一个"合格且包含该点"的窗口。
/// 合格 = 层 0、非本进程、≥200×150（过滤浮层、本应用窗口、工具提示等）。
pub fn pick_window_at(wins: &[WinInfo], our_pid: i32, point: (f64, f64)) -> Option<u32> {
    wins.iter()
        .find(|w| w.eligible(our_pid) && w.contains(point))
        .map(|w| w.id)
}

/// 前台窗口（供 capture_target 解析）：id / 标题 / 窗口框
#[derive(Debug, Clone)]
pub struct WindowRef {
    pub id: u32,
    pub title: String,
    pub frame: (f64, f64, f64, f64),
}

/// 光标下的普通窗口（层 0、非本进程、≥200×150）；没有则 None。
pub fn window_at(point: (f64, f64)) -> Option<WindowRef> {
    let wins = window_list();
    let id = pick_window_at(&wins, std::process::id() as i32, point)?;
    wins.into_iter()
        .find(|w| w.id == id)
        .map(|w| WindowRef {
            id: w.id,
            title: w.title,
            frame: w.frame,
        })
}

/// 键盘焦点（光标所在）窗口，三级取法：
/// 1. AX 焦点元素 → 窗口框匹配（±2px，最精确）；
/// 2. 取不到框（Electron 等应用常拿不到 AXWindow）→ 焦点应用的 pid → 它最前面的窗口；
/// 3. AX 整体不可用（应用不响应 AX，实测 OpenChamber 会返回 kAXErrorCannotComplete）
///    → 回退"全系统最前面的普通窗口"（≈ 前台应用的主窗口）。
pub fn focused_window() -> Option<WindowRef> {
    let our_pid = std::process::id() as i32;
    let Some(info) = crate::deliver::focused_element_info() else {
        crate::log::elog("[ctx] AX 焦点不可用，回退全系统最前窗口");
        return frontmost_window();
    };
    if info.pid == our_pid {
        return None;
    }
    let wins = window_list();
    if let Some(frame) = info.frame {
        if let Some(w) = wins.iter().find(|w| {
            w.layer == 0
                && w.pid != our_pid
                && (w.frame.0 - frame.0).abs() <= 2.0
                && (w.frame.1 - frame.1).abs() <= 2.0
                && (w.frame.2 - frame.2).abs() <= 2.0
                && (w.frame.3 - frame.3).abs() <= 2.0
        }) {
            return Some(WindowRef {
                id: w.id,
                title: w.title.clone(),
                frame: w.frame,
            });
        }
    }
    wins.into_iter()
        .find(|w| {
            w.pid == info.pid
                && w.layer == 0
                && w.frame.2 >= MIN_WIN_W
                && w.frame.3 >= MIN_WIN_H
        })
        .map(|w| WindowRef {
            id: w.id,
            title: w.title,
            frame: w.frame,
        })
}

/// 全系统最前面的普通窗口（层 0、非本进程、≥200×150；列表顺序=从前往后）
fn frontmost_window() -> Option<WindowRef> {
    let our_pid = std::process::id() as i32;
    window_list()
        .into_iter()
        .find(|w| {
            w.layer == 0
                && w.pid != our_pid
                && w.frame.2 >= MIN_WIN_W
                && w.frame.3 >= MIN_WIN_H
        })
        .map(|w| WindowRef {
            id: w.id,
            title: w.title,
            frame: w.frame,
        })
}

/// 读 CoreGraphics 窗口列表（文档保证从前往后）。失败返回空列表（调用方按"没有窗口"处理）。
fn window_list() -> Vec<WinInfo> {
    use core_graphics::window::{
        kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
    };
    unsafe {
        let arr = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            0,
        );
        if arr.is_null() {
            return Vec::new();
        }
        let n = CFArrayGetCount(arr);
        let mut out = Vec::with_capacity(n.max(0) as usize);
        for i in 0..n {
            let dict = CFArrayGetValueAtIndex(arr, i);
            if dict.is_null() {
                continue;
            }
            out.push(WinInfo {
                id: dict_number(dict, kCGWindowNumber as *const std::ffi::c_void) as u32,
                pid: dict_number(dict, kCGWindowOwnerPID as *const std::ffi::c_void) as i32,
                layer: dict_number(dict, kCGWindowLayer as *const std::ffi::c_void) as i64,
                title: dict_string(dict, kCGWindowName as *const std::ffi::c_void),
                frame: dict_bounds(dict, kCGWindowBounds as *const std::ffi::c_void),
            });
        }
        CFRelease(arr as *mut std::ffi::c_void);
        out
    }
}

unsafe fn dict_string(dict: *const std::ffi::c_void, key: *const std::ffi::c_void) -> String {
    let v = CFDictionaryGetValue(dict, key);
    if v.is_null() {
        return String::new();
    }
    let mut buf = [0i8; 512];
    let ok = CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8);
    if ok {
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    }
}

unsafe fn dict_number(dict: *const std::ffi::c_void, key: *const std::ffi::c_void) -> f64 {
    let v = CFDictionaryGetValue(dict, key);
    if v.is_null() {
        return 0.0;
    }
    let mut out: f64 = 0.0;
    let _ = CFNumberGetValue(v, K_CF_NUMBER_DOUBLE_TYPE, &mut out as *mut f64 as *mut std::ffi::c_void);
    out
}

unsafe fn dict_bounds(dict: *const std::ffi::c_void, key: *const std::ffi::c_void) -> (f64, f64, f64, f64) {
    let b = CFDictionaryGetValue(dict, key);
    if b.is_null() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let kx = CFString::from_str("X");
    let ky = CFString::from_str("Y");
    let kw = CFString::from_str("Width");
    let kh = CFString::from_str("Height");
    let ptr = |s: &CFRetained<CFString>| CFRetained::as_ptr(s).as_ptr() as *const std::ffi::c_void;
    (
        dict_number(b, ptr(&kx)),
        dict_number(b, ptr(&ky)),
        dict_number(b, ptr(&kw)),
        dict_number(b, ptr(&kh)),
    )
}

// ---------------------------------------------------------------------------
// ScreenCaptureKit 桥接
// ---------------------------------------------------------------------------

fn ns_err_msg(err: *mut NSError) -> String {
    unsafe {
        match err.as_ref() {
            Some(e) => e.localizedDescription().to_string(),
            None => "未知错误".to_string(),
        }
    }
}

/// 在可截取内容列表里按 windowID 找 SCWindow
fn find_sc_window(windows: &NSArray<SCWindow>, id: u32) -> Option<Retained<SCWindow>> {
    for i in 0..windows.count() {
        let w = unsafe { windows.objectAtIndex_unchecked(i) };
        if unsafe { w.windowID() } == id {
            return unsafe { Retained::retain(w as *const SCWindow as *mut SCWindow) };
        }
    }
    None
}

/// 在可截取内容列表里按 CGDirectDisplayID 找 SCDisplay
fn find_sc_display(displays: &NSArray<SCDisplay>, id: u32) -> Option<Retained<SCDisplay>> {
    for i in 0..displays.count() {
        let d = unsafe { displays.objectAtIndex_unchecked(i) };
        if unsafe { d.displayID() } == id {
            return unsafe { Retained::retain(d as *const SCDisplay as *mut SCDisplay) };
        }
    }
    None
}

/// 本应用自己的全部窗口（屏幕截图时排除用：浮窗/主窗/设置窗都不入镜）
fn own_windows(windows: &NSArray<SCWindow>, our_pid: i32) -> Vec<Retained<SCWindow>> {
    let mut out = Vec::new();
    for i in 0..windows.count() {
        let w = unsafe { windows.objectAtIndex_unchecked(i) };
        let owner = unsafe { w.owningApplication() };
        if owner.map(|a| unsafe { a.processID() }) == Some(our_pid) {
            if let Some(r) = unsafe { Retained::retain(w as *const SCWindow as *mut SCWindow) } {
                out.push(r);
            }
        }
    }
    out
}

/// 窗口中心所在显示器的缩放倍率（拿不到就按 2.0）
fn display_scale_for_center(displays: &NSArray<SCDisplay>, cx: f64, cy: f64) -> f64 {
    for i in 0..displays.count() {
        let d = unsafe { displays.objectAtIndex_unchecked(i) };
        let f = unsafe { d.frame() };
        if cx >= f.origin.x
            && cx <= f.origin.x + f.size.width
            && cy >= f.origin.y
            && cy <= f.origin.y + f.size.height
        {
            return crate::overlay::display_scale(unsafe { d.displayID() });
        }
    }
    2.0
}

/// 发起 SCK 单帧截图，完成时把结果（成功=目标描述，失败=错误串）发回通道。
/// 注意：块由系统持有到回调触发为止，本函数返回后块仍有效。
fn capture_png_async(
    filter: &SCContentFilter,
    cfg: &SCStreamConfiguration,
    path: PathBuf,
    desc: String,
    tx: mpsc::Sender<Result<String, String>>,
) {
    let block = RcBlock::new(move |img: *mut CGImage, err: *mut NSError| {
        let r = unsafe {
            if img.is_null() {
                Err(format!("SCK 截图失败: {}", ns_err_msg(err)))
            } else {
                write_png(&*img, &path).map(|_| desc.clone())
            }
        };
        let _ = tx.send(r);
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            cfg,
            Some(&block),
        );
    }
}

/// CGImage → PNG 文件（ImageIO）
fn write_png(img: &CGImage, path: &Path) -> Result<(), String> {
    let url = CFURL::from_file_path(path).ok_or_else(|| "截图路径无法转为 CFURL".to_string())?;
    let uti = CFString::from_str("public.png");
    let dest = unsafe { objc2_image_io::CGImageDestination::with_url(&url, &uti, 1, None) }
        .ok_or_else(|| "CGImageDestination 创建失败".to_string())?;
    unsafe {
        dest.add_image(img, None);
        if !dest.finalize() {
            return Err("PNG 写入失败(finalize=false)".to_string());
        }
    }
    Ok(())
}

/// 采集入口：枚举可截取内容 → 按"选择"建过滤器（窗口 / 显示器）→ 截图 → 等结果。
/// 目标来自 `active_target::detect_active_target`（与浮窗定位共用同一次探测）；
/// `use_window` 由配置在该消费端决定：要窗口且窗口存在 → 窗口，否则回退显示器。
/// 全程有超时（枚举 5s、截图 5s），不会像外部进程那样无限期挂住。
fn capture_with_sck(
    path: &Path,
    target: &crate::active_target::ActiveTarget,
    use_window: bool,
) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let path_owned = path.to_path_buf();
    let target = target.clone();
    let our_pid = std::process::id() as i32;
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let prepared = (|| -> Result<(Retained<SCContentFilter>, Retained<SCStreamConfiguration>, String), String> {
            if content.is_null() {
                return Err(format!("SCK 枚举失败: {}", ns_err_msg(err)));
            }
            let content: &SCShareableContent = unsafe { &*content };
            let displays = unsafe { content.displays() };
            let windows = unsafe { content.windows() };
            match target.window.as_ref().filter(|_| use_window) {
                Some(w) => {
                    let win = find_sc_window(&windows, w.id)
                        .ok_or_else(|| format!("窗口 {} 不在可截取列表(可能刚关闭)", w.id))?;
                    let frame = unsafe { win.frame() };
                    let scale = display_scale_for_center(
                        &displays,
                        frame.origin.x + frame.size.width / 2.0,
                        frame.origin.y + frame.size.height / 2.0,
                    );
                    let cfg = unsafe { SCStreamConfiguration::new() };
                    unsafe {
                        cfg.setWidth((frame.size.width * scale).round().max(1.0) as usize);
                        cfg.setHeight((frame.size.height * scale).round().max(1.0) as usize);
                        cfg.setShowsCursor(false);
                        cfg.setIgnoreShadowsSingleWindow(true);
                    }
                    let filter = unsafe {
                        SCContentFilter::initWithDesktopIndependentWindow(
                            SCContentFilter::alloc(),
                            &win,
                        )
                    };
                    Ok((filter, cfg, format!("window({} {})", w.id, w.title)))
                }
                None => {
                    let dt = &target.display;
                    let disp = find_sc_display(&displays, dt.id)
                        .ok_or_else(|| format!("显示器 {} 不在可截取列表", dt.id))?;
                    let ours = own_windows(&windows, our_pid);
                    let excludes = NSArray::from_retained_slice(&ours);
                    let filter = unsafe {
                        SCContentFilter::initWithDisplay_excludingWindows(
                            SCContentFilter::alloc(),
                            &disp,
                            &excludes,
                        )
                    };
                    let cfg = unsafe { SCStreamConfiguration::new() };
                    let scale = crate::overlay::display_scale(dt.id);
                    unsafe {
                        cfg.setWidth((dt.size.0 * scale).round().max(1.0) as usize);
                        cfg.setHeight((dt.size.1 * scale).round().max(1.0) as usize);
                        cfg.setShowsCursor(false);
                        cfg.setIgnoreShadowsDisplay(true);
                    }
                    Ok((
                        filter,
                        cfg,
                        format!(
                            "display({} id={} origin={},{} size={}x{})",
                            dt.index, dt.id, dt.origin.0, dt.origin.1, dt.size.0, dt.size.1
                        ),
                    ))
                }
            }
        })();
        match prepared {
            Ok((filter, cfg, desc)) => {
                capture_png_async(&filter, &cfg, path_owned.clone(), desc, tx.clone())
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    });
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &block,
        );
    }
    match rx.recv_timeout(SCK_TIMEOUT * 2) {
        Ok(r) => r,
        Err(_) => Err("SCK 截图超时(10s)".into()),
    }
}

/// 采集到 `dir`（每次新文件），返回文件路径与大小。
/// `mode` 是配置值（off / active_window / active_display），只在截图这一端消费：
/// - active_window：有活动窗口就截窗口；没有就回退截显示器（并在日志里说明）
/// - active_display：始终截显示器
pub fn capture(
    dir: &Path,
    tag: &str,
    target: &crate::active_target::ActiveTarget,
    mode: ShotTarget,
) -> Result<Shot, String> {
    if mode == ShotTarget::Off {
        return Err("截图已关闭(detect_active_target=off)".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("建截图目录失败: {e}"))?;
    let path = dir.join(format!("screen-{}-{tag}.png", now_ms()));
    let t0 = std::time::Instant::now();
    let use_window = mode == ShotTarget::ActiveWindow && target.window.is_some();
    if mode == ShotTarget::ActiveWindow && target.window.is_none() {
        crate::log::elog("[ctx] 光标下没有可截取窗口，回退为屏幕截图");
    }
    let desc = capture_with_sck(&path, target, use_window)?;
    let bytes = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
    if bytes == 0 {
        return Err("截图文件为空".into());
    }
    Ok(Shot {
        path,
        bytes,
        elapsed_ms: t0.elapsed().as_millis(),
        desc,
    })
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

/// 会话截图采集编排(从 recording.rs ctrl_start 下沉, 阶段A 第2步):
/// 按下时后台线程采一次屏, 结果写入共享槽(采集未完成/失败= None)。
/// 权限预检/落盘/清理/指纹日志都是截图域的事; 会话域只消费槽。
pub fn spawn_session_capture(
    app: &tauri::AppHandle,
    target: &crate::active_target::ActiveTarget,
    cfg_target: &str,
) -> std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>> {
    let slot: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let mode = parse_target(cfg_target);
    if mode == ShotTarget::Off {
        return slot;
    }
    let app = app.clone();
    let target = target.clone();
    let slot_out = slot.clone();
    std::thread::spawn(move || {
        if !preflight() {
            crate::log::elog("[ctx] 截图跳过: 未授予屏幕录制权限(可在设置页对应开关处申请)");
            return;
        }
        let dir = crate::settings::data_dir(&app);
        match capture_and_prune(&dir, "ctx", &target, mode) {
            Ok(shot) => {
                // sha256 与后续 [llm] 带图请求里的指纹对照 → 证明发出去的图就是这一张
                let fp = std::fs::read(&shot.path)
                    .map(|raw| sha256_short_of(&raw))
                    .unwrap_or_else(|_| "读取失败".into());
                crate::log::elog(&format!(
                    "[ctx] 截图已采集 path={} bytes={} ms={} sha256={} target={}",
                    shot.path.display(),
                    shot.bytes,
                    shot.elapsed_ms,
                    fp,
                    shot.desc
                ));
                *slot_out.lock().unwrap() = Some(shot.path);
            }
            Err(e) => crate::log::elog(&format!("[ctx] 截图失败, 本次不带图纠错: {e}")),
        }
    });
    slot
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
pub fn capture_and_prune(
    data_dir: &Path,
    tag: &str,
    target: &crate::active_target::ActiveTarget,
    mode: ShotTarget,
) -> Result<Shot, String> {
    let dir = data_dir.join(DIR_NAME);
    let shot = capture(&dir, tag, target, mode)?;
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
        let target = crate::active_target::ActiveTarget {
            display: crate::overlay::DisplayTarget {
                index: 1,
                id: 1,
                origin: (0.0, 0.0),
                size: (100.0, 100.0),
            },
            window: None,
        };
        let r = capture(Path::new("/dev/null/nope"), "t", &target, ShotTarget::ActiveWindow);
        assert!(r.is_err());
    }

    // —— 配置解析（旧键忽略：除 off/active_display 外一律 active_window）——

    #[test]
    fn parse_target_maps_known_values() {
        assert_eq!(parse_target("off"), ShotTarget::Off);
        assert_eq!(parse_target("active_display"), ShotTarget::ActiveDisplay);
        assert_eq!(parse_target("active_window"), ShotTarget::ActiveWindow);
    }

    #[test]
    fn parse_target_defaults_unknown_to_active_window() {
        assert_eq!(parse_target(""), ShotTarget::ActiveWindow);
        assert_eq!(parse_target("  "), ShotTarget::ActiveWindow);
        assert_eq!(parse_target("true"), ShotTarget::ActiveWindow); // 旧键遗留值不会开屏截整屏
        assert_eq!(parse_target("whatever"), ShotTarget::ActiveWindow);
        assert_eq!(parse_target(" off "), ShotTarget::Off);
    }

    // —— 光标命中窗口（纯规则）——

    fn win(id: u32, pid: i32, layer: i64, frame: (f64, f64, f64, f64)) -> WinInfo {
        WinInfo {
            id,
            pid,
            layer,
            title: String::new(),
            frame,
        }
    }

    #[test]
    fn hit_test_takes_topmost_window_under_point() {
        let wins = vec![
            win(1, 100, 0, (0.0, 0.0, 800.0, 600.0)), // 最前
            win(2, 200, 0, (0.0, 0.0, 900.0, 700.0)), // 在下面
        ];
        assert_eq!(pick_window_at(&wins, 999, (50.0, 50.0)), Some(1));
        // 点只落在后面那个窗口上 → 选它
        assert_eq!(pick_window_at(&wins, 999, (850.0, 650.0)), Some(2));
    }

    #[test]
    fn hit_test_skips_own_off_layer_and_small() {
        let wins = vec![
            win(1, 999, 0, (0.0, 0.0, 800.0, 600.0)), // 本进程 → 跳过
            win(2, 100, 3, (0.0, 0.0, 800.0, 600.0)), // 非 0 层 → 跳过
            win(3, 100, 0, (0.0, 0.0, 150.0, 100.0)), // 太小 → 跳过
            win(4, 100, 0, (0.0, 0.0, 800.0, 600.0)), // 合格
        ];
        assert_eq!(pick_window_at(&wins, 999, (10.0, 10.0)), Some(4));
    }

    #[test]
    fn hit_test_none_when_point_not_on_any_window() {
        let wins = vec![win(1, 100, 0, (0.0, 0.0, 800.0, 600.0))];
        assert_eq!(pick_window_at(&wins, 999, (900.0, 100.0)), None);
        assert_eq!(pick_window_at(&[], 999, (10.0, 10.0)), None);
    }

    #[test]
    fn wininfo_contains_point() {
        let w = win(1, 100, 0, (100.0, 200.0, 300.0, 400.0));
        assert!(w.contains((150.0, 250.0)));
        assert!(w.contains((100.0, 200.0)));
        assert!(w.contains((400.0, 600.0)));
        assert!(!w.contains((99.0, 250.0)));
        assert!(!w.contains((150.0, 601.0)));
    }

    #[test]
    fn target_name_matches_config_values() {
        assert_eq!(target_name(ShotTarget::Off), "off");
        assert_eq!(target_name(ShotTarget::ActiveWindow), "active_window");
        assert_eq!(target_name(ShotTarget::ActiveDisplay), "active_display");
    }
}
