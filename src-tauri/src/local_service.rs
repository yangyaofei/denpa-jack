// 本地网关（local 引擎的后端进程）生命周期管理。
// 方案 B（用户定则）：APP 拉子进程——启用 local 引擎时自动探活/拉起 denpa-asr，
// APP 退出时只收掉**自己拉起的**子进程（手动起的实例属于用户，不动）。
// 方案 A（手动 `denpa-asr serve`）与本方案共存：探活发现已在跑就直接复用。
use tauri::Manager;

use crate::settings::Config;

/// 从档案 base_url 解析端口（ws://host:port/...；默认/解析失败 8300）
pub fn port_of(base_url: &str) -> u16 {
    let b = base_url.trim();
    let hostpart = b
        .strip_prefix("ws://")
        .or_else(|| b.strip_prefix("wss://"))
        .unwrap_or(b);
    hostpart
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit(':')
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or(8300)
}

/// /health 探活：裸 TCP + HTTP GET（零新依赖；只连本机/局域网，2s 超时）
pub fn healthy(base_url: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let host = {
        let b = base_url.trim();
        let hp = b.strip_prefix("ws://").or_else(|| b.strip_prefix("wss://")).unwrap_or(b);
        hp.split('/').next().unwrap_or("").to_string()
    };
    let host = host.split(':').next().unwrap_or("127.0.0.1").to_string();
    let Ok(mut s) = TcpStream::connect((host.as_str(), port_of(base_url))) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    match s.read(&mut buf) {
        Ok(n) if n > 12 => String::from_utf8_lossy(&buf[..12]).starts_with("HTTP/1.1 200"),
        _ => false,
    }
}

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return format!("{h}/{rest}");
        }
    }
    p.to_string()
}

/// 拉起网关子进程：{local_service_dir}/.venv/bin/python -m denpa_asr.gateway
/// 日志重定向到数据目录 local-service.log（卡片里可实时看，等效 shell 输出）；
/// PID 记入 AppState（退出时只杀自己的）。
///
/// 双实例守卫（实测教训：预热已拉 A、用户再按启动又拉 B，B 撞端口 Errno 48 死掉，
/// 白加载 40s 还把状态搞乱）：
/// - 端口已健康 → 不拉（复用现有实例）
/// - 我们已有活着的 pid（正在启动中）→ 不拉，返回 already-starting
pub fn start(app: &tauri::AppHandle, cfg: &Config) -> Result<u32, String> {
    {
        let state = app.state::<std::sync::Mutex<crate::AppState>>();
        let st = state.lock().unwrap();
        if let Some(pid) = st.gateway_pid {
            if process_alive(pid) {
                return Err(format!("already-starting pid={pid}"));
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
    let log_path = crate::settings::data_dir(app).join("local-service.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打不开日志 {log_path:?}: {e}"))?;
    // 分隔线让用户在日志里分得清这是第几次拉起
    {
        use std::io::Write;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut w = std::io::BufWriter::new(&log);
        let _ = writeln!(w, "\n── APP 拉起网关 unix={ts} ──");
    }
    let child = std::process::Command::new(&python)
        .arg("-m")
        .arg("denpa_asr.gateway")
        .current_dir(&dir)
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("拉起网关失败: {e}"))?;
    let pid = child.id();
    // 丢弃 Child 句柄但保留 PID：进程独立存活, 由 AppState 在退出时按 PID 收掉
    std::mem::forget(child);
    crate::log::elog(&format!("[local-svc] 已拉起网关 pid={pid} 日志={}", log_path.display()));
    let state = app.state::<std::sync::Mutex<crate::AppState>>();
    let mut st = state.lock().unwrap();
    st.gateway_pid = Some(pid);
    st.gateway_started_at = Some(std::time::Instant::now());
    Ok(pid)
}

/// 卡片用的完整状态（替代过去的布尔探活）：
/// running=健康检查通过（pid 为空表示是外部/手动实例，APP 不管其生命周期）；
/// starting=我们拉起的进程还活着但未就绪(带已耗时)；failed=进程在就绪前退出；
/// stopped=无进程。同名裁决见 start() 的双实例守卫。
pub struct SvcStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub elapsed_secs: Option<u64>,
}

