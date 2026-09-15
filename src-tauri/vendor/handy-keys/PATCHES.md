# handy-keys 0.3.4 — 本地补丁（voice-input）

本目录是 crate `handy-keys` 0.3.4 的 vendored 副本，仅修改 `src/platform/macos/listener.rs`。
通过 `src-tauri/Cargo.toml` 的 `[patch.crates-io] handy-keys = { path = "vendor/handy-keys" }` 生效。

上游未修的 macOS 键盘状态缺陷，导致本项目遇到“修饰键卡住 / 纯修饰热键按不出来”问题
（典型场景：Cmd+V 粘贴后 Cmd 抬起通知被系统吞掉，随后按 Fn 类热键永不触发）。

## 补丁 1：FlagsChanged 全量对账（对应本项目约束 C58-①）

- 位置：`listener.rs` FlagsChanged 分支开头
- 问题：上游只在**非 FlagsChanged** 事件里对账（`reconcile_modifiers`）。系统在 tap 超时被禁用
  （Secure Input、锁屏、Mission Control）等窗口会丢事件；语音输入流程粘贴后直接按 Fn，没有普通键
  事件触发对账 → 过期的修饰键状态永远留在 tracked 里。
- 修法：FlagsChanged 事件自带 `flags`（系统真值），进入分支时先按 flags 全量修正
  Cmd / Shift / Ctrl / Opt 四组 tracked 位。

## 补丁 2：修饰键按下/抬起判定改用 flags 真值（对应本项目约束 C58-②）

- 位置：`listener.rs` `else if let Some(modifier_bit) = changed_modifier` 分支
- 问题：原实现用 tracked 状态反推（`was_set → is_key_down = !was_set`）。丢一次抬起后，
  下一次真实按下会被反转判为抬起，状态永久错乱。
- 修法：改用事件 `flags` 判断该修饰组是否按下（`group_in_flags`）；
  FN(keycode 0x3F) 走 `flags_have_fn(flags)` 判定，避免落入 `else { false }` 把 Fn 永远判为抬起。

## 复现与验证

- 补丁全文：`C58-listener.patch`（相对上游 0.3.4 的 `diff -u`）
- 上游 0.3.4 原始文件可用 `cargo add handy-keys@0.3.4` 或 crates.io 下载后对比
- 验证：`cd src-tauri && cargo test --lib`（76 项全绿）；实测用 `examples/kbd_probe.rs`
  （原始事件探针）确认“粘贴后立即按 Fn”不再失败

## 后续

理想做法是向上游提交 PR（两个修改都是通用缺陷，不限于本项目）；
在合入并发布新版本前，保持本 vendored 副本。
