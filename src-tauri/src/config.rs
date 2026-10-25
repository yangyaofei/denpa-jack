// 配置层: 档案制(参照 Swift 版 Config.swift 结构)
// 原则: 所有新增字段必须 #[serde(default)] —— 旧配置文件可解析(C17 教训)
// 阶段A 第3步拆分: 路径解析→config/paths.rs, 一次性迁移→config/migrate.rs;
// 本文件只剩"配置里有什么"(类型/默认值/读写/热键归一化)。
use serde::{Deserialize, Serialize};
use std::fs;

mod migrate;
pub mod paths;

pub use paths::{
    base_dir, config_path, data_dir, data_dir_no_app,
    resolve_data_dir, APP_ID,
};
pub(crate) use paths::write_config_atomic;

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
    pub provider: String, // volcengine | zhipu | local
    #[serde(default)]
    pub api_key: String,
    /// 本地服务地址（provider=local 时用；空=ws://127.0.0.1:8300）
    #[serde(default)]
    pub base_url: String,
}

/// 内置本地引擎的启动参数（2026-10-08 用户定则: 配置长在引擎身上, APP 是唯一
/// 事实源, 拉起子进程时按此生成 DENPA_ASR_* 环境变量; 改动重启网关后生效）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinEngineConfig {
    /// 模型: repo id(自动下载到项目 models/)或本地路径
    pub model: String,
    /// VAD: ""=关(默认, 2026-10-08 实测 ten 吃气声口述首尾帧, 丢字>省算力) | silero | ten
    pub vad: String,
    /// 松手后全句重解(豆包两段式)
    pub twopass: bool,
    /// 流式分块粒度(秒)
    pub chunk_size_sec: f64,
    /// 解码窗上限(秒)
    pub max_context_sec: f64,
    /// 端点检测: fixed | energy
    pub endpointing: String,
    /// 网关并发会话上限
    pub max_sessions: u32,
}
impl Default for BuiltinEngineConfig {
    fn default() -> Self {
        Self {
            model: "Qwen/Qwen3-ASR-1.7B".into(),
            vad: String::new(),
            twopass: true,
            chunk_size_sec: 0.5,
            max_context_sec: 120.0,
            endpointing: "energy".into(),
            max_sessions: 4,
        }
    }
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
    /// 活动目标探测的用途（决策在消费端）：off / active_window（默认）/ active_display。
    /// - active_window：截"主窗口"（键盘焦点窗口；没有则指针下的窗口）；都没有则回退截屏
    /// - active_display：截"光标所在显示器"；截图时排除本应用自己的窗口（浮窗不入镜）
    /// 需要屏幕录制权限；截图存 `<数据目录>/screen_context/`。
    /// 旧键 `screenshot_target` / `screenshot_context` 已废弃（不迁移，见 docs/SPEC.md §3.5.1）。
    #[serde(default = "d_detect_active_target")]
    pub detect_active_target: String,
    /// LLM 请求超时（秒）。越大越能容忍慢思考模型，但出错时用户等待也更久。
    #[serde(default = "d_llm_timeout")]
    pub llm_timeout_secs: u64,
    /// LLM 最大输出 token；0 = 不传该字段（用服务端默认）。
    /// 思考型模型可能把预算耗在思考上导致正文为空，需要时在这里调大。
    #[serde(default)]
    pub llm_max_tokens: u32,
    /// LLM 失败重试次数（0 = 不重试）。
    /// 只有"传输失败 / 响应读不出 / HTTP 5xx 或 429 / 响应无有效输出"才重试；
    /// 4xx（模型名、Key、参数错误）属配置问题，重试无意义，直接返回错误。
    #[serde(default = "d_llm_retries")]
    pub llm_retries: u32,
    /// 启动时自动检查更新（只报告新版本，不自动安装；关于页可手动检查与安装）
    #[serde(default = "default_true")]
    pub auto_check_update: bool,
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
    /// 交付时在文本末尾追加的换行数（0=不追加；1=换行一次；2=空一行…；上限 10）。
    /// 只改交付出去的文本，不改历史里的识别结果。
    #[serde(default)]
    pub trailing_newlines: u32,
    /// 粘贴优先名单（bundle id，每行一个）：命中时跳过 AX 直写、直接 Cmd+V 粘贴。
    /// 手动配置项（C 方案），用于 AX 写入会重复输入的 App——如 com.google.GeminiMacOS
    /// 实测 AX 写入生效但回读信号为空，旧校验误判后会再粘贴一次（双重插入，2026-10-14）。
    #[serde(default)]
    pub ax_paste_only_apps: Vec<String>,
    /// local 引擎网关自动拉起(APP 启动时探活, 不在则起子进程; 退出时只收自己的)
    #[serde(default = "default_true")]
    pub local_autostart: bool,
    /// denpa-asr 项目目录(内置引擎拉子进程用; 外部网关档案不受此管)
    #[serde(default = "default_local_service_dir")]
    pub local_service_dir: String,
    /// 内置本地引擎启动参数(模型/VAD/二遍重解/分块/窗口/端点/并发; 重启网关生效)
    #[serde(default)]
    pub builtin_engine: BuiltinEngineConfig,
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

/// 配置版本(转发 core: 计数已收口, 写路径走 core::bump_config)
pub fn config_version() -> u64 {
    crate::core::config_version()
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
    // 一次性迁移(C54 bindings / F5 兜底 / 内置引擎 / VAD 归零)集中在 config/migrate.rs
    migrate::migrate_on_load(&mut cfg, &p);
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

    crate::core::bump_config();
    config.hotkey.key = normalize_key(&config.hotkey.key); // 兼容读(旧字段仅迁移用)
    // 配置变更可能改麦克风选择(优先级/指定设备): 必须清解析缓存, 否则仍用旧设备
    // (此前 MIC_CACHE 只在 open 失败时失效 → 改优先级后实际仍录旧麦, 用户报“设了 USB 优先却还用内置麦”)
    crate::audio::invalidate_mic_cache();
    // D10: 数据目录**不**在此失效——中途切换会让 app.log(已打开的句柄)/history.jsonl
    // 写到一半分叉到新目录, 两边各半。data_dir 缓存只在启动解析一次, 改配置后
    // 由前端提示"重启生效"(common.ts saveCfg 比对 data_dir 变化)。
    let p = config_path(&app);
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    write_config_atomic(&p, &json).map_err(|e| e.to_string())
}

pub fn d_max_rec() -> u32 { 1800 }
fn d_min_rec() -> f64 { 0.3 }
fn d_keep_audio() -> u32 { 50 }
fn d_llm_timeout() -> u64 { 30 }
fn d_llm_retries() -> u32 { 2 }
/// 活动目标探测用途的默认值：截当前窗口（"旧键忽略、新键缺省 active_window"）
fn d_detect_active_target() -> String { "active_window".into() }
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

// 默认润色提示词(DEFAULT_LLM_PROMPT)已归位 llm.rs(阶段A 第3步): 那是 LLM 域的资产,
// pipeline/commands 消费; 配置域只存用户自定义的 prompt 字段。

fn default_shortcut_activation() -> String {
    "hold".into()
}

fn default_true() -> bool {
    true
}

fn default_local_service_dir() -> String {
    "~/workspace/denpa-asr".to_string()
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
    // resolve_data_dir/config_without_data_dir 用例已随路径解析迁至 config/paths.rs(阶段A 第3步)

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
