// 配置域·路径解析(阶段A 第3步从 settings.rs 拆出):
// 数据目录的锚点/缓存/解析都在这里——"路径怎么算"与"配置里有什么"是两件事。
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

/// bundle id 单一来源(tauri.conf.json 的 identifier 必须与此一致)
/// 数据目录 ~/Library/Application Support/<APP_ID>/, 同时用于自粘贴焦点判定;
/// 改名时必须同步 tauri.conf.json + 迁移旧目录, 否则日志/llm_logs/历史会分叉
pub const APP_ID: &str = "io.github.yangyaofei.denpajack";

/// 默认数据目录（也是 config.json 的固定位置，作为一切路径的锚点）
pub fn base_dir(app: &tauri::AppHandle) -> PathBuf {
    // 自测隔离: 测试副本(改 bundle id 无效——tauri 的 identifier 构建时烘焙)用
    // 环境变量把整个数据目录指到独立位置, 避免读到真实配置/写脏真实历史。
    // 正常启动不设此变量, 走 app_config_dir。
    if let Ok(d) = std::env::var("VOICEMAC_DATA_DIR") {
        if !d.trim().is_empty() {
            let p = PathBuf::from(d);
            fs::create_dir_all(&p).ok();
            return p;
        }
    }
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

/// 数据根目录（带缓存，启动后恒定——D10: 配置改 data_dir 不中途切换，重启才生效；
/// app.log 句柄/history.jsonl 正在写的文件不该被切到新目录造成两边各半）。
/// history/recordings/llm_logs/app.log 都从这里派生。
pub fn data_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Some(p) = DATA_ROOT.lock().unwrap().clone() {
        return p;
    }
    let base = base_dir(app);
    let configured = crate::config::get_config(app.clone())
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
    // 自测隔离与 base_dir 同源(见 base_dir 注释)
    if let Ok(d) = std::env::var("VOICEMAC_DATA_DIR") {
        if !d.trim().is_empty() {
            let p = PathBuf::from(d);
            fs::create_dir_all(&p).ok();
            return p;
        }
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

/// 原子写 config.json(审核a-4): 先写同目录临时文件再 rename——写盘中途崩溃
/// (进程死亡真实发生过, 见 crash_log)不会留下半截 JSON 让全部配置不可读。
/// rename 在同一文件系统上原子, 半截文件只可能是 .tmp(下次覆盖)。
pub(crate) fn write_config_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
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
        let cfg: crate::config::Config = serde_json::from_str("{}").expect("空对象应可解析");
        assert_eq!(cfg.data_dir, "");
    }
}
