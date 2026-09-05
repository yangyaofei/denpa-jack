use std::time::Duration;
#[tokio::main]
async fn main() {
    let key = std::env::args().nth(1).unwrap();
    let wav = std::fs::read("../tmp/repacked.wav").unwrap();
    let part = reqwest::multipart::Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav").unwrap();
    let form = reqwest::multipart::Form::new().text("model", "glm-asr").part("file", part);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().unwrap();
    let r = client.post("https://open.bigmodel.cn/api/paas/v4/audio/transcriptions")
        .bearer_auth(&key).multipart(form).send().await.unwrap();
    println!("status={} body={}", r.status(), r.text().await.unwrap().get(..180).map(String::from).unwrap_or_default());
}
