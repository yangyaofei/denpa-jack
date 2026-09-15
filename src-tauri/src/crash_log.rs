//! 进程死亡诊断：把"应用静默消失"变成可事后定位的日志
//!
//! 背景：应用中偶尔会无声关闭，app.log 里只到正常操作就中断，系统侧也没有留下可用的崩溃报告
//! （`~/Library/Logs/DiagnosticReports` 无对应文件）。原因是日志只覆盖业务链路，覆盖不到进程死亡：
//! Rust panic 只记 message、无位置与回溯；stderr 在打包版（Finder 启动）没有任何去处；ObjC/CoreAudio
//! 抛出的异常与信号直接终止进程，不经过任何 Rust 代码；也没有"上次是否正常运行结束"的判定。
//!
//! 本模块提供六件事：
//! 1. `redirect_stderr`：把 stderr 指到 app.log —— 进程内所有 C/ObjC/Rust 的 stderr 输出都进日志
//! 2. `install_panic_hook`：panic 时记录线程名、源码位置、消息与回溯
//! 3. `install_signal_handlers`：收到致命信号时先落一行日志，再交还默认处理（保留系统崩溃报告）
//! 4. `begin_session`/`end_session`：会话文件判定"上次是否正常退出"，并扫描新的系统崩溃报告
//! 5. `spawn_heartbeat`：每分钟记录内存与线程数，用于区分"崩溃"与"被系统/外部结束"
//! 6. 自测通道：`VOICEMAC_AUTOTEST=panic`（后台线程 panic）与 `=abort`（自杀信号），用于验证以上链路
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

static STDERR_REDIRECTED: AtomicBool = AtomicBool::new(false);

/// stderr 是否已改指日志文件（log.rs 据此决定要不要再写一遍 stderr，避免每行重复）
pub fn stderr_redirected() -> bool {
    STDERR_REDIRECTED.load(Ordering::Relaxed)
}

/// 直接追加一行到 app.log。
/// 不复用 log.rs 的 LOG_LOCK：panic 与信号处理可能在持锁线程上发生，抢锁会自锁。
pub fn append_raw(line: &str) {
    append_raw_to(&crate::settings::data_dir_no_app().join("app.log"), line);
}

/// 追加到指定日志文件（会话生命周期函数用显式路径，便于测试与多目录场景）
fn append_raw_to(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let _ = writeln!(f, "[{now}] {line}");
    }
}

/// 把 stderr(2) 复制到 app.log。返回 fd 由调用方持有（进程生命周期内不关闭）。
pub fn redirect_stderr(log_path: &Path) -> Result<File, String> {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|e| format!("打开 {} 失败: {e}", log_path.display()))?;
    let rc = unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&f), libc::STDERR_FILENO) };
    if rc < 0 {
        return Err(format!("dup2 失败: {}", std::io::Error::last_os_error()));
    }
    STDERR_REDIRECTED.store(true, Ordering::Relaxed);
    Ok(f)
}

/// panic hook：记录线程名 + 源码位置 + 消息 + 回溯（原先只记 message，无法定位）
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic>".to_string()
        };
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<未命名线程>");
        append_raw(&format!("[crash] panic thread={name} at {loc}: {payload}"));
        let bt = std::backtrace::Backtrace::force_capture();
        for line in bt.to_string().lines().take(40) {
            append_raw(&format!("[crash]   {line}"));
        }
    }));
}

const FATAL_SIGNALS: [libc::c_int; 6] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGABRT,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGTRAP,
];

/// 信号处理：先写一行日志（stderr 已指向日志文件，write 是异步信号安全的），再恢复默认处理并重新抛出。
/// 恢复默认处理是必要的 —— 否则系统不会为该进程生成崩溃报告（.ips），后续就没有栈可看。
pub fn install_signal_handlers() {
    unsafe {
        for sig in FATAL_SIGNALS {
            libc::signal(sig, signal_handler as *const () as libc::sighandler_t);
        }
    }
}

