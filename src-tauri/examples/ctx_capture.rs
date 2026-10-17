//! 一次性验证: 实测「只截最前面那个 App 的前窗」这条路径（解析窗口列表 → 挑目标 → screencapture -l）。
//! 用法: cargo run --example ctx_capture [输出目录]
//! 输出: OK path=… bytes=… ms=… window=Some(id) / ERR 原因
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "tmp_ctx_probe".to_string());
    let p = std::path::PathBuf::from(dir);
    for i in 1..=3 {
        match denpa_jack_lib::screen_context::capture(&p, "probe") {
            Ok(s) => println!(
                "[{i}] OK window={:?} bytes={} ms={} path={}",
                s.window,
                s.bytes,
                s.elapsed_ms,
                s.path.display()
            ),
            Err(e) => println!("[{i}] ERR {e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
