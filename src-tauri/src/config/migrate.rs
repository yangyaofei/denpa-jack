// 配置域·一次性迁移(阶段A 第3步从 settings.rs get_config 拆出):
// 全部"读旧配置→改写成新形态→落盘"的迁移都集中在这里, 每段一个 if, 逐字未改。
// 迁移即落盘(同目录原子写), 下次读取不再触发。
use crate::config::paths::write_config_atomic;
use crate::config::{BindingSet, Config};

/// get_config 加载后调用: 按序跑四段迁移(C54 bindings → F5 兜底 → 内置引擎 → VAD 归零)
pub(crate) fn migrate_on_load(cfg: &mut Config, p: &std::path::Path) {
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
            let mut normed: Vec<String> = set.iter().map(|s| crate::config::normalize_combo(s)).filter(|s| !s.is_empty()).collect();
            normed.sort();
            normed.dedup();
            normed
        };
        cfg.bindings.insert("transcribe".into(), BindingSet { current: set });
        // 迁移即落盘(旧字段 skip_serializing 不再写出, 重复组合已归一去重)
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = write_config_atomic(p, &json);
        }
    }
    // F5 兜底(审核a-3): 旧迁移条件"≠F5 才迁"把热键恰好等于默认 F5 的用户漏掉——
    // bindings 为空且从未有 bindings 键 → 无热键可按。这里对"从未有键"的配置补默认 F5;
    // 只查键不存在(不查 current 空)——用户显式清空全部热键的选择不被覆盖。
    if !cfg.bindings.contains_key("transcribe") {
        cfg.bindings.insert("transcribe".into(), BindingSet { current: vec!["F5".to_string()] });
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = write_config_atomic(p, &json);
        }
    }
    // 内置引擎迁移(2026-10-08): 激活档案是"local 且地址为空"的旧形态 → 切到
    // 特化的 builtin id(type/name 定死, 随机端口联动); 空地址 local 档案移除
    // (外部网关档案必须显式填地址)。迁移即落盘。
    if cfg.active_asr_id != crate::engine::gateway::BUILTIN_ID {
        let old_id = cfg
            .asr_profiles
            .iter()
            .find(|x| x.id == cfg.active_asr_id && x.provider == "local" && x.base_url.trim().is_empty())
            .map(|x| x.id.clone());
        if let Some(old_id) = old_id {
            cfg.active_asr_id = crate::engine::gateway::BUILTIN_ID.into();
            cfg.asr_profiles.retain(|x| x.id != old_id);
            if let Ok(json) = serde_json::to_string_pretty(&cfg) {
                let _ = write_config_atomic(p, &json);
            }
        }
    }
    // VAD 一次性迁移: 0.3.23 曾把默认 ten 写进配置; 当天实测 ten 吃气声口述首尾帧
    // (rec_1791445446593/460175, 音频完整模型直读全对而交付缺头少尾), 默认改关。
    // 该值只可能是默认带入(0.3.23 仅存在数小时), 强制归零不冤枉任何显式选择。
    if cfg.builtin_engine.vad == "ten" {
        cfg.builtin_engine.vad = String::new();
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = write_config_atomic(p, &json);
        }
    }
}
