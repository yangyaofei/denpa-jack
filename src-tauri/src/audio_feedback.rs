// 音频反馈(对齐 Handy audio_feedback): 录音开始/结束系统提示音
// NSSound 系统音效(Tink=开始, Pop=结束), 主线程调用
use objc2::MainThreadMarker;
use objc2_app_kit::{NSSound, NSSoundName};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Start,
    End,
}

impl Cue {
    fn sound_name(self) -> &'static str {
        match self {
            Cue::Start => "Tink",
            Cue::End => "Pop",
        }
    }
}

/// 播放提示音(静默失败: 提示音是增强, 不阻塞主流程)
pub fn play(cue: Cue) {
    if MainThreadMarker::new().is_some() {
        let name = NSSoundName::from_str(cue.sound_name());
        if let Some(snd) = NSSound::soundNamed(&name) {
            snd.setVolume(0.35);
            snd.play();
        }
    }
}

/// 从任意线程触发播放(hop 到主线程, NSSound 需要)
pub fn play_on_main(app: &tauri::AppHandle, cue: Cue) {
    let _ = app.run_on_main_thread(move || play(cue));
}
