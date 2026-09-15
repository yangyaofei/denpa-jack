// 配置层: 档案制(参照 Swift 版 Config.swift 结构)
// 原则: 所有新增字段必须 #[serde(default)] —— 旧配置文件可解析(C17 教训)
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

/// bundle id 单一来源(tauri.conf.json 的 identifier 必须与此一致)
/// 数据目录 ~/Library/Application Support/<APP_ID>/, 同时用于自粘贴焦点判定;
/// 改名时必须同步 tauri.conf.json + 迁移旧目录, 否则日志/llm_logs/历史会分叉
pub const APP_ID: &str = "io.github.yangyaofei.denpajack";

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
/// C54 绑定集: 一个动作(如 transcribe)的全部生效键组合
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct BindingSet {
    pub current: Vec<String>,
}

impl HotkeyConfig {
    pub fn shortcut_str(&self) -> String {
        let mut parts = vec![];
        if self.ctrl { parts.push("ctrl".into()); }
        if self.alt { parts.push("alt".into()); }
        if self.shift { parts.push("shift".into()); }
        if self.cmd { parts.push("cmd".into()); }
        // C51b 纯修饰组合: key 为空时不回填 F5(否则 flags-tap 无法识别+误注册 F5)
        if !self.key.trim().is_empty() {
            parts.push(normalize_key(&self.key));
        }
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
    #[serde(default = "d_max_rec")]
    pub max_recording_seconds: u32, // 1800
    #[serde(default = "d_min_rec")]
    pub min_recording_seconds: f64, // 0.3
    #[serde(default = "d_keep_audio")]
    pub keep_audio_count: u32,
    #[serde(default)]
    pub mic_device_uid: String, // 指定输入设备(设备名, 空=自动)
    #[serde(default)]
    pub mic_priority: Vec<String>, // 优先级(设备名序列, 按序取第一个在线)
    /// 旧字段(只读迁移: get_config 时并入 bindings, 保存时不再写出)
    #[serde(default, skip_serializing)]
    pub hotkey: HotkeyConfig,
    /// 旧多热键字段(只读迁移)
    #[serde(default, skip_serializing)]
    pub hotkeys: Vec<HotkeyConfig>,
    /// C54 唯一真相: 动作→键组合列表("transcribe"=录音)。
    /// 旧 hotkey/hotkeys 在 get_config 迁移时并入, 之后此字段为准。
    #[serde(default)]
    pub bindings: std::collections::HashMap<String, BindingSet>,
    /// 触发模式: "hold"=按住说话(默认) / "toggle"=短按开始再短按结束
    #[serde(default = "default_shortcut_activation")]
    pub activation: String,
    #[serde(default)]
    pub audio_feedback: bool,
    #[serde(default = "default_true")]
    pub restore_clipboard: bool,
    /// C50: 粘贴后把转写文本保留在剪贴板(与 restore_clipboard 互斥, 保存时反向同步)
    #[serde(default)]
    pub keep_in_clipboard: bool,
    /// 松开后尾音缓冲毫秒(蓝牙麦词尾截断时调大; 0=立即停)
    #[serde(default)]
    pub extra_tail_ms: u32,
    #[serde(default)]
    pub auto_submit: bool,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,
    #[serde(default = "default_history_limit")]
    pub history_limit: u64,
    /// 数据目录：history / recordings / llm_logs / app.log 的根目录。
    /// 空 = 默认目录（`~/Library/Application Support/io.github.yangyaofei.denpajack`）。
    /// 支持绝对路径、以 `~/` 开头、或相对默认目录的相对路径。
    #[serde(default)]
    pub data_dir: String,

}

/// 默认数据目录（也是 config.json 的固定位置，作为一切路径的锚点）
pub fn base_dir(app: &tauri::AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .expect("app_config_dir 不可用");
    fs::create_dir_all(&dir).ok();
    dir
}

/// 解析数据目录：配置为空用默认目录；`~/x` 展开为家目录；相对路径相对默认目录
pub fn resolve_data_dir(base: &std::path::Path, configured: &str) -> PathBuf {
    let t = configured.trim();
    if t.is_empty() {
        return base.to_path_buf();
    }
    let expanded = match t.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => t.to_string(),
        },
        None => t.to_string(),
    };
    let p = PathBuf::from(expanded);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

