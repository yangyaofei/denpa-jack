// 权限总检查: 启动时一次性查清所有权限的状态, 缺哪一项都明确告知并能跳到系统设置
//
// 必需两项(缺了对应能力就不可用):
//   - microphone    (TCC 麦克风): 录音。授权状态由 AVFoundation 查询(权威来源, 非猜测)
//   - accessibility (TCC 辅助功能/AX): ①键盘监听与快捷键录制 ②把结果直接写进光标或自动粘贴
// 可选一项:
//   - screen_recording (TCC 屏幕录制): 只在开启「截图作为纠错上下文」时需要。常驻列表以便用户
//     随时查看状态与跳转设置; 不计入 missing(), 不参与启动提示与录音拦截。
//
// **不需要"输入监控"**: 只有"自研 CGEventTap listen-only 引擎"才需要它, 该引擎已在 C54 删除。
// 现在键盘层 = handy-keys(HotkeyManager/KeyboardListener, 内部 CGEventTap), 但库自己的权限要求是
// 辅助功能(listener.rs: AXIsProcessTrusted 检查); 录音期间的 esc 走 Carbon 全局快捷键(global-shortcut 插件)。
// 实机验证: 只授予辅助功能时热键/录制/AX 写入全部正常。
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PermState {
    pub key: &'static str,
    pub label: &'static str,
    pub granted: bool,
    /// authorized | denied | restricted | not_determined
    pub status: String,
    /// 必需(true)=缺失则影响可用性, 启动提示/录音拦截只看必需项; 可选(false)=开关式功能需要
    pub required: bool,
    pub purpose: &'static str,
    pub impact: &'static str,
    pub settings_url: &'static str,
}

/// 麦克风授权状态: 0 未决定 / 1 受限 / 2 拒绝 / 3 已授权
pub fn mic_status_code() -> i32 {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    let Some(media) = (unsafe { AVMediaTypeAudio }) else {
        return 0;
    };
    let st = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    if st == AVAuthorizationStatus::Authorized {
        3
    } else if st == AVAuthorizationStatus::Denied {
        2
    } else if st == AVAuthorizationStatus::Restricted {
        1
    } else {
        0
    }
}

pub fn mic_status_text() -> &'static str {
    match mic_status_code() {
        3 => "authorized",
        2 => "denied",
        1 => "restricted",
        _ => "not_determined",
    }
}

pub fn mic_granted() -> bool {
    mic_status_code() == 3
}

/// 主动请求麦克风权限: 未决定时弹系统框; 已拒绝则不会再弹(必须去系统设置)
pub fn request_mic() {
    use block2::RcBlock;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
    let Some(media) = (unsafe { AVMediaTypeAudio }) else {
        return;
    };
    let block = RcBlock::new(|_granted: objc2::runtime::Bool| {});
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
}

/// 屏幕录制权限状态（开关式功能：只有「截图作为纠错上下文」开启时才需要）
pub fn screen_recording_state() -> PermState {
    let granted = crate::screen_context::preflight();
    PermState {
        key: "screen_recording",
        label: "屏幕录制",
        granted,
        status: if granted { "authorized" } else { "denied" }.to_string(),
        required: false,
        purpose: "把当前屏幕截图作为纠错上下文(仅在「截图作为纠错上下文」开关开启时需要)",
        impact: "缺少它时截图采集失败：转写仍正常，只是不带屏幕上下文",
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    }
}

/// 全部权限的当前状态(供前端一次性展示；含可选项，前端按 required 区分展示)
pub fn all() -> Vec<PermState> {
    let ax = crate::deliver::ax_trusted(false);
    vec![
        PermState {
            key: "microphone",
            label: "麦克风",
            granted: mic_granted(),
            status: mic_status_text().to_string(),
            required: true,
            purpose: "录音(语音转文字的输入)",
            impact: "缺少它无法录音：按下热键会直接失败并提示",
            settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        },
        PermState {
            key: "accessibility",
            label: "辅助功能",
            granted: ax,
            status: if ax { "authorized" } else { "denied" }.to_string(),
            required: true,
            purpose: "把转写结果直接写进当前光标，或自动粘贴",
            impact: "缺少它仍能录音，但结果只能进剪贴板，不能自动输入",
            settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        },
        screen_recording_state(),
    ]
}

/// 缺失的**必需**权限 key 列表(空 = 必需项全部就绪)。
/// 可选项(屏幕录制)不计入：它缺了只影响开关式功能，不该在启动时提示或拦截录音。
pub fn missing() -> Vec<&'static str> {
    all().iter()
        .filter(|p| p.required && !p.granted)
        .map(|p| p.key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三项常驻；只有麦克风与辅助功能是必需，屏幕录制为可选
    #[test]
    fn all_has_three_entries_with_only_screen_optional() {
        let list = all();
        assert_eq!(list.len(), 3);
        let req: Vec<&str> = list.iter().filter(|p| p.required).map(|p| p.key).collect();
        assert_eq!(req, vec!["microphone", "accessibility"]);
        let opt: Vec<&str> = list.iter().filter(|p| !p.required).map(|p| p.key).collect();
        assert_eq!(opt, vec!["screen_recording"]);
        // 每项都要有跳系统设置的深链与用途说明（前端直接展示，缺了用户就不知道去哪改）
        for p in &list {
            assert!(p.settings_url.starts_with("x-apple.systempreferences:"), "{} 缺少设置深链", p.key);
            assert!(!p.purpose.is_empty() && !p.impact.is_empty(), "{} 缺少用途/影响说明", p.key);
        }
    }

    /// 可选项未授权也不进 missing（启动提示与录音拦截只看必需项）
    #[test]
    fn missing_excludes_optional_permissions() {
        assert!(!missing().contains(&"screen_recording"));
    }
}
