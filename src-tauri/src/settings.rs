// 配置层: 档案制(参照 Swift 版 Config.swift 结构)
// 原则: 所有新增字段必须 #[serde(default)] —— 旧配置文件可解析(C17 教训)
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmProfile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub provider: String, // deepseek | zhipu | openai-compatible
    #[serde(default)]
    pub base_url: String, // 自定义网关; 空=按 provider 默认
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub thinking: bool, // 旧字段, 仅兼容旧配置读取
    /// 思考强度: low / high (显式参数 reasoning_effort)
    #[serde(default = "default_effort")]
    pub effort: String,
}

fn default_effort() -> String {
    "low".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AsrProfile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub provider: String, // volcengine | zhipu
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub hotwords_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DictEntry {
    pub term: String,            // 标准写法
    #[serde(default)]
    pub variants: Vec<String>,   // 显式变体
    #[serde(default)]
    pub guard_words: Vec<String>,// 护栏词
    #[serde(default)]
    pub boost: u8,               // 热词权重 1-10
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegexRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_hotkey_key")]
    pub key: String, // global-shortcut 格式: f5 / a / 1 / space / up ...
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub cmd: bool,
    #[serde(default)]
    pub shift: bool,
}
impl Default for HotkeyConfig {
    fn default() -> Self {
        Self { key: "f5".into(), ctrl: false, alt: false, cmd: false, shift: false }
    }
}
impl HotkeyConfig {
    pub fn shortcut_str(&self) -> String {
        let mut parts = vec![];
        if self.ctrl { parts.push("ctrl".into()); }
        if self.alt { parts.push("alt".into()); }
        if self.shift { parts.push("shift".into()); }
        if self.cmd { parts.push("cmd".into()); }
        parts.push(normalize_key(if self.key.is_empty() { "f5" } else { &self.key }));
        parts.join("+")
    }
}

/// 键名归一化为 keyboard_types Code 格式(global-hotkey 只认 KeyA/Space/F5/Digit1/Comma...)
pub fn normalize_key(k: &str) -> String {
    let k = k.trim();
    if k.is_empty() {
        return "F5".into();
    }
    // 已是 Code 格式
    if k.starts_with("Key") || k.starts_with("Digit") || k.starts_with("Arrow") || k.starts_with("Numpad") {
        return k.into();
    }
    {
        let upper = k.to_ascii_uppercase();
        if upper.starts_with('F') && upper.len() >= 2 && upper[1..].chars().all(|c| c.is_ascii_digit()) {
            return upper;
        }
    }
    match k.to_ascii_lowercase().as_str() {
        "space" => "Space".into(),
        "return" | "enter" => "Enter".into(),
        "tab" => "Tab".into(),
        "backspace" | "delete" => "Backspace".into(),
        "up" => "ArrowUp".into(),
        "down" => "ArrowDown".into(),
        "left" => "ArrowLeft".into(),
        "right" => "ArrowRight".into(),
        "esc" | "escape" => "Escape".into(),
        "," | "comma" => "Comma".into(),
        "." | "period" => "Period".into(),
        "/" | "slash" => "Slash".into(),
        ";" | "semicolon" => "Semicolon".into(),
        "'" | "quote" => "Quote".into(),
        "-" | "minus" => "Minus".into(),
        "=" | "equal" => "Equal".into(),
        "[" | "bracketleft" => "BracketLeft".into(),
        "]" | "bracketright" => "BracketRight".into(),
        "\\" | "backslash" => "Backslash".into(),
        _ => {
            // 单字母/单数字 → KeyX/DigitX
            let lower = k.to_ascii_lowercase();
            if lower.len() == 1 {
                let c = lower.chars().next().unwrap();
                if c.is_ascii_alphabetic() {
                    return format!("Key{}", c.to_ascii_uppercase());
                }
                if c.is_ascii_digit() {
                    return format!("Digit{c}");
                }
            }
            k.into()
        }
    }
}
fn default_hotkey_key() -> String { "f5".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub keys: Vec<String>, // API Key 池(档案间共享)
    #[serde(default)]
    pub llm_profiles: Vec<LlmProfile>,
    #[serde(default)]
    pub asr_profiles: Vec<AsrProfile>,
    #[serde(default)]
    pub active_llm_id: String,
    #[serde(default)]
    pub active_asr_id: String,
    #[serde(default)]
    pub dict: Vec<DictEntry>,
    #[serde(default)]
    pub normalizations: Vec<RegexRule>,
    #[serde(default)]
    pub use_llm_correction: bool,
    #[serde(default)]
    pub clipboard_only: bool,
    #[serde(default)]
    pub max_recording_seconds: u32, // 1800
    #[serde(default)]
    pub min_recording_seconds: f64, // 0.3
    #[serde(default)]
    pub keep_audio_count: u32,
    #[serde(default)]
    pub mic_device_uid: String, // 指定输入设备(设备名, 空=自动)
    #[serde(default)]
    pub mic_priority: Vec<String>, // 优先级(设备名序列, 按序取第一个在线)
    #[serde(default)]
    pub hotkey: HotkeyConfig,
    #[serde(default)]
    pub audio_feedback: bool,
    #[serde(default = "default_true")]
    pub restore_clipboard: bool,
    /// 松开后尾音缓冲毫秒(蓝牙麦词尾截断时调大; 0=立即停)
    #[serde(default)]
    pub extra_tail_ms: u32,
    #[serde(default)]
    pub auto_submit: bool,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,
    #[serde(default = "default_history_limit")]
    pub history_limit: u64,

}

pub fn config_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .expect("app_config_dir 不可用");
    fs::create_dir_all(&dir).ok();
    dir.join("config.json")
}


