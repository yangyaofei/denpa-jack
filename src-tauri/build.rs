// 跨边界技术契约与四域门面的编译期强制闸（递归扫描 src/，违反即编译失败）：
//   1. 坐标系闸：NS（主屏左下、y 向上）/ CG（主屏左上、y 向下）/ Tauri 三套坐标系互不兼容，
//      所有坐标写入 API 仅允许出现在 ui/overlay.rs。
//   2. HUD 单一入口闸：浮窗状态与窗口控制只允许在 ui/hud.rs 与 ui/overlay.rs 出现，
//      其余模块只向 core 写事实或调 ui:: 门面。
//   3. 主线程 AppKit 闸：NSStatusItem / NSPasteboard / setFrameOrigin / NSSound 仅限主线程，
//      使用文件必须显式含 run_on_main_thread 或 MainThreadMarker 调度标记。
//   4. 四域门面闸：capture / model / pipeline / ui 跨域引用必须命中 FACADE 白名单
//      （门面动词表达意图，禁止暴露通道/线程等机制词；豁免表已清零）。
//   5. Tauri ACL 编译：末尾调用 tauri_build::build() 生成 capabilities 与 ACL 清单到 OUT_DIR。
fn main() {
    println!("cargo:rerun-if-changed=src/");
    // 递归收集 src/ 下全部 .rs(域子目录 engine/ 等同样受闸——搬家不豁免契约)
    let mut files: Vec<(String, String, String)> = Vec::new(); // (相对路径, basename, 内容)
    collect_rs("src", "", &mut files);
    for (rel, name, content) in files {
        // 写坐标(改变窗口位置)只准 overlay.rs; 读坐标允许验证者(autotest 断言)
        let overlay_only = ["setFrameOrigin", "CGDisplayBounds", "CGGetDisplaysWithPoint", "cursor_position"];
        for api in overlay_only {
            if name != "overlay.rs" && content.contains(api) {
                panic!(
                    "契约违规: 坐标 API `{api}` 只允许出现在 ui/overlay.rs(全 NS 链路), 发现于 {name}。\
                     三套坐标系互不兼容, 新坐标代码进 ui/overlay.rs 并注明体系。"
                );
            }
        }
        // HUD 单一入口契约: 浮窗状态与窗口只允许 ui/hud.rs(状态机/策略)与 ui/overlay.rs(窗口机制)操作,
        // 其他模块只能调用 ui:: 门面或向 core 报事实(见 ui/hud.rs 顶部注释)。
        let hud_only = ["hud_set", "HudSnapshot", "show_hud(", "hide_hud(", "show_hud_msg("];
        if name != "hud.rs" && name != "overlay.rs" && name != "build.rs" {
            for api in hud_only {
                if content.contains(api) {
                    panic!(
                        "契约违规: `{api}` 只允许出现在 ui/hud.rs / ui/overlay.rs——\
                         HUD 的状态与显隐必须由 hud 模块统一管理(见 ui/hud.rs 顶部注释)。\
                         发现于 {name}。"
                    );
                }
            }
        }
        // NS 主线程-only API 的调用文件必须同时出现主线程调度标记
        let main_only = ["NSStatusItem", "NSPasteboard", "setFrameOrigin", "NSSound"];
        let uses_main_only = main_only.iter().any(|a| content.contains(a));
        if uses_main_only
            && name != "feedback.rs" // NSSound 特例: play_on_main 内部已调度(原名 audio_feedback.rs)
            && !content.contains("run_on_main_thread")
            && !content.contains("MainThreadMarker")
        {
            panic!(
                "契约违规: {name} 使用主线程-only AppKit API 但未见 run_on_main_thread/MainThreadMarker 调度。"
            );
        }
        // ───────── 四域门面闸（见 lib.rs 顶部架构契约）─────────
        // 域文件引用他域只许经 crate::域::{门面符号}（FACADE 白名单）；根目录文件=装配壳+基座，豁免。
        let src_domain = rel.split('/').next().unwrap_or("");
        let is_domain_file = matches!(src_domain, "capture" | "ui" | "pipeline" | "model");
        if is_domain_file {
            let facade: &[(&str, &[&str])] = &[
                ("capture", &["ctrl_start", "ctrl_stop", "ctrl_abort", "esc_listen_begin", "esc_maybe_end", "RecordingSession", "list_mics", "resolve_mic", "ax_trusted", "focused_element_info", "frontmost_bundle", "FocusInfo", "FocusLock", "FocusElement", "inline_image", "data_url_prefix", "InlineImage"]),
                ("model", &["transcribe", "retranscribe", "resolve_engine", "cancel_current", "refine", "selftest", "list_models", "budget_hotwords", "Cmd", "AsrEvent"]),
                ("pipeline", &["SessionHandoff", "mark_cancelled", "cancel_latest", "submit_live", "submit_failed", "record_no_speech", "is_cancelled", "clear_cancelled_for_test"]),
                ("ui", &["recording_started", "recording_ended", "dismiss", "refresh_status", "display_scale", "DisplayTarget", "cursor_point", "display_target_at", "main_display_target"]),
            ];
            // 存量豁免(前缀匹配): 已清零——domain 间只剩门面符号与 core 两条通道。
            // 新增跨域需求: 进 FACADE 门面, 或反向通知走 core; 严禁再登记豁免。
            let exempt: &[(&str, &str)] = &[];
            for dom in ["capture", "model", "pipeline", "ui"] {
                if dom == src_domain {
                    continue;
                }
                let pat = format!("crate::{}::", dom);
                let mut from = 0usize;
                while let Some(rel_pos) = content[from..].find(&pat) {
                    let pos = from + rel_pos;
                    let after = &content[pos + pat.len()..];
                    let seg: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    if seg.is_empty() {
                        // use crate::ui::{a,b} 的花括号形态: 导入的符号由 Rust 保证存在,
                        // 且子模块下钻(crate::ui::hud::)仍会被下一轮拦截——放行
                        from = pos + pat.len();
                        continue;
                    }
                    let context: String = content[pos..].chars().take(130).collect();
                    let ok_facade = facade
                        .iter()
                        .find(|(d, _)| *d == dom)
                        .map(|(_, syms)| syms.iter().any(|s| !seg.is_empty() && *s == seg))
                        .unwrap_or(false);
                    let ok_exempt = exempt.iter().any(|(p, _)| context.starts_with(p));
                    if !ok_facade && !ok_exempt {
                        panic!(
                            "门面违规: `crate::{}::{}` 不在 {} 域门面白名单, 也不在豁免登记表——\
                             外部只许经门面符号调用(见 lib.rs 与各域 mod.rs 门面说明)。发现于 {}。\n\
                             豁免表登记项: {:?}",
                            dom, seg, dom, rel, exempt
                        );
                    }
                    from = pos + pat.len();
                }
            }
        }
    }
    // 生成 Tauri ACL 与 capabilities 到 OUT_DIR（供 tauri::generate_context! 解析权限，
    // 缺此步会导致 capabilities/default.json 里的 core:window:allow-start-dragging 等全部失效）
    tauri_build::build();
}

/// 递归收集 .rs 文件: (相对路径, 文件名, 内容)。文件名用 basename 判等——子目录里的
/// overlay.rs/hud.rs(将来 ui/overlay.rs 等)与平铺时同规则豁免。
fn collect_rs(dir: &str, prefix: &str, out: &mut Vec<(String, String, String)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let path = e.path();
        let base = path.file_name().unwrap().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { base.clone() } else { format!("{}/{}", prefix, base) };
        if path.is_dir() {
            collect_rs(&path.to_string_lossy(), &rel, out);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        out.push((rel, name, content));
    }
}
