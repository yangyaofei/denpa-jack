// 权限总检查: 启动时一次性查清所有"必需权限", 缺哪一项都明确告知并能跳到系统设置
//
// 本项目必需两项(缺了对应能力就不可用):
//   - microphone    (TCC 麦克风): 录音。授权状态由 AVFoundation 查询(权威来源, 非猜测)
//   - accessibility (TCC 辅助功能/AX): ①键盘监听与快捷键录制 ②把结果直接写进光标或自动粘贴
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

/// 屏幕录制权限状态（开关式功能：只有"截图作为纠错上下文"开启时才需要）
pub fn screen_recording_state() -> PermState {
    let granted = crate::screen_context::preflight();
    PermState {
        key: "screen_recording",
        label: "屏幕录制",
        granted,
        status: if granted { "authorized" } else { "denied" }.to_string(),
        purpose: "把当前屏幕截图作为纠错上下文(仅在「截图作为纠错上下文」开关开启时需要)",
        impact: "缺少它时截图采集失败：转写仍正常，只是不带屏幕上下文",
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    }
}

/// 全部必需权限的当前状态(供前端一次性展示)
pub fn all() -> Vec<PermState> {
    let ax = crate::deliver::ax_trusted(false);
    vec![
        PermState {
            key: "microphone",
            label: "麦克风",
            granted: mic_granted(),
            status: mic_status_text().to_string(),
            purpose: "录音(语音转文字的输入)",
            impact: "缺少它无法录音：按下热键会直接失败并提示",
            settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        },
        PermState {
            key: "accessibility",
            label: "辅助功能",
            granted: ax,
            status: if ax { "authorized" } else { "denied" }.to_string(),
            purpose: "把转写结果直接写进当前光标，或自动粘贴",
            impact: "缺少它仍能录音，但结果只能进剪贴板，不能自动输入",
            settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        },
    ]
}

/// 缺失的权限 key 列表(空 = 全部就绪)
pub fn missing() -> Vec<&'static str> {
    all().iter().filter(|p| !p.granted).map(|p| p.key).collect()
}