#[tauri::command]
pub fn get_config(app: tauri::AppHandle) -> Result<Config, String> {
    let p = config_path(&app);
    if !p.exists() {
        return Ok(default_config());
    }
    let raw = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Config>(&raw) {
        Ok(c) => Ok(c),
        Err(e) => {
            // 按字段救活(Handy salvage): 一个坏字段不能毁掉整个配置
            let val: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => return Err(format!("config 解析失败: {e}")),
            };
            let mut def = default_config();
            if let serde_json::Value::Object(map) = val {
                let mut merged = serde_json::to_value(&def).unwrap_or_default();
                if let serde_json::Value::Object(m) = &mut merged {
                    for (k, v) in map {
                        if let Some(slot) = m.get_mut(&k) {
                            // 同类型才覆盖
                            let same_type = matches!(
                                (&*slot, &v),
                                (serde_json::Value::Bool(_), serde_json::Value::Bool(_))
                                    | (serde_json::Value::Number(_), serde_json::Value::Number(_))
                                    | (serde_json::Value::String(_), serde_json::Value::String(_))
                                    | (serde_json::Value::Array(_), serde_json::Value::Array(_))
                                    | (serde_json::Value::Object(_), serde_json::Value::Object(_))
                            );
                            if same_type {
                                *slot = v;
                            }
                        }
                    }
                }
                if let Ok(revived) = serde_json::from_value::<Config>(merged) {
                    def = revived;
                }
            }
            Ok(def)
        }
    }
}

#[tauri::command]
pub fn save_config(app: tauri::AppHandle, mut config: Config) -> Result<(), String> {
    config.hotkey.key = normalize_key(&config.hotkey.key);
    let p = config_path(&app);
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&p, json).map_err(|e| e.to_string())
}

fn default_config() -> Config {
    Config {
        use_llm_correction: true,
        clipboard_only: false,
        max_recording_seconds: 1800,
        min_recording_seconds: 0.3,
        keep_audio_count: 50,
        hotkey: HotkeyConfig { key: "f5".into(), ..Default::default() },
        ..Default::default()
    }
}

pub fn default_llm_prompt() -> String {
    DEFAULT_LLM_PROMPT.trim().to_string()
}

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

fn default_shortcut_activation() -> String {
    "hold_or_toggle".into()
}

fn default_hold_threshold_ms() -> u64 {
    300
}

fn default_true() -> bool {
    true
}

fn default_overlay_position() -> String {
    "bottom".into()
}

fn default_history_limit() -> u64 {
    200
}