extern "C" fn signal_handler(sig: libc::c_int) {
    let prefix = b"[crash] killed by signal ";
    unsafe {
        let _ = libc::write(libc::STDERR_FILENO, prefix.as_ptr() as *const libc::c_void, prefix.len());
        let mut digits = [0u8; 4];
        let mut n = sig;
        let mut i = digits.len();
        if n == 0 {
            i -= 1;
            digits[i] = b'0';
        }
        while n > 0 && i > 0 {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        let name = signal_name(sig);
        let _ = libc::write(libc::STDERR_FILENO, digits[i..].as_ptr() as *const libc::c_void, digits.len() - i);
        let _ = libc::write(libc::STDERR_FILENO, b" (" as *const u8 as *const libc::c_void, 2);
        let _ = libc::write(libc::STDERR_FILENO, name.as_ptr() as *const libc::c_void, name.len());
        let _ = libc::write(libc::STDERR_FILENO, b")\n" as *const u8 as *const libc::c_void, 2);
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

fn signal_name(sig: libc::c_int) -> &'static [u8] {
    match sig {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGBUS => b"SIGBUS",
        libc::SIGABRT => b"SIGABRT",
        libc::SIGILL => b"SIGILL",
        libc::SIGFPE => b"SIGFPE",
        libc::SIGTRAP => b"SIGTRAP",
        _ => b"unknown",
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Session {
    pub pid: u32,
    pub started_at_ms: u64,
    pub version: String,
}

fn session_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".session.json")
}

/// 启动时调用：判定上次是否正常退出、上报新的系统崩溃报告、写下本次会话。
pub fn begin_session(data_dir: &Path) {
    let path = session_path(data_dir);
    let log_path = data_dir.join("app.log");
    // 先取上次日志的最后写入时间：约等于上次进程仍然活着的最后时刻
    let last_write = std::fs::metadata(&log_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let prev = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Session>(&s).ok());

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut reported = load_seen_reports(data_dir);
    if let Some(p) = &prev {
        let alive_s = now_ms.saturating_sub(p.started_at_ms) / 1000;
        let last_write_s = match last_write {
            Some(t) => format!("{t}"),
            None => "<无>".to_string(),
        };
        append_raw_to(&log_path, &format!(
            "[crash] 上次会话未标记正常退出(疑似崩溃或被强制结束): pid={} 启动时刻={}ms 距本次启动={}s 上次日志最后写入={}",
            p.pid, p.started_at_ms, alive_s, last_write_s
        ));
    }

    // 上报新增的系统崩溃报告（.ips）：文件名 + 前几行关键字段
    match scan_new_crash_reports(&reported) {
        Ok((new_files, kept)) => {
            for (name, head) in &new_files {
                append_raw_to(&log_path, &format!("[crash] 发现系统崩溃报告 {name}"));
                for line in head {
                    append_raw_to(&log_path, &format!("[crash]   {line}"));
                }
            }
            reported = kept;
        }
        Err(e) => append_raw(&format!("[crash] 扫描崩溃报告失败: {e}")),
    }

    save_seen_reports(data_dir, &reported);
    let me = Session {
        pid: std::process::id(),
        started_at_ms: now_ms,
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Ok(s) = serde_json::to_string_pretty(&me) {
        let _ = std::fs::write(&path, s);
    }
    append_raw_to(&log_path, &format!("[app] 会话开始 pid={}", me.pid));
}

/// 正常退出时调用：清掉会话文件（下次启动就不会报"未正常退出"）
pub fn end_session(data_dir: &Path) {
    append_raw_to(&data_dir.join("app.log"), "[app] 正常退出");
    let _ = std::fs::remove_file(session_path(data_dir));
}

/// 已上报过的崩溃报告文件名（独立文件，不随正常退出清理；否则每次正常退出后会重复上报）
fn seen_reports_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".crash_reports_seen.json")
}

fn load_seen_reports(data_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(seen_reports_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_seen_reports(data_dir: &Path, list: &[String]) {
    if let Ok(s) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(seen_reports_path(data_dir), s);
    }
}

/// 从崩溃报告里挑出可用于定位的字段行（跳过 threads/usedImages 这类巨行）
pub(crate) fn crash_report_head(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| {
            (l.contains("\"exception\"") || l.contains("\"termination\"") || l.contains("\"signal\""))
                && !l.contains("\"threads\"")
        })
        .take(4)
        .map(|l| l.trim().chars().take(220).collect())
        .collect()
}

/// 扫描 `~/Library/Logs/DiagnosticReports` 里本应用的新崩溃报告。
/// 返回（新增报告的 文件名 + 头部关键行，全部已上报过的文件名）。
fn scan_new_crash_reports(
    already: &[String],
) -> Result<(Vec<(String, Vec<String>)>, Vec<String>), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let dir = PathBuf::from(home).join("Library/Logs/DiagnosticReports");
    if !dir.is_dir() {
        return Ok((Vec::new(), already.to_vec()));
    }
    let mut mine: Vec<(String, PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // 进程名含连字符或空格，两种都收
        if (name.starts_with("denpa-jack") || name.starts_with("Denpa Jack"))
            && name.ends_with(".ips")
        {
            mine.push((name, e.path()));
        }
    }
    let mut fresh = Vec::new();
    let mut all: Vec<String> = already.to_vec();
    for (name, path) in mine {
        if already.iter().any(|a| a == &name) {
            continue;
        }
        let head = std::fs::read_to_string(&path)
            .ok()
            .map(|s| crash_report_head(&s))
            .unwrap_or_default();
        fresh.push((name.clone(), head));
        all.push(name);
    }
    Ok((fresh, all))
}

/// 心跳：每分钟记录内存与线程数。
/// 用途：若日志在某个时刻突然中断、且没有 panic/信号记录，则更可能是被外部结束或系统回收。
pub fn spawn_heartbeat() {
    std::thread::spawn(|| {
        let started = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let (rss_mb, threads) = proc_stats();
            append_raw(&format!(
                "[hb] uptime={}s rss={}MB threads={}",
                started.elapsed().as_secs(),
                rss_mb.map(|v| v.to_string()).unwrap_or_else(|| "?".to_string()),
                threads.map(|v| v.to_string()).unwrap_or_else(|| "?".to_string())
            ));
        }
    });
}

