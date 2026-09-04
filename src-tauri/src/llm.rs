// LLM 润色(Rust 移植自 Swift LLMPolisher)
// 工具调用强制 submit_corrected_text; 无 tool_call 时降级取 content
use serde_json::{json, Value};

pub struct LlmOpts {
    pub provider: String, // deepseek | zhipu | openai-compatible
    pub base_url: String, // 空=按 provider 默认
    pub model: String,
    pub api_key: String,
    pub prompt: String,
    /// 思维链开关
    pub thinking: bool,
    /// 思考强度: low / high
    pub effort: String,
}

/// 按 provider 拉取可用模型列表(OpenAI 兼容 GET /models)
pub fn default_base_url(provider: &str) -> &'static str {
    if provider == "deepseek" {
        "https://api.deepseek.com"
    } else {
        "https://open.bigmodel.cn/api/paas/v4"
    }
}

pub async fn list_models(provider: &str, base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "{}/models",
        if base_url.is_empty() { default_base_url(provider).to_string() } else { base_url.trim_end_matches('/').to_string() }
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp: serde_json::Value = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let mut out: Vec<String> = resp["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    out.sort();
    Ok(out)
}

/// LLM 调用全量落盘: llm_logs/{ts}-{provider}.json (请求/响应原文, key 脱敏)
fn dump_call(dir_tag: &str, url: &str, payload: &serde_json::Value, resp: &str) {
    use std::io::Write;
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/com.yangyaofei.tauri-app/llm_logs");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = format!("{ts}-{dir_tag}.json");
    let body = serde_json::json!({
        "ts": ts,
        "url": url,
        "request": payload,
        "response_raw": resp,
    });
    if let Ok(mut f) = std::fs::File::create(dir.join(&name)) {
        let _ = writeln!(f, "{body:#}");
    }
}

pub async fn polish(text: &str, dict_terms: &[String], o: &LlmOpts) -> Result<String, String> {
    let base = if o.base_url.is_empty() {
        default_base_url(&o.provider).to_string()
    } else {
        o.base_url.trim_end_matches('/').to_string()
    };
    let url = format!("{base}/chat/completions");
    let key = o.api_key.clone();
    let sys = format!(
        "{}\n用户术语表（遇到同音/近音或拼写相近的错误时, 必须纠正为术语表写法; 括号内为常见误写）：\n{}\n\n输出要求：你必须调用 submit_corrected_text 工具，把修正后的完整文本通过 corrected_text 参数提交；不要直接输出正文。",
        o.prompt,
        dict_terms.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n")
    );
    let mut payload = json!({
        "model": o.model,
        "temperature": 0.1,
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": text}
        ],
        "tools": [{
            "type": "function",
            "function": {
                "name": "submit_corrected_text",
                "description": "提交纠错后的完整文本",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "corrected_text": {"type": "string", "description": "纠错后的完整文本"}
                    },
                    "required": ["corrected_text"]
                }
            }
        }]
        // 不强制 tool_choice: 兼容端支持不一, 模型自主决定(可带 thinking); 解析侧两种结果都接
    });
    // 思考深度(官网 docs.bigmodel.cn 对话补全 OpenAPI):
    // zhipu: reasoning_effort 是顶层独立参数, thinking 开启时生效, 默认 max;
    //        GLM-5.3/5.3-FLASH 仅支持 low/high/max → 最小档=low
    // 两个独立参数, 显式传:
    //   thinking: 智谱=思维链开关(仅智谱有此参数); deepseek 无此参数不传
    //   reasoning_effort: 两家同参数(官网均为 low/high, 默认 max/high)
    // 直传: 选项是什么就发什么, 不做映射/降级/强制(错误由服务端返回, 测试按钮可见)
    payload["thinking"] = json!({"type": if o.thinking { "enabled" } else { "disabled" }});
    payload["reasoning_effort"] = json!(o.effort);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    let resp = client
        .post(url.clone())
        .header("Authorization", format!("Bearer {key}"))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("LLM 请求失败: {e}"))?;
    let status = resp.status();
    let raw = resp.text().await.map_err(|e| format!("LLM 响应读取失败: {e}"))?;
    dump_call(
        &format!("{}-{}ms", &o.provider, t0.elapsed().as_millis()),
        &url,
        &payload,
        &raw,
    );
    let body: Value = serde_json::from_str(&raw).map_err(|e| format!("LLM 响应解析失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM HTTP {status}: {}", body));
    }
    let choice = &body["choices"][0];
    if let Some(args) = choice["message"]["tool_calls"][0]["function"]["arguments"].as_str() {
        if let Ok(v) = serde_json::from_str::<Value>(args) {
            if let Some(t) = v["corrected_text"].as_str() {
                return Ok(t.to_string());
            }
        }
    }
    if let Some(c) = choice["message"]["content"].as_str() {
        return Ok(c.to_string());
    }
    Err(format!("LLM 无有效输出: {body}"))
}
