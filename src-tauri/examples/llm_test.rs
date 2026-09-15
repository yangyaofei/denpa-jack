// LLM 润色 headless 测试: cargo run --bin llm_test
use denpa_jack_lib::llm::{polish, LlmOpts};

#[tokio::main]
async fn main() {
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    let opts = LlmOpts {
        provider: "deepseek".into(),
        model: "deepseek-chat".into(),
        api_key,
        prompt: "你是语音转写润色助手：删除废词、修复改口与同音错误、保留语气，直接输出润色文本。".into(),
        thinking: false,
    };
    let input = "那个，Phase one 阶段，我们需要把那个...那个 user ID，不对，是 user name，传给后端。然后那个...然后那个 sequel 报错的话，就重试一下。";
    let t0 = std::time::Instant::now();
    match polish(input, &["SQL".into(), "谢克数学".into()], &opts).await {
        Ok(out) => println!("PASS ({:.2}s)\nIN : {}\nOUT: {}", t0.elapsed().as_secs_f64(), input, out),
        Err(e) => println!("FAIL: {e}"),
    }
}
