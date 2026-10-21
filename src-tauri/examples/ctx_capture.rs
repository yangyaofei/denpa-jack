// 诊断: 活动目标探测 + ScreenCaptureKit 截图（窗口/屏幕两种用途），不经录音流程。
// 用法: cargo run --example ctx_capture -- <window|display|off> [输出目录]
// 默认输出到 ./tmp/ctx_probe/（在仓库根跑时落 tmp/，便于直接看图与核对尺寸）。
fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "window".to_string());
    let dir = args.next().unwrap_or_else(|| "tmp/ctx_probe".to_string());
    let dir = std::path::PathBuf::from(dir);

    let shot_mode = match mode.as_str() {
        "window" => denpa_jack_lib::screen_context::ShotTarget::ActiveWindow,
        "display" => denpa_jack_lib::screen_context::ShotTarget::ActiveDisplay,
        "off" => denpa_jack_lib::screen_context::ShotTarget::Off,
        other => {
            eprintln!("未知用途: {other}（window|display|off）");
            std::process::exit(2);
        }
    };
    println!(
        "preflight(屏幕录制权限) = {}",
        denpa_jack_lib::screen_context::preflight()
    );
    println!(
        "焦点窗口 = {:?}",
        denpa_jack_lib::screen_context::focused_window().map(|w| (w.id, w.title.clone(), w.frame))
    );
    let target = denpa_jack_lib::active_target::detect_active_target();
    println!(
        "活动目标: display={} id={} size={}x{} window={:?}",
        target.display.index,
        target.display.id,
        target.display.size.0,
        target.display.size.1,
        target
            .window
            .as_ref()
            .map(|w| (w.id, w.title.clone(), w.frame))
    );
    if shot_mode == denpa_jack_lib::screen_context::ShotTarget::Off {
        println!("off 模式不截图");
        return;
    }
    match denpa_jack_lib::screen_context::capture(&dir, "probe", &target, shot_mode) {
        Ok(shot) => {
            println!(
                "OK path={} bytes={} ms={} desc={}",
                shot.path.display(),
                shot.bytes,
                shot.elapsed_ms,
                shot.desc
            );
        }
        Err(e) => {
            println!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}
