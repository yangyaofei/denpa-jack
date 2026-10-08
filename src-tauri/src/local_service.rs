// 内置本地网关（denpa-asr 子进程）生命周期管理——真·子进程语义。
//
// 设计要点（2026-10-08 与用户对齐）：
// 1. 随机端口联动：APP 挑空闲端口传给网关，登记进本模块；引擎侧"内置 provider"
//    （type/name 定死的特化类型）直接取登记端口，用户零配置。
// 2. 子进程句柄全程持有（不 mem::forget——旧实现死后无人 wait 变僵尸，
//    kill(pid,0) 对僵尸恒"活着"，状态永远卡"启动中"，实测教训）。
// 3. monitor 线程负责探活/收尸：try_wait 即 reap；健康探测也在这里（结果缓存，
//    前端 status 命令纯快照，不再每次轮询干等 2s 超时）。
// 4. 停止两段式：SIGTERM(uvicorn 自带优雅退出) → 最多 5s → SIGKILL 兜底 → wait 收尸。
// 5. 日志：每次启动 truncate（日志框只属于当前实例）；读取时 \r→换行、剥 ANSI，
//    官方输出（含下载进度条）一律保留——信息越多越好，坏的是渲染不是内容。
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::settings::Config;

/// 内置引擎档案 id（type/name 定死的特化 provider；不占 asr_profiles 列表）
pub const BUILTIN_ID: &str = "builtin";

#[derive(Default)]
pub struct BuiltinGw {
    /// 子进程是否活着（快照语义：句柄本体留在登记处，这里只带事实）
    pub alive: bool,
    pub port: u16,
    pub started_at: Option<Instant>,
    pub healthy_ever: bool,
    pub healthy: bool,
    /// 子进程退出信息（"exit=1" / "signal=9"），None=没退过/还在
    pub exit: Option<String>,
}

/// 全局唯一的内置网关登记处：facts=事实快照(可 clone)，child=句柄槽(monitor 与
/// stop() 共用；stop 取走后 monitor 自然退出)
pub struct Gw {
    pub facts: Mutex<BuiltinGw>,
    pub child: Mutex<Option<Child>>,
}
pub static BUILTIN: Mutex<Option<Arc<Gw>>> = Mutex::new(None);

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return format!("{h}/{rest}");
        }
    }
    p.to_string()
}

/// 挑一个空闲随机端口（bind :0 拿到即放；本机场景竞争窗口可忽略）
fn pick_free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .map(|l| l.local_addr().map(|a| a.port()).unwrap_or(0))
        .map_err(|e| format!("挑随机端口失败: {e}"))
}

/// /health 探活（短超时；只用于内置实例的 127.0.0.1）
pub fn healthy_on(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    let req = format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 128];
    matches!(s.read(&mut buf), Ok(n) if n > 12 && String::from_utf8_lossy(&buf[..12]).starts_with("HTTP/1.1 200"))
}

/// 外部地址探活（外部网关档案用；host:port 形式）。#[allow(dead_code)]: 外部档案
/// 状态展示的预留入口, 当前 UI 只管内置实例, 保留给"外部网关"档案页将来用。
#[allow(dead_code)]
pub fn healthy_at(base_url: &str) -> bool {
    let b = base_url.trim();
    let hp = b
        .strip_prefix("ws://")
        .or_else(|| b.strip_prefix("wss://"))
        .unwrap_or(b)
        .split('/')
        .next()
        .unwrap_or("");
    let (host, port) = match hp.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(8300)),
        None => (hp.to_string(), 8300),
    };
    if host.is_empty() {
        return false;
    }
    let Ok(mut s) = TcpStream::connect((host.as_str(), port)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 128];
    matches!(s.read(&mut buf), Ok(n) if n > 12 && String::from_utf8_lossy(&buf[..12]).starts_with("HTTP/1.1 200"))
}

fn log_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    crate::settings::data_dir(app).join("local-service.log")
}

