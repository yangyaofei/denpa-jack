// list_models 实测(两家供应商)
#[tokio::main]
async fn main() {
    let key = std::env::var("DS_KEY").unwrap_or_default();
    let r = denpa_jack_lib::llm::list_models("deepseek", "", &key).await;
    println!("deepseek models: {:?}", r.map(|v| v.join(",")));
    let zp = std::env::var("ZP_KEY").unwrap_or_default();
    let r2 = denpa_jack_lib::llm::list_models("zhipu", "", &zp).await;
    println!("zhipu models: {:?}", r2.map(|v| format!("{} 个", v.len())));
}
