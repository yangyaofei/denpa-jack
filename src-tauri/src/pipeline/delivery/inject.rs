//! 文本注入：把一段文本放进目标输入框（用户定则 2026-11-10 封装）。
//!
//! 对外只暴露**意图入口**（[`inject_focused`] / [`inject_background`]），
//! 调用方（pipeline::deliver）不再感知 AX 属性、placeholder 陷阱、粘贴
//! 名单、策略顺序——那些是"输入框的多样性"，全部封装在本模块的两层里：
//!
//! - **探测层** [`probe`]：一次读齐目标状态（有效值/占位符是否在显示/
//!   bundle id），归成 [`TargetState`]——回答"这个框现在是什么情况"。
//! - **策略层** [`choose`]：TargetState → [`Strategy`]——回答"这种情况下
//!   该怎么写"。纯函数，可单测。
//!
//! 新输入框特性（自定义编辑器把 placeholder 固化、AX 写入重复输入、…）
//! 只改这两层；调用方永远是同一个入口，不叠 if-else。
//!
//! 已知特性登记（探测/策略依据， 持续追加）：
//! - Web 编辑器（Electron/自定义 contenteditable）：空框显示 placeholder 时，
//!   AX 选区写入会把 placeholder 从显示态固化成内容态（2026-11-10 OpenCode
//!   真机事故：placeholder 进了消息正文）→ 空框一律粘贴。
//! - AXValue 读到的"内容"可能是占位符文本（非原生 placeholder 属性的框）→
//!   与 AXPlaceholderValue 比对，相等视为空框（2026-11-03 后台写事故）。
//! - 名单内 App 的 AX 写入会重复输入（config.ax_paste_only_apps）→ 只粘贴。

use crate::capture::FocusLock;

/// 目标输入框的探测结果（探测层的输出，策略层的输入）。
/// 判据全部来自 2026-11-17 axscan 83 份实测（四类形态见 choose 注释）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TargetState {
    /// 前台应用 bundle id（名单判定用；None=解析失败）
    pub bundle: Option<String>,
    /// 空框且 AXValue 读到的是占位符文本（与 AXPlaceholderValue 去空白相等）
    pub placeholder_showing: bool,
    /// 内容不可信（placeholder 属性缺失且 value 含零宽字符——微信类编辑器
    /// 把提示文本拼进 value, 读到的内容不能作为任何决策依据）
    pub untrusted: bool,
    /// 焦点仍是快照时的目标（false=焦点已变，走后台语义）
    pub focus_ok: bool,
}

/// 写入策略（策略层输出；执行顺序与回退关系封装在各策略的实现里）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// 粘贴（Cmd+V）——不触碰编辑器内部态，一切"不确定"情形的兜底，
    /// 也是 placeholder 显示中/名单命中/clipboard_only 之外的 AX 不可用框的唯一安全路径
    Paste,
    /// 先试 AX 选区写入（不碰剪贴板），失败回退粘贴
    TryAxThenPaste,
}

/// 纯函数：目标状态 → 写入策略。
/// 四类形态判据(2026-11-17 axscan 83 份实测定稿)：
/// ① ph 在 + value 去空白==ph → 空框显示占位符(OpenChamber/标准框) → 只粘贴
/// ② ph 在 + value 干净 → 正常框 → AX→粘贴
/// ③ ph 无 + value 含零宽字符 → 微信类(提示文本拼在 value) → 只粘贴
/// 名单/clipboard_only/焦点失效同走只粘贴。
pub fn choose(state: &TargetState, clipboard_only: bool, paste_only_apps: &[String]) -> Strategy {
    // clipboard_only 是全局开关（不探测，直接粘贴语义=只写剪贴板由调用方执行）
    if clipboard_only {
        return Strategy::Paste;
    }
    // 粘贴优先名单：bundle 命中（前缀匹配，如 "com.example" 覆盖其子 App）
    if let Some(b) = state.bundle.as_deref() {
        if paste_only_apps.iter().any(|p| b == p || b.starts_with(&format!("{p}."))) {
            return Strategy::Paste;
        }
    }
    // ① placeholder 显示中的空框：AX 选区写入会触发 Web 编辑器把占位符固化(见模块注释)
    if state.placeholder_showing {
        return Strategy::Paste;
    }
    // ③ 内容不可信(微信类)：读到的 value 混有编辑器自拼的提示文本，
    //    任何"读改写"都会固化虚文本 → 只粘贴
    if state.untrusted {
        return Strategy::Paste;
    }
    // 焦点校验失败的框不允许 AX 直写（写错目标），由调用方转后台/粘贴
    if !state.focus_ok {
        return Strategy::Paste;
    }
    Strategy::TryAxThenPaste
}

