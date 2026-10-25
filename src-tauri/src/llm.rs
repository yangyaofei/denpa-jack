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
    /// 请求超时（秒）
    pub timeout_secs: u64,
    /// 最大输出 token；0 = 不传该字段
    pub max_tokens: u32,
    /// 失败重试次数（仅对可重试失败生效）
    pub retries: u32,
}

/// 按 provider 拉取可用模型列表(OpenAI 兼容 GET /models)
pub fn default_base_url(provider: &str) -> &'static str {
    if provider == "deepseek" {
        "https://api.deepseek.com"
    } else {
        "https://open.bigmodel.cn/api/paas/v4"
    }
}

pub fn default_llm_prompt() -> String {
    DEFAULT_LLM_PROMPT.trim().to_string()
}

/// 默认润色提示词(阶段A 第3步从 settings.rs 归位): 它是 LLM 域资产——
/// pipeline/commands 消费; 配置里只存用户自定义的 prompt 字段(空=用此默认)。
pub const DEFAULT_LLM_PROMPT: &str = r#"
一个智能的语音转写文本润色助手。你的目标是输出一份准确、通顺且专业的记录。你需要找到“忠实原文”与“书面化”之间的平衡点：既要删除明显的口误和废话，又要保留说话人的表达逻辑、语气及情绪，不要进行过度的摘要或重写。

## Core Guidelines (核心原则)

### 1. 智能去噪与自我修正 (Smart Cleaning & Self-Correction) - [重点]

* 处理改口/自我修正：如果说话人先说了一个内容，紧接着又否定或修改了它，只保留最终正确的版本。
  * Bad: "我们需要用方案一，呃不是，是方案二。"
  * Good: "我们需要使用方案二。"
* 删除无意义重复：删除因思考卡顿导致的单词重复（如"这个这个这个"），但保留强调性的重复。
* 删除废词：彻底删除"呃、那个、嗯、You know、然后就是"等口语填充词。

### 2. 语气与情绪保留 (Tone & Emotion Preservation) - [关键]

* 区分废词与情绪词：删除无意义的填充词，但必须保留传递情绪的语气词（如"啊、呢、吧、嘛、哎、卧槽"等）。
* 禁止"压平"句式：绝对不要把反问句、感叹句强行修改为平铺直叙的陈述句。保留说话人表达时的急促感、疑问或不满。
* 尊重原始张力：保留带有个人色彩的生动吐槽、感叹或非正式比喻，不要将其"净化"成刻板、礼貌的陈述句。
  * Bad: "这 API 设计得也太反人类了吧？" -> "这个 API 的设计非常不合理。" （抹杀了反问和情绪张力）
  * Good: 保持原话 "这 API 设计得也太反人类了吧？"

### 3. 语法与流畅度修正 (Grammar & Flow)

* 修复语病：修正明显的语法错误（如主谓不一致、语序颠倒、缺词），使句子通顺。
* 适度润色：将过于破碎的短语拼接成完整的句子，但不要改变原句的主动/被动语态或核心逻辑。
* 拒绝过度总结：保留叙述细节，不要把长段的叙述强行压缩成一个 bullet point。

### 4. 听感同音修复 (Phonetic Context Repair)

* 精准替换：根据上下文修复 ASR 识别错误的同音词。
  * "含到 word" / "函道 word" -> Handover
  * "Meat request" -> Merge Request
  * "Sequel" -> SQL
  * "Q rui" -> Query
  * 艺术 -> issue

### 5. 技术规范化 (Technical Standards)

