// 端到端冒烟: 真实麦克风 2s → 豆包流式 → 词典 → 剪贴板交付
// 复刻 app 的 recording_start→stop 路径, 验证不再崩: cargo run --example smoke_e2e
use std::sync::mpsc;
use denpa_jack_lib::audio;
use denpa_jack_lib::doubao::{self, AsrEvent, Cmd};
use denpa_jack_lib::dict::TextCorrector;
use denpa_jack_lib::settings::DictEntry;

#[tokio::main]
async fn main() {
    let api_key = std::env::var("VOLCENGINE_API_KEY").unwrap_or_default();
    assert!(!api_key.is_empty(), "VOLCENGINE_API_KEY 未设置");

    // 1) 麦克风采集(与 app 同路径)
    let (atx, _arx) = mpsc::channel::<Vec<u8>>();
    let rec = audio::start_input(None, atx).expect("start_input 失败");
    println!("[1] 麦克风采集启动 OK, 录 2 秒…");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let total = rec.total();
    let pcm = rec.take_pcm();
    drop(rec);
    println!("[2] 采集 {total}B (pcm {}B)", pcm.len());
    assert!(total >= 9600, "音频不足");

    // 2) 豆包会话(tokio 上下文内 spawn — 复刻 tauri::async_runtime)
    let (tx, rx) = tokio::sync::mpsc::channel::<Cmd>(64);
    let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel::<AsrEvent>();
    let key = api_key.clone();
    tauri::async_runtime::spawn(async move {
        doubao::run_session(key, vec!["谢克数学".into()], rx, move |e| {
            let _ = etx.send(e);
        })
        .await;
    });
    // 桥接: 采集线程 → 会话
    let feed_tx = tx.clone();
    std::thread::spawn(move || {
        // 重放录到的 pcm(模拟实时分包)
        for chunk in pcm.chunks(6400) {
            if feed_tx.try_send(Cmd::Feed(chunk.to_vec())).is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = feed_tx.try_send(Cmd::Finish);
    });
    drop(tx);

    // 3) 收事件
    let mut raw = String::new();
    while let Some(e) = erx.recv().await {
        match e {
            AsrEvent::Partial(t) => println!("  partial: {t}"),
            AsrEvent::Result(t) => {
                raw = t;
                break;
            }
            AsrEvent::Error(e) => {
                println!("[3] 引擎错误: {e}");
                break;
            }
        }
    }
    println!("[3] 终稿: {raw}");

    // 4) 词典纠错(与 app 同路径)
    let corr = TextCorrector {
        entries: vec![DictEntry {
            term: "谢克数学".into(),
            variants: vec!["些克数学".into(), "歇课数学".into()],
            guard_words: vec![],
            boost: 3,
        }],
        normalizations: vec![],
    };
    let (final_text, _) = corr.correct(&raw);
    println!("[4] 纠错后: {final_text}");

    // 5) 剪贴板交付(pbcopy 模拟, app 内用 clipboard-manager 插件)
    use std::io::Write;
    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("pbcopy 失败");
    child.stdin.as_mut().unwrap().write_all(final_text.as_bytes()).unwrap();
    println!("[5] 已写入剪贴板 ✓  (cmd+v 粘贴需辅助功能, 冒烟跳过)");
    println!("E2E-SMOKE-PASS");
}
