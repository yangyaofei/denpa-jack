// 历史文件诊断: 逐行用 HistoryRecord 强类型解析, 打出解析失败的行号与 serde 错误
// (find_by_ts "强类型未命中但 Value 兜底命中" 的定位探针, 2026-11-03)
use denpa_jack_lib::pipeline::history::HistoryRecord;

fn main() {
    let dir = std::env::var("HOME")
        .map(|h| format!("{h}/Library/Application Support/io.github.yangyaofei.denpajack"))
        .unwrap_or_default();
    let path = std::path::Path::new(&dir).join("history.jsonl");
    let raw = std::fs::read_to_string(&path).expect("read");
    let mut ok = 0;
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryRecord>(line) {
            Ok(_) => ok += 1,
            Err(e) => println!("行{} 失败: {} | 行首: {}", i + 1, e, &line[..line.len().min(110)]),
        }
    }
    println!("解析成功 {ok} 行 / 共 {}", raw.lines().filter(|l| !l.trim().is_empty()).count());
}
