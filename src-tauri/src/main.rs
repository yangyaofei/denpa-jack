// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 死亡诊断必须最早安装：Tauri/AppKit 初始化阶段的崩溃同样要能落到日志里
    let log_path = denpa_jack_lib::settings::data_dir_no_app().join("app.log");
    match denpa_jack_lib::crash_log::redirect_stderr(&log_path) {
        Ok(f) => std::mem::forget(f), // fd 交给进程持有，不关闭
        Err(e) => eprintln!("stderr 落盘失败(仍继续启动): {e}"),
    }
    denpa_jack_lib::crash_log::install_panic_hook();
    denpa_jack_lib::crash_log::install_signal_handlers();
    denpa_jack_lib::crash_log::maybe_spawn_autotest();
    denpa_jack_lib::run()
}