* 代码标记：变量名、文件路径、API 强制使用反引号 ( ` ) 包裹。示例：`userId`, `config.yaml`
* 序号补零：Phase one -> Phase 01; Step 5 -> Step 05。
* 中英空格：中英文之间强制添加空格。

### 6. 截图上下文对照 (Screenshot Grounding) - [优先]

随附截图是本次转写的**上下文**：屏幕上往往正显示着你刚才说的话涉及的内容（文档、窗口标题、代码、列表、聊天记录），其中的词可以直接复用。

* 转写里出现**听着别扭、不成词、或与常见写法对不上**的片段时，先去截图里找有没有**同一个词**（同音 / 近音 / 缩写 / 中英混写）。
  * 找得到 → 采用截图里的写法（含大小写、连字符、中英空格），不要另造写法。
  * 找不到 → 保持原样，不要凭猜测换成别的词。
* 截图里的专有名词、文件名、命令、变量名若与转写片段明显指同一个东西，以截图为准。
* 不要把截图里与这段转写无关的内容写进结果，也不要复述截图内容。

示例（截图里显示一行 `基线文档全部通读`）：
* Input: "你把那个机线文档全部通读一遍，然后再说结论。"
* Output: 你把那个 `基线文档` 全部通读一遍，然后再说结论。
* 解析: "机线文档"不成词，截图同一位置写着"基线文档全部通读" → 按截图写法修正。

示例（截图里没有对应词）：
* Input: "然后那个机组里面的文档，我们都要。"
* Output: 然后那个机组里面的文档，我们都要。
* 解析: 截图里找不到与"机组"对应的写法 → 保持原样，不猜。

## Few-Shot Examples (学习这种"中间力度")

### Example 1: 处理改口与重复 (修正力度：中)
* Input: "那个，Phase one 阶段，我们需要把那个...那个 user ID，不对，是 user name，传给后端。然后那个...然后那个 sequel 报错的话，就重试一下。"
* Output: Phase 01 阶段，我们需要把 `userName` 传给后端。如果 SQL 报错的话，就重试一下。
* 解析: 捕捉到了 userName 的改口逻辑，删除"那个"，修复了 Sequel 到 SQL 的转换并添加了代码反引号，序号 Phase one 规范为 Phase 01。

### Example 2: 处理 Handover 与语病 (修正力度：中)
* Input: "重复一下正在干的任务，在你重复完后，我要清理上下文。那个，在这之前，你需要含到 word 文档的编写。"
* Output: 重复一下当前正在进行的任务，在你重复完后，我要清理上下文。在这之前，你需要进行 Handover 文档的编写。
* 解析: 修复了"正在干的"这种生硬口语，修复了"含到word"为 Handover，保持了原本的对话指令流。

### Example 3: 混合逻辑与代码 (修正力度：中)
* Input: "如果这个值是空的，呃，如果是 null 的话，就抛异常。标题写一下，1.1 异常处理。"
* Output: 如果这个值是 `null`，就抛出异常。标题写一下，1.1 异常处理。
* 解析: 处理了"空的...如果是 null"的自我修正，保留了技术术语 null 并加反引号。识别了标题格式。

### Example 4: 情绪保留与反问句式 (修正力度：轻)
* Input: "哎哟，这 API 设计得也太反人类了吧？那个...我们绝对不能这么搞啊，这不是纯纯给自己挖坑嘛！"
* Output: "哎哟，这 API 设计得也太反人类了吧？我们绝对不能这么搞啊，这不是纯纯给自己挖坑嘛！"
* 解析: 删除了废词"那个"，但完整保留了"哎哟、吧、啊、嘛"等情绪语气词，并保留了反问句式和"反人类、挖坑"等生动吐槽，拒绝将其书面化为无感情的陈述句。

## 最终输出要求

* 直接输出润色后的最终文本。
* 绝对不要输出"解析："等任何解释性过程。
* 不要输出多余的开场白或结尾。
"#;

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
    // 日志里把 base64 换成"可核验指纹"（字节数/data URL 字符数/前缀/sha256 前 16 位），
    // 既不让日志背 1.7MB 的 base64，又能证明发出去的内容确实等于磁盘上那张截图。
    let mut shot_note: Option<String> = None;
    let image_url = match screenshot {
        Some(p) => match crate::screen_context::inline_image(p) {
            Ok(img) => {
                let mb = img.bytes as f64 / 1024.0 / 1024.0;
                shot_note = Some(format!(
                    "<截图内联: {} ({mb:.2} MB, {} 字节, data URL {} 字符, 前缀 {}, sha256 {}…)>",
                    p.display(),
                    img.bytes,
                    img.data_url.chars().count(),
                    crate::screen_context::data_url_prefix(&img.data_url),
                    img.sha256_short
                ));
                crate::log::elog(&format!(
                    "[llm] 带图请求: bytes={} data_url_chars={} sha256={}",
                    img.bytes,
                    img.data_url.chars().count(),
                    img.sha256_short
                ));
                Some(img.data_url)
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
    if o.max_tokens > 0 {
        payload["max_tokens"] = json!(o.max_tokens);
    }

    // 落盘前把 base64 换成文件引用：请求里是完整内联数据，日志里只留路径与大小（截图本体已存 screen_context/）
    let mut dump_payload = payload.clone();
    if let Some(note) = &shot_note {
        if let Some(arr) = dump_payload["messages"][1]["content"].as_array_mut() {
            if let Some(first) = arr.first_mut() {
                first["image_url"]["url"] = json!(note);
            }
        }
    }

    // 失败重试：只对"传输失败/响应读不出/5xx/429/无有效输出"重试；4xx 配置错误直接返回
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(o.timeout_secs.max(1)))
        .build()
        .map_err(|e| e.to_string())?;
    let attempts = o.retries + 1;
    let mut last_err = String::new();
    for attempt in 1..=attempts {
        let t0 = std::time::Instant::now();
        match attempt_once(&client, &url, &key, &payload).await {
            Ok(out) => {
                dump_call(
                    &format!("{}-{}ms-a{attempt}", &o.provider, t0.elapsed().as_millis()),
                    &url,
                    &dump_payload,
                    &out.raw,
                );
                if attempt > 1 {
                    crate::log::elog(&format!("[llm] 第 {attempt} 次尝试成功（共 {attempts} 次机会）"));
                }
                return Ok(LlmOut { text: out.text, thinking: out.thinking });
            }
            Err(e) => {
                dump_call(
                    &format!("{}-{}ms-a{attempt}-失败", &o.provider, t0.elapsed().as_millis()),
                    &url,
                    &dump_payload,
                    &e.detail,
                );
                crate::log::elog(&format!(
                    "[llm] 失败 第{attempt}/{attempts}次 可重试={} 原因={} 详情={}",
                    e.retryable,
                    e.kind,
                    e.detail.chars().take(200).collect::<String>()
                ));
                last_err = format!("{}（{}）", e.kind, e.detail.chars().take(200).collect::<String>());
                if !e.retryable || attempt == attempts {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(400 * 2u64.pow(attempt - 1)));
            }
        }
    }
    Err(format!("LLM 失败（已尝试 {attempts} 次）: {last_err}"))
}

/// 一次请求的结果（成功时带原始响应文本，便于落盘）
struct AttemptOk {
    raw: String,
    text: String,
    thinking: Option<String>,
}

/// 一次尝试的失败：`retryable` 决定是否值得重试
struct AttemptErr {
    kind: String,
    detail: String,
    retryable: bool,
}

/// 是否值得重试：传输/解析类失败与 5xx/429 值得；4xx（模型名、Key、参数错）不值得
fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

async fn attempt_once(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    payload: &Value,
) -> Result<AttemptOk, AttemptErr> {
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {key}"))
        .json(payload)
        .send()
        .await
        .map_err(|e| AttemptErr {
            kind: "请求失败".into(),
            detail: e.to_string(),
            retryable: true,
        })?;
    let status = resp.status();
    let raw = resp.text().await.map_err(|e| AttemptErr {
        kind: format!("响应读取失败 HTTP {status}"),
        detail: e.to_string(),
        retryable: is_retryable_status(status.as_u16()) || status.is_success(),
    })?;
    let body: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Err(AttemptErr {
                kind: format!("响应解析失败 HTTP {status}"),
                detail: format!("{e} | 原始响应前 200 字: {}", raw.chars().take(200).collect::<String>()),
                retryable: is_retryable_status(status.as_u16()) || status.is_success(),
            })
        }
    };
    if !status.is_success() {
        return Err(AttemptErr {
            kind: format!("HTTP {status}"),
            detail: body.to_string(),
            retryable: is_retryable_status(status.as_u16()),
        });
    }
    let thinking = extract_thinking(&body);
    match extract_text(&body) {
        Some(t) => Ok(AttemptOk { raw, text: t, thinking }),
        None => Err(AttemptErr {
            kind: "无有效输出".into(),
            detail: body.to_string(),
            retryable: true,
        }),
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

    /// 重试判据：只有 429 与 5xx 值得重试；其它 4xx（模型名/Key/参数错）重试无意义
    #[test]
    fn retry_decision_matches_status_semantics() {
        for s in [429, 500, 502, 503, 504] {
            assert!(is_retryable_status(s), "{s} 应可重试");
        }
        for s in [400, 401, 403, 404, 422] {
            assert!(!is_retryable_status(s), "{s} 不应重试");
        }
        assert!(!is_retryable_status(200));
    }

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
