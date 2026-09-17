//! 应用内更新（`tauri-plugin-updater`）：检查 → 下载 → 替换自身 → 重启。
//!
//! 为什么自己做更新、而不是让用户去网页下载：
//! 浏览器下载的文件会带 `com.apple.quarantine`，macOS 按"来自互联网的未受信任 App"处理，
//! 会重新索要麦克风/辅助功能权限（用户实测过）。我们自己下载不带该标记
//! （应用未声明 `LSFileQuarantineEnabled`），且新二进制满足同一份代码签名要求，
//! 因此 TCC 授权保持不变。
//!
//! 信任来源：更新包的 minisign 签名。公钥写在 `tauri.conf.json` 的 `plugins.updater.pubkey`，
//! 私钥只在 CI Secrets（`TAURI_SIGNING_PRIVATE_KEY[?]`）里 —— 校验在下载后、安装前强制进行，无法关闭。
use tauri::Emitter;

/// 检查结果（前端展示用）
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub version: String,
    pub notes: String,
    pub date: Option<String>,
}

async fn check_inner(
    app: &tauri::AppHandle,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| format!("更新器初始化失败: {e}"))?;
    updater.check().await.map_err(|e| format!("检查更新失败: {e}"))
}

/// 检查更新。返回 None = 已是最新；Some = 有可用版本（只报告，不安装）。
#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    match check_inner(&app).await? {
        Some(u) => {
            crate::log::elog(&format!(
                "[update] 发现新版本 {} (当前 {}) {}",
                u.version, u.current_version, u.download_url
            ));
            Ok(Some(UpdateInfo {
                current: u.current_version.clone(),
                version: u.version.clone(),
                notes: u.body.clone().unwrap_or_default(),
                date: u.date.map(|d| d.to_string()),
            }))
        }
        None => {
            crate::log::elog("[update] 已是最新版本");
            Ok(None)
        }
    }
}

/// 下载并安装更新，成功后重启应用（本函数不会返回）。
/// 下载进度通过 `update-progress` 事件推给前端。
#[tauri::command]
pub async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    let Some(u) = check_inner(&app).await? else {
        return Err("已是最新版本".into());
    };
    crate::log::elog(&format!("[update] 开始下载并安装 {}", u.version));
    let app_ev = app.clone();
    let mut downloaded: u64 = 0;
    u.download_and_install(
        move |chunk, total| {
            downloaded += chunk as u64;
            let _ = app_ev.emit(
                "update-progress",
                serde_json::json!({ "downloaded": downloaded, "total": total }),
            );
        },
        || {},
    )
    .await
    .map_err(|e| format!("安装更新失败: {e}"))?;
    crate::log::elog("[update] 安装完成, 重启应用");
    app.restart();
}