pub fn status(app: &tauri::AppHandle) -> SvcStatus {
    let (pid, started) = {
        let state = app.state::<std::sync::Mutex<crate::AppState>>();
        let st = state.lock().unwrap();
        (st.gateway_pid, st.gateway_started_at)
    };
    let base = crate::recording::resolve_asr(&crate::settings::get_config(app.clone()).unwrap_or_default())
        .map(|(p, _)| p.base_url)
        .unwrap_or_default();
    if healthy(&base) {
        // 端口活着的可能不是我们拉的那个(手动 serve / 旧实例): pid 已死就说实话
        let pid_alive = pid.map(process_alive).unwrap_or(false);
        return SvcStatus {
            state: "running".into(),
            pid: if pid_alive { pid } else { None },
            elapsed_secs: started.filter(|_| pid_alive).map(|t| t.elapsed().as_secs()),
        };
    }
    match pid {
        Some(pid) if process_alive(pid) => SvcStatus {
            state: "starting".into(),
            pid: Some(pid),
            elapsed_secs: started.map(|t| t.elapsed().as_secs()),
        },
        Some(pid) => {
            // 进程死了但健康检查也没过 → 启动失败，清掉引用让状态不再误导
            let state = app.state::<std::sync::Mutex<crate::AppState>>();
            state.lock().unwrap().gateway_pid = None;
            crate::log::elog(&format!("[local-svc] 网关进程 pid={pid} 在就绪前退出了(启动失败, 看 local-service.log)"));
            SvcStatus { state: "failed".into(), pid: None, elapsed_secs: None }
        }
        None => SvcStatus { state: "stopped".into(), pid: None, elapsed_secs: None },
    }
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// 读日志尾部（网关 stdout/stderr 都在 local-service.log；等效 shell 输出）
pub fn log_tail(app: &tauri::AppHandle, lines: usize) -> Vec<String> {
    let path = crate::settings::data_dir(app).join("local-service.log");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    let mut out: Vec<String> = content.lines().rev().take(lines).map(String::from).collect();
    out.reverse();
    out
}

/// 停止：只杀 APP 自己拉起的实例（手动 `denpa-asr serve` 的不属于我们，不动）
pub fn stop(app: &tauri::AppHandle) -> bool {
    let pid = {
        let state = app.state::<std::sync::Mutex<crate::AppState>>();
        let mut st = state.lock().unwrap();
        st.gateway_started_at = None;
        st.gateway_pid.take()
    };
    if let Some(pid) = pid {
        crate::log::elog(&format!("[local-svc] APP 退出, 收掉自己拉起的网关 pid={pid}"));
        unsafe {
            // SIGTERM: 让网关正常退出(仅当我们仍是它的属主场景下调用)
            if libc::kill(pid as i32, libc::SIGTERM) == 0 {
                return true;
            }
        }
    }
    false
}

/// 启动预热（setup 里异步跑, 不阻塞启动）：启用 local 引擎且网关不在 → 拉起
pub fn prewarm(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(cfg) = crate::settings::get_config(app.clone()) else { return };
        let provider = crate::settings::get_config(app.clone())
            .ok()
            .and_then(|c| {
                c.asr_profiles
                    .iter()
                    .find(|p| p.id == c.active_asr_id)
                    .map(|p| p.provider.clone())
            })
            .unwrap_or_default();
        if provider != "local" || !cfg.local_autostart {
            return;
        }
        let base = crate::settings::get_config(app.clone())
            .ok()
            .and_then(|c| {
                c.asr_profiles
                    .iter()
                    .find(|p| p.id == c.active_asr_id)
                    .map(|p| p.base_url.clone())
            })
            .unwrap_or_default();
        if healthy(&base) {
            crate::log::elog("[local-svc] 网关已在运行(手动/上次残留), 复用");
            return;
        }
        crate::log::elog("[local-svc] 网关未运行, 自动拉起(模型加载 ~20-40s)…");
        if let Err(e) = start(&app, &cfg) {
            crate::log::elog(&format!("[local-svc] 自动拉起失败: {e}"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::port_of;

    #[test]
    fn port_rules() {
        assert_eq!(port_of(""), 8300);
        assert_eq!(port_of("127.0.0.1:9000"), 9000);
        assert_eq!(port_of("ws://127.0.0.1:8300"), 8300);
        assert_eq!(port_of("ws://192.168.1.5:9100/v1"), 9100);
        assert_eq!(port_of("ws://a.b/"), 8300);
        assert_eq!(port_of("ws://a.b:xx/"), 8300);
    }
}