static DATA_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// 配置变更后必须清缓存（否则数据仍写到旧目录）
pub fn invalidate_data_dir_cache() {
    *DATA_ROOT.lock().unwrap() = None;
}

/// 数据根目录（带缓存；save_config 时失效）。history/recordings/llm_logs/app.log 都从这里派生。
pub fn data_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Some(p) = DATA_ROOT.lock().unwrap().clone() {
        return p;
    }
    let base = base_dir(app);
    let configured = get_config(app.clone())
        .map(|c| c.data_dir)
        .unwrap_or_default();
    let p = resolve_data_dir(&base, &configured);
    fs::create_dir_all(&p).ok();
    *DATA_ROOT.lock().unwrap() = Some(p.clone());
    p
}

/// 没有 app handle 的地方（如 LLM 落盘）取同一份目录：缓存已由启动流程预热；
/// 未预热时按家目录推导默认目录。
pub fn data_dir_no_app() -> PathBuf {
    if let Some(p) = DATA_ROOT.lock().unwrap().clone() {
        return p;
    }
    let base = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support").join(APP_ID))
        .unwrap_or_else(|_| PathBuf::from("."));
    fs::create_dir_all(&base).ok();
    // 没有 AppHandle 时直接从磁盘读 config.json 取 data_dir，保证日志与业务写在同一目录
    let configured = fs::read_to_string(base.join("config.json"))
        .ok()
        .map(|s| configured_data_dir_from_json(&s))
        .unwrap_or_default();
    let p = resolve_data_dir(&base, &configured);
    fs::create_dir_all(&p).ok();
    *DATA_ROOT.lock().unwrap() = Some(p.clone());
    p
}

