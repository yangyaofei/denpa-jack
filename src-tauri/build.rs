// 跨边界契约的编译期强制(docs/CONTRACTS.md 的执行者):
// 坐标 API 只准 overlay.rs 出现——模块收口从"约定"升级为"编译失败"
fn main() {
    println!("cargo:rerun-if-changed=src/");
    // 写坐标(改变窗口位置)只准 overlay.rs; 读坐标允许验证者(autotest 断言)
    let overlay_only = ["setFrameOrigin", "CGDisplayBounds", "CGGetDisplaysWithPoint", "cursor_position"];
    let entries = std::fs::read_dir("src").expect("read src");
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        for api in overlay_only {
            if name != "overlay.rs" && content.contains(api) {
                panic!(
                    "契约违规: 坐标 API `{api}` 只允许出现在 overlay.rs(全 NS 链路), 发现于 {name}。\
                     见 docs/CONTRACTS.md 坐标系一节——三套坐标系互不兼容, 新坐标代码进 overlay.rs 并注明体系。"
                );
            }
        }
        // HUD 单一入口契约: 浮窗状态与窗口只允许 hud.rs(状态机/策略)与 overlay.rs(窗口机制)操作,
        // 其他模块只能调用 hud 模块的接口报"发生了什么事实"。见 docs/CONTRACTS.md 与 src/hud.rs 顶部。
        let hud_only = ["hud_set", "HudSnapshot", "show_hud(", "hide_hud(", "show_hud_msg("];
        if name != "hud.rs" && name != "overlay.rs" && name != "build.rs" {
            for api in hud_only {
                if content.contains(api) {
                    panic!(
                        "契约违规: `{api}` 只允许出现在 hud.rs / overlay.rs——\
                         HUD 的状态与显隐必须由 hud 模块统一管理, 其他模块只能调用它的接口(见 src/hud.rs 顶部)。\
                         发现于 {name}。"
                    );
                }
            }
        }
        // NS 主线程-only API 的调用文件必须同时出现主线程调度标记
        let main_only = ["NSStatusItem", "NSPasteboard", "setFrameOrigin", "NSSound"];
        let uses_main_only = main_only.iter().any(|a| content.contains(a));
        if uses_main_only
            && name != "audio_feedback.rs" // NSSound 特例: play_on_main 内部已调度
            && !content.contains("run_on_main_thread")
            && !content.contains("MainThreadMarker")
        {
            panic!(
                "契约违规: {name} 使用主线程-only AppKit API 但未见 run_on_main_thread/MainThreadMarker 调度。\
                 见 docs/CONTRACTS.md 线程模型一节。"
            );
        }
    }
}