#[repr(C)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    pti_total_user: u64,
    pti_total_system: u64,
    pti_threads_user: u64,
    pti_threads_system: u64,
    pti_policy: i32,
    pti_faults: i32,
    pti_pageins: i32,
    pti_cow_faults: i32,
    pti_messages_sent: i32,
    pti_messages_received: i32,
    pti_syscalls_mach: i32,
    pti_syscalls_unix: i32,
    pti_csw: i32,
    pti_threadnum: i32,
    pti_numrunning: i32,
    pti_priority: i32,
}

const PROC_PIDTASKINFO: libc::c_int = 4;

/// 取本进程常驻内存(MB)与线程数；取不到返回 (None, None)
fn proc_stats() -> (Option<u64>, Option<i32>) {
    let mut info: ProcTaskInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;
    let rc = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut ProcTaskInfo as *mut libc::c_void,
            size,
        )
    };
    if rc != size {
        return (None, None);
    }
    (Some(info.pti_resident_size / 1024 / 1024), Some(info.pti_threadnum))
}

/// 自测通道：验证诊断链路本身是否有效
///   VOICEMAC_AUTOTEST=panic  后台线程 panic → 应出现 [crash] panic（含位置与回溯）
///   VOICEMAC_AUTOTEST=abort  进程自杀 SIGABRT → 应出现 [crash] killed by signal 6，
///                            且下次启动应报告"上次会话未标记正常退出"
pub fn maybe_spawn_autotest() {
    match std::env::var("VOICEMAC_AUTOTEST").as_deref() {
        Ok("panic") => {
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                panic!("AUTOTEST panic: 验证崩溃诊断链路");
            });
        }
        Ok("abort") => {
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                unsafe { libc::raise(libc::SIGABRT) };
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_path_is_in_data_dir() {
        let p = session_path(Path::new("/tmp/x"));
        assert!(p.ends_with(".session.json"));
    }

    #[test]
    fn session_roundtrip() {
        let s = Session {
            pid: 7,
            started_at_ms: 123,
            version: "0.1.0".into(),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&j).unwrap(), s);
    }

    #[test]
    fn signal_names_are_stable() {
        assert_eq!(signal_name(libc::SIGSEGV), b"SIGSEGV");
        assert_eq!(signal_name(libc::SIGABRT), b"SIGABRT");
        assert_eq!(signal_name(999), b"unknown");
    }

    #[test]
    fn proc_stats_readable() {
        let (rss, threads) = proc_stats();
        // 在有权限的平台上应能读到；读不到也不 panic
        if let Some(t) = threads {
            assert!(t >= 1);
        }
        if let Some(m) = rss {
            assert!(m < 100_000);
        }
    }

    #[test]
    fn stderr_flag_default_false() {
        // 测试进程没调用 redirect_stderr，应为 false
        assert!(!stderr_redirected());
    }

    #[test]
    fn begin_session_writes_file_then_end_session_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        begin_session(p);
        assert!(session_path(p).is_file(), "开始会话应写下会话文件");
        let prev: Session = serde_json::from_str(&std::fs::read_to_string(session_path(p)).unwrap()).unwrap();
        assert_eq!(prev.pid, std::process::id());
        end_session(p);
        assert!(!session_path(p).exists(), "正常退出应清掉会话文件");
    }

    #[test]
    fn crash_report_head_skips_threads_line() {
        let sample = "{\"exception\" : {\"type\":\"EXC_CRASH\",\"signal\":\"SIGABRT\"},\n\
             \"threads\" : [{\"threadState\":{\"x\":[{\"value\":1}]}}],\n\
             \"termination\" : {\"flags\":0}}";
        let head = crash_report_head(sample);
        assert!(head.iter().any(|l| l.contains("EXC_CRASH")));
        assert!(head.iter().any(|l| l.contains("termination")));
        assert!(!head.iter().any(|l| l.contains("threads")), "巨行应被跳过：{head:?}");
    }

    #[test]
    fn seen_reports_persist_in_separate_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        save_seen_reports(p, &["a.ips".to_string()]);
        assert_eq!(load_seen_reports(p), vec!["a.ips".to_string()]);
        end_session(p);
        assert_eq!(load_seen_reports(p), vec!["a.ips".to_string()], "正常退出不应丢掉去重记录");
    }

    #[test]
    fn begin_session_reports_previous_unclean_exit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        // 伪造一份"上次没正常退出"的会话文件
        let stale = Session {
            pid: 4242,
            started_at_ms: 1,
            version: "0".into(),
        };
        std::fs::write(session_path(p), serde_json::to_string(&stale).unwrap()).unwrap();
        begin_session(p);
        let log = std::fs::read_to_string(p.join("app.log")).unwrap();
        assert!(log.contains("上次会话未标记正常退出"), "应记录异常退出：{log}");
        assert!(log.contains("pid=4242"), "应带上上次的 pid：{log}");
    }
}
