# 依赖说明：handy-keys（带本地补丁）

## 结论

`handy-keys`（跨平台全局键盘库，MIT）用于实现**全局快捷键**，因为需求包含 **Fn 键**与
**纯修饰键组合**，而 `tauri-plugin-global-shortcut` 官方限制“必须至少一个非修饰键、不支持 Fn”。

上游 0.3.4 有两个 macOS 键盘状态缺陷会导致本项目“修饰键卡住 / 纯修饰热键按不出来”，
因此在 **自己 fork 的分支**上打了补丁，通过 `Cargo.toml` 的 `[patch.crates-io]` 以**固定 commit** 引入：

```toml
[patch.crates-io]
handy-keys = { git = "https://github.com/yangyaofei/handy-keys", rev = "27edee4e7af2689630f5450887197a8dce7a0a1d" }
```

- 用 `rev` 而非 `branch`：分支后续变动不影响构建；`src-tauri/Cargo.lock` 也钉死同一版本。
- 首次构建需要网络从 GitHub 拉取（之后走 cargo 缓存）。

## 两处补丁（均在 `src/platform/macos/listener.rs`，macOS 专有模块，不影响 Windows/Linux）

### 补丁 1：FlagsChanged 全量对账

- **问题**：上游只在**非 FlagsChanged** 事件里调用 `reconcile_modifiers()` 对账。系统在 tap 被禁用
  （超时、Secure Input、锁屏、Mission Control 等）期间会丢事件；而“用 Cmd+V 粘贴后直接按纯修饰热键”
  的流程不产生普通键事件，于是丢失的修饰键抬起永远留在 tracked 状态里。修饰键匹配是精确匹配
  （`Modifiers::matches`），多余的位会让 `Fn` 单独不再匹配 → 热键失效，直到用户乱按几个普通键。
- **修法**：进入 FlagsChanged 分支时调用 `reconcile_modifiers()`，用事件自带的 `flags`（系统真值）
  修正 Cmd/Shift/Ctrl/Opt/FN 五组状态。

### 补丁 2：按下/抬起判定改用 flags 真值

- **问题**：上游用 tracked 状态反推（`was_set → is_key_down = !was_set`）。丢一次抬起后，该键
  后续判定会永久反转；且 Fn（keycode `0x3F`）也走这一分支，只存在于 flags 中，原逻辑会让它
  每次都判成抬起。
- **修法**：改用事件 `flags` 判定该修饰组是否按下，Fn 走 `flags_have_fn()`。
- **已知限制**：CGEventFlags 不区分左右。同组（如左右 Cmd）两键同时按住时松开一侧，该侧的位会
  保留到整组释放。组匹配（`Modifiers::matches` 按 compound 组任意一侧）不受影响，下一次对账清掉。

## 上游 PR 与合并后的收尾

- PR：https://github.com/handy-computer/handy-keys/pull/37
  （分支 `fix/missed-modifier-events-reconcile`；改动仅 `src/platform/macos/listener.rs`，25+/29−；
   `cargo check` 干净、`cargo test` 6 通过）
- **上游合入并发布修复版后**：
  1. 删除 `Cargo.toml` 的 `[patch.crates-io]` 段；
  2. 依赖改为 crates.io 的修复版（如 `handy-keys = "0.4"`）；
  3. 更新 `Cargo.lock`；4. 更新本文件。
- **需要再改补丁时**：在 fork 分支提交 → 更新这里的 `rev` → `cargo update -p handy-keys` → 跑 `cargo test --lib`。

## 验证方式

- 复现：按住 Cmd → 粘贴到非激活窗口（Cmd+V）→ 按纯修饰热键（如 Fn）→ 修复前失效，乱按几个键才恢复。
- 验证：用事件探针打印原始 CGEvent 字段，粘贴后立即按 Fn 连续 5 次全部正确；80ms 间隔连按 15 次
  得到 15 组完整 down/up。本项目 `cargo test --lib` 76 项全绿。
