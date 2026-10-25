# 改动流程

一条需求从提出到发布，固定按下面七步走。每一步都有产出物，没有产出就等于没做。

## 1. 需求 / 问题

- 用户描述现象或需求（通常是口语），**不一定是"要做"**。
- 这一阶段只做两件事：**读代码/查历史**，必要时给分析。**不动手改**。
- 涉及行为的问题，先查：`docs/SPEC.md` 与 git 历史（这个行为以前有没有？是回归还是新需求？）。

## 2. 方案确认（门禁）

- 给出方案要点：改哪些文件、判定规则、行为差异、代价。
- 有多个选项时列出差异，**等用户明确说"做 / ok / 继续"** 再动手。
- 规则：用户问"为什么/什么区别/怎么做"，只回答；**"做"是唯一的开工口令**。

## 3. 实现

- 只用编辑工具（`edit` / `write`）改代码与文档；**禁止** `sed -i`、`cp` 覆盖、Python 批量替换。
- 一处实现原则：新增能力先问"是否可以复用现有函数/模块"；同一判断不得出现第二份实现。
- 同步更新：模块顶部注释、`docs/SPEC.md`、示例（如有）、前端字段与界面。
- 命名反映职责：探测叫 `active_target`，不叫 `capture_target`；配置只在消费端使用。

## 4. 自测（门禁）

按顺序，全过才继续（详见 `04-verification.md`）：

1. `cargo check --all-targets`：0 警告、0 错误。
2. `cargo test`：全部通过（纯函数/规则必须有单测）。
3. `npm run build` + `node tests/frontend-checks.mjs`：全部 PASS（含"版本号六处一致"）。
4. 行为证据：探针输出 / e2e 日志 / 截图 / 尺寸与哈希。
5. 不通过不进入下一步；修不了就停下来报告原因。

## 5. 打包、签名、装机

```bash
npm run tauri build -- --bundles app            # 本地无 updater 私钥时签名报错可忽略
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
codesign --verify --deep --strict "src-tauri/target/release/bundle/macos/Denpa Jack.app"

osascript -e 'quit app "Denpa Jack"'; sleep 2
rm -rf "/Applications/Denpa Jack.app"
cp -R "src-tauri/target/release/bundle/macos/Denpa Jack.app" /Applications/
xattr -cr "/Applications/Denpa Jack.app"
open "/Applications/Denpa Jack.app"
```

装机后确认：`PlistBuddy -c Print:CFBundleShortVersionString` 与 `pgrep -x denpa-jack`。

## 6. 提交

- commit message 用固定结构：标题 / 问题 / 发现过程 / 实现方案 / 实现过程 / 总结。
- 写清"为什么改"，不是"改了哪些行"。
- 未获授权不 push、不打 tag。

## 7. 发布与汇报

- 发布流程见 `05-release-and-ops.md`（版本六处 → push → tag → Release → 验产物）。
- 汇报格式：结论先行 + 证据（数字/日志行/文件路径）+ 未做的事（需授权）。

## 流程中的红线

| 场景 | 必须做 | 禁止 |
|---|---|---|
| 用户提问 | 回答 / 分析 | 直接改代码 |
| 要碰用户桌面 | 先问 | 打开/激活应用、移动鼠标、代按键盘 |
| 改代码/文档 | 编辑工具 | sed / cp / python 批量替换 |
| 验证 | 真实执行并给证据 | 只看代码就声称"好了" |
| 发布 | 等"发布/继续"指令 | 擅自 push/tag/Release |
| 失败 | 停下说明原因 | 掩盖、绕过、假装成功 |
