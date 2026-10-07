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
/// 日志重定向到数据目录 local-service.log；PID 记入 AppState（退出时只杀自己的）
pub fn start(app: &tauri::AppHandle, cfg: &Config) -> Result<u32, String> {
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
    app.state::<std::sync::Mutex<crate::AppState>>()
        .lock()
        .unwrap()
        .gateway_pid = Some(pid);
    Ok(pid)
}

/// 停止：只杀 APP 自己拉起的实例（手动 `denpa-asr serve` 的不属于我们，不动）
pub fn stop(app: &tauri::AppHandle) -> bool {
    let pid = app
        .state::<std::sync::Mutex<crate::AppState>>()
        .lock()
        .unwrap()
        .gateway_pid
        .take();
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
