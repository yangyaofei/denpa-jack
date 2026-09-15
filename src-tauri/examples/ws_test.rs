// Headless 引擎测试: 读真实录音 wav → 喂 Rust 版豆包引擎 → 打印事件
// 用法: cargo run --bin ws_test -- <api_key> <wav路径> [wav路径...]
// wav 要求: 16k mono int16 PCM(test_input/ 已满足)
use std::io::Read;
use denpa_jack_lib::doubao;
use std::time::Duration;

// 简化 wav 解析: 找 data chunk, 返回 i16le PCM
fn wav_pcm(path: &str) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut raw = vec![];
    f.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let mut i = 12; // 跳过 RIFF+WAVE
    while i + 8 <= raw.len() {
        let id = &raw[i..i + 4];
        let size = u32::from_le_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        if id == b"data" {
            return Ok(raw[i + 8..(i + 8 + size).min(raw.len())].to_vec());
        }
        i += 8 + size + (size % 2);
    }
    Err("no data chunk".into())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: ws_test <api_key> <wav> [...]");
        std::process::exit(1);
    }
    let key = args[1].clone();
    for wav in &args[2..] {
        println!("=== {wav}");
        let pcm = wav_pcm(wav).expect("wav 解析失败");
        println!("pcm {} bytes ({:.1}s)", pcm.len(), pcm.len() as f64 / 32000.0);
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let h = tokio::spawn(doubao::run_session(
            key.clone(),
            vec!["谢克数学".into()],
            rx,
            |ev| match ev {
                doubao::AsrEvent::Partial(t) => println!("[partial] {t}"),
                doubao::AsrEvent::Result(t) => println!("[result] {t}"),
                doubao::AsrEvent::Error(t) => println!("[error] {t}"),
            },
        ));
        // 200ms 节奏喂帧
        for chunk in pcm.chunks(6400) {
            if tx.send(doubao::Cmd::Feed(chunk.to_vec())).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = tx.send(doubao::Cmd::Finish).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), h).await;
    }
}
