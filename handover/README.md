# 交接文档索引（handover）

本目录记录 Denpa Jack 的**产品要求、改动流程、验证与发布方式、协作纪律**，以及必要的代码结构说明。目标：零上下文的人能按这里的方式继续改这个项目。

## 阅读顺序

| 顺序 | 文档 | 内容 |
|---|---|---|
| 1 | [`02-project-and-requirements.md`](02-project-and-requirements.md) | 项目是什么、整体需求、功能范围与非目标 |
| 2 | [`03-change-process.md`](03-change-process.md) | 一次改动从提出到发布的完整流程（含门禁） |
| 3 | [`04-verification.md`](04-verification.md) | 验证体系：测试、自测通道、证据形式 |
| 4 | [`05-release-and-ops.md`](05-release-and-ops.md) | 版本纪律、签名、CI、Release、装机与善后 |
| 5 | [`06-collaboration-rules.md`](06-collaboration-rules.md) | 协作纪律（哪些事必须等指令、哪些事不能做） |
| 6 | [`07-lessons.md`](07-lessons.md) | 这一轮踩过的坑与由此定下的规则 |
| 参考 | [`01-architecture.md`](01-architecture.md) | 代码结构速查（活动目标探测、截图、浮窗） |

## 当前状态（截至 2026-10-24）

- 最新版本 **0.3.29**，已发布（`v0.3.29`，GitHub Release 非 draft / 非 prerelease）。
- 发布资产：`Denpa.Jack_0.3.29_aarch64.app.tar.gz`（+`.sig`）、`.app.zip`、`.dmg`、`latest.json`、`SHA256SUMS_0.3.29.txt`。
- 产物已回验：`codesign --verify --strict` 通过、版本 0.3.29、`latest.json` 签名一致。
- 本机 `/Applications/Denpa Jack.app` 为 0.3.29 基线 + 结构重构（见下），正常运行。

## 结构重构（2026-10-24，阶段A 五步，未发版）

0.3.29 之后做了一轮纯结构重构（行为零变化，未发版）：

- 全局 static 收敛进 `core.rs`（CoreState 单锁，版本计数轮询协议）；
- 引擎域：`engines.rs`/`doubao.rs` 等五文件归位 `engine/` 目录，trait `AsrEngine` 统一入口；
- `recording.rs` 工具函数各回各家（麦克风→`audio.rs`，热词预算→`dict.rs`，WAV 解析→`history.rs`，截图编排→`screen_context.rs`）；
- `settings.rs` 拆出 `settings/paths.rs`（数据目录/配置路径）与 `settings/migrate.rs`（配置迁移），提示词归位 `llm.rs`——阶段A 后续已统一改名 `config.rs` + `config/`；
- 交付层 `deliver.rs`+`paste_tx.rs` 分家为 `delivery/`（`ax.rs`/`mod.rs`/`paste.rs`）；
- 前端 `main.ts` 926 行按页拆分 `src/app/`（types/util/common + 六页域），HTML 零改动。

## 本轮覆盖的改动

- **0.3.28**：截图从 `screencapture` 命令行改为 ScreenCaptureKit；新增目标选项（窗口/屏幕）；浮窗按下即显示。
- **0.3.29**：截图与浮窗共用同一次"活动目标"探测（焦点窗口 → 指针窗口 → 指针所在屏）；配置键 `screenshot_target` → `detect_active_target`。

## 不在本目录范围

- 语音引擎（豆包 / 智谱 / 内置本地网关）、词典纠错、LLM 润色、历史记录、自动更新：见 `docs/SPEC.md` 与对应模块注释。
- 本地 ASR 服务（`~/workspace/denpa-asr`）：见该仓库与 `research-plan/voice-mac-app/local-asr/PLAN.md`。