/// 探测当前前台焦点输入框的状态（一次读齐：占位符判定+内容可信度+焦点有效性）。
/// 任何一步读不到都保守归位（不命中名单; 判定 false=按正常框走，回退链兜底）。
pub fn probe(focus: &FocusLock) -> TargetState {
    let cs = crate::pipeline::delivery::ax::focused_content_state();
    TargetState {
        bundle: focus.bundle.clone(),
        placeholder_showing: cs.placeholder_showing,
        untrusted: cs.untrusted,
        focus_ok: focus.still_valid(),
    }
}

/// 前台注入入口（pipeline::deliver 调用）：探测 → 选策略 → 执行。
/// 返回交付标记（pasted-ax / pasted-cmdv），剪贴板写入由调用方先完成
/// （用户定则：无论走哪条路径，文本必须先在剪贴板里可拿）。
pub fn inject_focused(
    app: &tauri::AppHandle,
    text: &str,
    clipboard_only: bool,
    focus: &FocusLock,
) -> Result<String, String> {
    let state = probe(focus);
    let cfg = crate::config::get_config(app.clone()).unwrap_or_default();
    let strategy = choose(&state, clipboard_only, &cfg.ax_paste_only_apps);
    crate::log::elog(&format!(
        "[inject] 策略={strategy:?} placeholder_showing={} bundle={:?}",
        state.placeholder_showing, state.bundle
    ));
    match strategy {
        Strategy::Paste => Ok("pasted-cmdv".into()),
        Strategy::TryAxThenPaste => match crate::pipeline::delivery::ax_insert(text) {
            Ok(()) => Ok("pasted-ax".into()),
            Err(e) => {
                crate::log::elog(&format!("[inject] AX 写入失败回退粘贴: {e}"));
                Ok("pasted-cmdv".into())
            }
        },
    }
}

/// 后台注入入口（焦点已变时对快照元素写入）：探测与追加语义封装在
/// ax::bg_append_write 内（读有效值时已做 placeholder 归零），此处保留
/// 意图入口供调用方，实现细节（读改写/回读验证）不外泄。
pub fn inject_background(el: &crate::capture::FocusElement, text: &str) -> Result<(), String> {
    crate::pipeline::delivery::ax::bg_append_write(el, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(bundle: Option<&str>, ph: bool, focus_ok: bool) -> TargetState {
        TargetState { bundle: bundle.map(String::from), placeholder_showing: ph, untrusted: false, focus_ok }
    }

    #[test]
    fn untrusted_content_never_ax() {
        // 微信类(2026-11-17 axscan 实测): ph 属性缺失 + value 含零宽字符
        // → 读到的内容混有编辑器自拼的提示文本, 读改写会固化虚文本 → 只粘贴
        let mut s = state(Some("com.tencent.xinWeChat"), false, true);
        s.untrusted = true;
        assert_eq!(choose(&s, false, &[]), Strategy::Paste);
    }

    #[test]
    fn clipboard_only_always_pastes() {
        assert_eq!(choose(&state(None, false, true), true, &[]), Strategy::Paste);
        assert_eq!(choose(&state(None, false, true), true, &["a.b.c".to_string()]), Strategy::Paste);
    }

    #[test]
    fn placeholder_showing_never_ax() {
        // OpenCode 事故回归: 空框显示占位符时, AX 选区写入会把占位符固化成正文
        assert_eq!(choose(&state(None, true, true), false, &[]), Strategy::Paste);
    }

    #[test]
    fn paste_only_list_matches_bundle() {
        assert_eq!(choose(&state(Some("dev.opencode.app"), false, true), false, &["dev.opencode.app".into()]), Strategy::Paste);
        // 前缀匹配覆盖子 App
        assert_eq!(choose(&state(Some("dev.opencode.app.helper"), false, true), false, &["dev.opencode.app".into()]), Strategy::Paste);
        assert_eq!(choose(&state(Some("com.other.app"), false, true), false, &["dev.opencode.app".into()]), Strategy::TryAxThenPaste);
    }

    #[test]
    fn focus_lost_never_ax() {
        assert_eq!(choose(&state(None, false, false), false, &[]), Strategy::Paste);
    }

    #[test]
    fn normal_box_tries_ax_then_paste() {
        assert_eq!(choose(&state(Some("com.apple.Term"), false, true), false, &[]), Strategy::TryAxThenPaste);
    }
}