/// 从 config.json 文本里取 data_dir（无 AppHandle 场景用；缺失/解析失败一律视为空=默认目录）
fn configured_data_dir_from_json(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("data_dir").and_then(|d| d.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

pub fn config_path(app: &tauri::AppHandle) -> PathBuf {
    base_dir(app).join("config.json")
}


/// 配置变更信号(save_config 时递增; 主窗轮询刷新顶栏状态)
pub static CONFIG_VER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn config_version() -> u64 {
    CONFIG_VER.load(std::sync::atomic::Ordering::SeqCst)
}

#[tauri::command]

pub fn get_config(app: tauri::AppHandle) -> Result<Config, String> {
    let p = config_path(&app);
    if !p.exists() {
        return Ok(default_config());
    }
    let raw = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut cfg = match serde_json::from_str::<Config>(&raw) {
        Ok(c) => c,
        Err(e) => {
            // 按字段恢复(Handy salvage): 一个坏字段不能毁掉整个配置
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
            def
        }
    };
    // C54 迁移: hotkey/hotkeys 并入 bindings("transcribe")——一次迁移, bindings 为唯一真相
    let needs_migrate = cfg.bindings.is_empty() && (!cfg.hotkeys.is_empty() || cfg.hotkey.shortcut_str() != "F5");
    if needs_migrate {
        let mut set: Vec<String> = vec![cfg.hotkey.shortcut_str()];
        for h in &cfg.hotkeys {
            let s = h.shortcut_str();
            if !s.is_empty() {
                set.push(s);
            }
        }
        let set: Vec<String> = {
            let mut normed: Vec<String> = set.iter().map(|s| normalize_combo(s)).filter(|s| !s.is_empty()).collect();
            normed.sort();
            normed.dedup();
            normed
        };
        cfg.bindings.insert("transcribe".into(), BindingSet { current: set });
        // 迁移即落盘(旧字段 skip_serializing 不再写出, 重复组合已归一去重)
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = fs::write(&p, json);
        }
    }
    Ok(cfg)
}

impl Config {
    /// transcribe 动作的全部生效键组合(字符串, handy-keys Hotkey::from_str 可解析)
    pub fn transcribe_bindings(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .bindings
            .get("transcribe")
            .map(|b| b.current.clone())
            .unwrap_or_default()
            .iter()
            .map(|s| normalize_combo(s))
            .filter(|s| !s.is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// C55 组合归一化: 小写 + 修饰键固定序(ctrl/option/shift/cmd/fn) + 主键最后 + 去重
/// ("Cmd+Option+Ctrl" 与 "ctrl+option+cmd" 归一为同一串, 防止重复注册)
pub fn normalize_combo(s: &str) -> String {
    const MOD_ORDER: [&str; 5] = ["ctrl", "option", "shift", "cmd", "fn"];
    let mut mods = vec![];
    let mut keys = vec![];
    for p in s.split('+') {
        let p = p.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        let p = if p == "alt" { "option".to_string() } else { p };
        if MOD_ORDER.contains(&p.as_str()) {
            if !mods.contains(&p) {
                mods.push(p);
            }
        } else if !keys.contains(&p) {
            keys.push(p);
        }
    }
    mods.sort_by_key(|m| MOD_ORDER.iter().position(|x| x == m).unwrap());
    mods.extend(keys);
    mods.join("+")
}

#[tauri::command]
pub fn save_config(app: tauri::AppHandle, mut config: Config) -> Result<(), String> {
    // C50 互斥: 保留剪贴板 与 恢复原剪贴板 不同时成立
    if config.keep_in_clipboard {
        config.restore_clipboard = false;
    }
    // C55: 绑定列表归一去重(防顺序变体重复注册)
    if let Some(set) = config.bindings.get_mut("transcribe") {
        let mut normed: Vec<String> = set.current.iter().map(|s| normalize_combo(s)).filter(|s| !s.is_empty()).collect();
        normed.sort();
        normed.dedup();
        set.current = normed;
    }
    // 触发模式缓存同步(引擎回调读全局, 无 app handle)
    *crate::ACTIVATION.lock().unwrap() = config.activation.clone();

    CONFIG_VER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    config.hotkey.key = normalize_key(&config.hotkey.key); // 兼容读(旧字段仅迁移用)
    // 配置变更可能改麦克风选择(优先级/指定设备): 必须清解析缓存, 否则仍用旧设备
    // (此前 MIC_CACHE 只在 open 失败时失效 → 改优先级后实际仍录旧麦, 用户报“设了 USB 优先却还用内置麦”)
    crate::recording::invalidate_mic_cache();
    // 配置变更可能改数据目录: 同样必须清缓存, 否则新数据仍写到旧目录
    invalidate_data_dir_cache();
    let p = config_path(&app);
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&p, json).map_err(|e| e.to_string())
}

pub fn d_max_rec() -> u32 { 1800 }
fn d_min_rec() -> f64 { 0.3 }
fn d_keep_audio() -> u32 { 50 }
pub fn default_config() -> Config {
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
    "hold".into()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_combo_orders_and_dedups() {
        assert_eq!(normalize_combo("Cmd+Option+Ctrl"), "ctrl+option+cmd");
        assert_eq!(normalize_combo("ctrl+option+cmd"), "ctrl+option+cmd");
        assert_eq!(normalize_combo("ALT+Space"), "option+space");
        assert_eq!(normalize_combo("fn+ctrl"), "ctrl+fn");
        assert_eq!(normalize_combo("ctrl+ctrl+a"), "ctrl+a");
        assert_eq!(normalize_combo("F5"), "f5");
        assert_eq!(normalize_combo("Ctrl+Cmd+Space"), "ctrl+cmd+space");
    }

    #[test]
    fn normalize_key_basics() {
        assert_eq!(normalize_key("f5"), "F5");
        assert_eq!(normalize_key(""), "F5");
        assert_eq!(normalize_key("space"), "Space");
        assert_eq!(normalize_key("esc"), "Escape");
        assert_eq!(normalize_key("KeyA"), "KeyA");
        assert_eq!(normalize_key("a"), "KeyA");
        assert_eq!(normalize_key("1"), "Digit1");
        assert_eq!(normalize_key("/"), "Slash");
    }
    #[test]
    fn shortcut_str_order_and_modifiers() {
        let hk = HotkeyConfig { key: "f5".into(), ctrl: true, alt: true, ..Default::default() };
        assert_eq!(hk.shortcut_str(), "ctrl+alt+F5");
        // C51b: 空 key=纯修饰组合(无键), 不回填 F5
        let hk2 = HotkeyConfig { key: "".into(), ..Default::default() };
        assert_eq!(hk2.shortcut_str(), "");
    }
    #[test]
    fn c17_deserialize_partial_config_no_wipe() {
        // C17 回归: 缺字段不能把已有默认值清空/panic——全 serde(default)
        let cfg: Result<Config, _> = serde_json::from_str("{}");
        assert!(cfg.is_ok());
        let c = cfg.unwrap();
        assert!(!c.hotkey.key.is_empty());
        assert!(c.min_recording_seconds > 0.0);
    }
    #[test]
    fn config_roundtrip_preserves_profiles() {
        let mut c = default_config();
        c.keys = vec!["k1".into()];
        c.llm_profiles = vec![LlmProfile {
            id: "l1".into(), name: "n".into(), provider: "deepseek".into(), base_url: "".into(),
            model: "m".into(), api_key: "k".into(), prompt: "p".into(), thinking: false, effort: "high".into(),
        }];
        let s = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.llm_profiles[0].model, "m");
        assert_eq!(back.keys[0], "k1");
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn resolve_data_dir_empty_uses_base() {
        let base = std::path::Path::new("/tmp/base");
        assert_eq!(resolve_data_dir(base, ""), base.to_path_buf());
        assert_eq!(resolve_data_dir(base, "   "), base.to_path_buf());
    }

    #[test]
    fn resolve_data_dir_absolute_and_relative() {
        let base = std::path::Path::new("/tmp/base");
        assert_eq!(
            resolve_data_dir(base, "/Volumes/Ext/denpa"),
            std::path::PathBuf::from("/Volumes/Ext/denpa")
        );
        assert_eq!(
            resolve_data_dir(base, "data2"),
            std::path::PathBuf::from("/tmp/base/data2")
        );
    }

    #[test]
    fn resolve_data_dir_expands_home() {
        let base = std::path::Path::new("/tmp/base");
        let home = std::env::var("HOME").expect("HOME 应存在");
        assert_eq!(
            resolve_data_dir(base, "~/DenpaData"),
            std::path::PathBuf::from(format!("{home}/DenpaData"))
        );
    }

    #[test]
    fn config_without_data_dir_defaults_to_empty() {
        let cfg: Config = serde_json::from_str("{}").expect("空对象应可解析");
        assert_eq!(cfg.data_dir, "");
    }

    #[test]
    fn normalize_key_whitespace_and_case() {
        assert_eq!(normalize_key("  f5  "), "F5");
        assert_eq!(normalize_key("SPACE"), "Space");
        assert_eq!(normalize_key("Enter"), "Enter");
        assert_eq!(normalize_key("KeyA"), "KeyA");
        assert_eq!(normalize_key("Digit7"), "Digit7");
    }

    #[test]
    fn normalize_key_fallback_f5() {
        assert_eq!(normalize_key(""), "F5");
        assert_eq!(normalize_key("   "), "F5");
        // 非空未知键保持原样(不悄悄重置用户配置)
        assert_eq!(normalize_key("unknown!!"), "unknown!!");
    }

    #[test]
    fn shortcut_str_modifier_order_stable() {
        let h = HotkeyConfig { key: "KeyJ".into(), ctrl: true, alt: true, cmd: false, shift: false };
        assert_eq!(h.shortcut_str(), "ctrl+alt+KeyJ");
        let h2 = HotkeyConfig { key: "F9".into(), ctrl: false, alt: false, cmd: true, shift: true };
        assert_eq!(h2.shortcut_str(), "shift+cmd+F9");
    }

    #[test]
    fn config_json_missing_optional_fields_never_zero() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.min_recording_seconds, 0.3);
        assert_eq!(c.max_recording_seconds, 1800);
        assert_eq!(c.keep_audio_count, 50);
        assert_eq!(c.history_limit, 200);
    }

    #[test]
    fn config_legacy_key_mapping_salvage() {
        let c: Config = serde_json::from_str(r#"{"keys": ["k1"], "hotkey_key_code": 96}"#).unwrap();
        assert_eq!(c.keys, vec!["k1".to_string()]);
        assert!(c.llm_profiles.is_empty());
    }
}
