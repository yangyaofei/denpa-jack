// LLM 润色 headless 测试: cargo run --example llm_test
use denpa_jack_lib::llm::{polish, LlmOpts};

#[tokio::main]
async fn main() {
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    let opts = LlmOpts {
        provider: "deepseek".into(),
        base_url: std::env::var("BASE_URL").unwrap_or_default(),
        model: std::env::var("MODEL").unwrap_or_else(|_| "deepseek-chat".into()),
        api_key,
        prompt: "你是语音转写润色助手：删除废词、修复改口与同音错误、保留语气，直接输出润色文本。".into(),
        thinking: false,
        effort: "low".into(),
        timeout_secs: 30,
        max_tokens: 0,
        retries: 1,
    };
    let input = "那个，Phase one 阶段，我们需要把那个...那个 user ID，不对，是 user name，传给后端。然后那个...然后那个 sequel 报错的话，就重试一下。";
    let t0 = std::time::Instant::now();
    match polish(input, &["SQL".into(), "谢克数学".into()], &opts, None).await {
        Ok(out) => println!("PASS ({:.2}s)\nIN : {}\nOUT: {}", t0.elapsed().as_secs_f64(), input, out.text),
        Err(e) => println!("FAIL: {e}"),
    }
}
