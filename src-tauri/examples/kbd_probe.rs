// 键盘原始事件探针: 直接用 handy-keys KeyboardListener 打印每个事件(毫秒时间戳),
// 不经过我们的 manager/协调器——用于区分"系统丢事件" vs "我们的接线丢事件"。
// 用法: cargo run --release --example kbd_probe (Ctrl+C 退出)
use handy_keys::KeyboardListener;

fn main() {
    println!("kbd_probe: 打印全部原始键盘事件(按 Ctrl+C 退出)");
    let listener = KeyboardListener::new().expect("listener 创建失败(需辅助功能权限)");
    let t0 = std::time::Instant::now();
    loop {
        match listener.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(ev) => {
                let ms = t0.elapsed().as_millis();
                let key = ev.key.map(|k| format!("{k:?}")).unwrap_or_else(|| "-".into());
                let changed = ev
                    .changed_modifier
                    .map(|m| format!("{m:?}"))
                    .unwrap_or_else(|| "-".into());
                let hotkey = ev
                    .as_hotkey()
                    .map(|h| h.to_handy_string())
                    .unwrap_or_default();
                println!("[{ms:>6}ms] down={} key={key:<12} changed={changed:<12} mods={:?} hotkey={hotkey}",
                    ev.is_key_down, ev.modifiers);
            }
            Err(e) => {
                if format!("{e:?}").contains("Timeout") {
                    continue;
                }
                eprintln!("listener 错误: {e:?}");
                break;
            }
        }
    }
}