/// 拉起内置网关（双守卫：已有活的子进程→拒绝重复拉）
pub fn start(app: &tauri::AppHandle, cfg: &Config) -> Result<String, String> {
    {
        let reg = BUILTIN.lock().unwrap();
        if let Some(g) = reg.as_ref() {
            let g = g.facts.lock().unwrap();
            if g.alive {
                return Err(format!("already-starting port={}", g.port));
            }
        }
    }
    let dir = expand_home(cfg.local_service_dir.trim());
    let python = std::path::Path::new(&dir).join(".venv/bin/python");
    if !python.exists() {
        return Err(format!(
            "本地网关项目未就绪：{} 不存在（先在 denpa-asr 仓库 uv sync）",
            python.display()
        ));
    }
    let port = pick_free_port()?;
    let lp = log_path(app);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // 每次启动清空: 日志框只属于当前实例
        .open(&lp)
        .map_err(|e| format!("打不开日志 {lp:?}: {e}"))?;
    let child = Command::new(&python)
        .arg("-m")
        .arg("denpa_asr.gateway")
        .env("DENPA_ASR_PORT", port.to_string())
        .current_dir(&dir)
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("拉起网关失败: {e}"))?;
    crate::log::elog(&format!("[local-svc] 已拉起内置网关 pid={} port={} 日志={}", child.id(), port, lp.display()));

    let gw = Arc::new(Gw {
        facts: Mutex::new(BuiltinGw {
            alive: true,
            port,
            started_at: Some(Instant::now()),
            healthy_ever: false,
            healthy: false,
            exit: None,
        }),
        child: Mutex::new(Some(child)),
    });
    *BUILTIN.lock().unwrap() = Some(gw.clone());

    // monitor 线程：探活 + 收尸（try_wait 即 reap；句柄被 stop() 取走或进程退出后线程退出）
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let mut slot = gw.child.lock().unwrap();
            let Some(child) = slot.as_mut() else { break };
            match child.try_wait() {
                Ok(None) => {
                    let mut f = gw.facts.lock().unwrap();
                    let ok = healthy_on(f.port);
                    f.healthy = ok;
                    if ok {
                        f.healthy_ever = true;
                    }
                }
                Ok(Some(st)) => {
                    let mut f = gw.facts.lock().unwrap();
                    f.alive = false;
                    f.healthy = false;
                    f.exit = Some(format!("{st}"));
                    crate::log::elog(&format!("[local-svc] 内置网关退出({st}) port={}", f.port));
                    break;
                }
                Err(_) => continue, // try_wait 失败按存活处理, 下轮再看
            }
        }
    });
    Ok(format!("started port={port}"))
}

/// 两段式停止：SIGTERM(uvicorn 优雅退出) → 最多 5s → SIGKILL → wait 收尸。
/// 只作用于我们自己拉起的子进程；外部/手动实例不在登记处，天然不受影响。
pub fn stop() -> bool {
    let Some(gw) = BUILTIN.lock().unwrap().clone() else { return false };
    let mut slot = gw.child.lock().unwrap();
    let Some(mut child) = slot.take() else { return false };
    {
        let mut f = gw.facts.lock().unwrap();
        f.alive = false;
        f.healthy = false;
        f.started_at = None;
    }
    let pid = child.id();
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    // 优雅窗口: 最多 5s 等 uvicorn 自己退
    for _ in 0..10 {
        match child.try_wait() {
            Ok(Some(_)) => break,
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    let _ = child.kill(); // 已退则无操作; 还活着则 SIGKILL 兜底
    let _ = child.wait(); // 收尸
    crate::log::elog(&format!("[local-svc] 内置网关已停止(pid={pid})"));
    true
}

/// 状态快照（UI/引擎侧读；纯读不探测，健康值来自 monitor 线程的缓存）
pub fn snapshot() -> BuiltinGw {
    let reg = BUILTIN.lock().unwrap();
    match reg.as_ref() {
        Some(g) => g.facts.lock().unwrap().clone(),
        None => BuiltinGw::default(),
    }
}

/// 内置引擎的目标地址（引擎侧联动入口）；127.0.0.1:port
pub fn builtin_base_url() -> Result<String, String> {
    let s = snapshot();
    if s.healthy && s.port > 0 {
        return Ok(format!("127.0.0.1:{}", s.port));
    }
    if s.alive {
        Err("内置引擎还在启动(模型加载约 40s, 见设置-语音识别-本地网关)".into())
    } else if let Some(exit) = s.exit {
        Err(format!("内置引擎没在运行(上次退出: {exit}), 到设置里点启动"))
    } else {
        Err("内置引擎没在运行, 到设置里点启动".into())
    }
}

/// 日志尾部（读取时归一化：\r→换行、剥 ANSI CSI 转义；官方输出一律保留，只修渲染）
pub fn log_tail(app: &tauri::AppHandle, lines: usize) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(log_path(app)) else { return vec![] };
    let mut out: Vec<String> = Vec::new();
    for line in raw.replace('\r', "\n").lines() {
        let cleaned = strip_ansi(line);
        if !cleaned.trim().is_empty() {
            out.push(cleaned);
        }
    }
    let start = out.len().saturating_sub(lines);
    out[start..].to_vec()
}

/// 剥 ANSI CSI 序列（ESC '[' params 字母）——进度条彩色码去掉后就是纯文本进度行
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// 启动预热（APP 启动时异步跑）：激活档案是内置引擎且 autostart 开 → 拉起
pub fn prewarm(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(cfg) = crate::settings::get_config(app.clone()) else { return };
        if cfg.active_asr_id != BUILTIN_ID || !cfg.local_autostart {
            return;
        }
        let s = snapshot();
        if s.healthy || s.alive {
            return; // 已就绪或已在启动中
        }
        crate::log::elog("[local-svc] 预热: 拉起内置网关…");
        if let Err(e) = start(&app, &cfg) {
            crate::log::elog(&format!("[local-svc] 预热拉起失败: {e}"));
        }
    });
}

impl Clone for BuiltinGw {
    fn clone(&self) -> Self {
        Self {
            alive: self.alive,
            port: self.port,
            started_at: self.started_at,
            healthy_ever: self.healthy_ever,
            healthy: self.healthy,
            exit: self.exit.clone(),
        }
    }
}
