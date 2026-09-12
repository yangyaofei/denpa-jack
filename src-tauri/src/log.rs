// 文件日志(C18: 关键链路落文件, 防统一日志限流丢行)
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use tauri::Manager;

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn app_log_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub fn log(app: &tauri::AppHandle, msg: &str) {
    let _g = LOG_LOCK.lock().unwrap();
    if let Some(dir) = app_log_dir(app) {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(dir.join("app.log")) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let _ = writeln!(f, "[{now}] {msg}");
        }
    }
    eprintln!("[voice] {msg}");
}

/// 无 app 句柄时的诊断日志: stderr + app.log 双写(打包版 stderr 不可见)
pub fn elog(msg: &str) {
    eprintln!("[vm] {msg}");
    use std::io::Write;
    // C53: 与 log() 共用 LOG_LOCK——否则多线程并发 append 撕行(实测 "[diag] ...[ts] setup done" 互相嵌入,
    // grep 按"key-engine"检索时整行丢失, 造成"日志没打"的误判)
    let _g = LOG_LOCK.lock().unwrap();
    let dir = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_default(),
    )
    .join("Library/Application Support/com.yangyaofei.tauri-app");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("app.log"))
    {
        let _ = writeln!(f, "[diag] {msg}");
    }
}
