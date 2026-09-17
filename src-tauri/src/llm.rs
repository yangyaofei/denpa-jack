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
    let dir = crate::settings::data_dir_no_app().join("llm_logs");
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmOut {
    pub text: String,
    /// C50: 思考模型链路(reasoning_content), 存入历史供人工审查
    pub thinking: Option<String>,
}

/// 构造 user 消息内容。
/// 带截图时为 [图片, 文本] 数组——官方限制图片只能出现在 user 消息里（放 system/assistant 会 400）；
/// 不带截图时保持纯字符串，与开关关闭前的行为完全一致。
fn build_user_content(text: &str, image_data_url: Option<String>) -> Value {
    match image_data_url {
        Some(url) => json!([
            {"type": "image_url", "image_url": {"url": url}},
            {"type": "text", "text": format!("参考随附截图中的上下文，修正下面这段转写：\n{text}")}
        ]),
        None => json!(text),
    }
}

pub async fn polish(
    text: &str,
    dict_terms: &[String],
    o: &LlmOpts,
    screenshot: Option<&std::path::Path>,
) -> Result<LlmOut, String> {
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
    // 屏幕上下文：截图 base64 内联进 user 消息；读图失败只记日志并退回纯文本纠错（不阻塞交付）
    let mut shot_note: Option<String> = None;
    let image_url = match screenshot {
        Some(p) => match crate::screen_context::to_data_url(p) {
            Ok(u) => {
                let mb = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / 1024.0 / 1024.0;
                shot_note = Some(format!("<截图内联: {} ({mb:.2} MB)>", p.display()));
                Some(u)
            }
            Err(e) => {
                crate::log::elog(&format!("[llm] 截图读取失败, 退回纯文本纠错: {e}"));
                None
            }
        },
        None => None,
    };
    let mut payload = json!({
        "model": o.model,
        "temperature": 0.1,
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": build_user_content(text, image_url)}
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
    // 落盘前把 base64 换成文件引用：请求里是完整内联数据，日志里只留路径与大小（截图本体已存 screen_context/）
    let mut dump_payload = payload.clone();
    if let Some(note) = &shot_note {
        if let Some(arr) = dump_payload["messages"][1]["content"].as_array_mut() {
            if let Some(first) = arr.first_mut() {
                first["image_url"]["url"] = json!(note);
            }
        }
    }
    dump_call(
        &format!("{}-{}ms", &o.provider, t0.elapsed().as_millis()),
        &url,
        &dump_payload,
        &raw,
    );
    let body: Value = serde_json::from_str(&raw).map_err(|e| format!("LLM 响应解析失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM HTTP {status}: {}", body));
    }
    let thinking = extract_thinking(&body);
    match extract_text(&body) {
        Some(t) => Ok(LlmOut { text: t, thinking }),
        None => Err(format!("LLM 无有效输出: {body}")),
    }
}

/// C50: 提取思考链(deepseek/zhipu 均为 message.reasoning_content)
fn extract_thinking(body: &Value) -> Option<String> {
    body["choices"][0]["message"]["reasoning_content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

/// 从响应体提取修正文本: tool_calls[0].args.corrected_text 优先, 降级 message.content
fn extract_text(body: &Value) -> Option<String> {
    let choice = &body["choices"][0];
    if let Some(args) = choice["message"]["tool_calls"][0]["function"]["arguments"].as_str() {
        if let Ok(v) = serde_json::from_str::<Value>(args) {
            if let Some(t) = v["corrected_text"].as_str() {
                return Some(t.to_string());
            }
        }
    }
    // 空 content 返回 None(让上层降级/报错)——Some("") 会用空串覆盖词典结果
    choice["message"]["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_content_without_image_is_plain_text() {
        // 开关关闭时的行为必须与加功能前一致：纯字符串
        let c = build_user_content("原始文本", None);
        assert_eq!(c, json!("原始文本"));
    }

    #[test]
    fn user_content_with_image_puts_image_first_and_keeps_text() {
        let c = build_user_content("原始文本", Some("data:image/png;base64,AAA".to_string()));
        let arr = c.as_array().expect("带图时应为内容块数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "image_url");
        assert_eq!(arr[0]["image_url"]["url"], "data:image/png;base64,AAA");
        assert_eq!(arr[1]["type"], "text");
        assert!(arr[1]["text"].as_str().unwrap().contains("原始文本"));
    }

    #[test]
    fn extract_from_tool_call() {
        let body = json!({"choices":[{"message":{"tool_calls":[{"function":{"arguments":"{\"corrected_text\":\"谢克数学\"}"}}]}}]});
        assert_eq!(extract_text(&body).unwrap(), "谢克数学");
    }
    #[test]
    fn extract_fallback_content() {
        let body = json!({"choices":[{"message":{"content":"裸输出"}}]});
        assert_eq!(extract_text(&body).unwrap(), "裸输出");
    }
    #[test]
    fn extract_none_when_empty() {
        let body = json!({"choices":[{"message":{}}]});
        assert!(extract_text(&body).is_none());
    }
    #[test]
    fn extract_malformed_args_falls_back() {
        // arguments 不是合法 JSON → 降级 content
        let body = json!({"choices":[{"message":{"tool_calls":[{"function":{"arguments":"not-json"}}],"content":"降级"}}]});
        assert_eq!(extract_text(&body).unwrap(), "降级");
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn default_base_url_known_providers() {
        assert_eq!(default_base_url("deepseek"), "https://api.deepseek.com");
        assert_eq!(default_base_url("zhipu"), "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn extract_text_tool_call_beats_content() {
        let v: serde_json::Value = serde_json::json!({
            "choices": [{"message": {
                "content": "裸输出不应采用",
                "tool_calls": [{"function": {"arguments": "{\"corrected_text\":\"工具结果\"}"}}]
            }}]
        });
        assert_eq!(extract_text(&v).unwrap(), "工具结果");
    }

    #[test]
    fn extract_text_malformed_args_falls_back_to_content() {
        let v: serde_json::Value = serde_json::json!({
            "choices": [{"message": {
                "content": "降级内容",
                "tool_calls": [{"function": {"arguments": "not-json"}}]
            }}]
        });
        assert_eq!(extract_text(&v).unwrap(), "降级内容");
    }

    #[test]
    fn extract_text_empty_returns_none() {
        let v: serde_json::Value = serde_json::json!({"choices": [{"message": {"content": ""}}]});
        assert!(extract_text(&v).is_none());
    }
}
