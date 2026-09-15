# VoiceInput

macOS 语音输入工具：**按住快捷键说话 → 松开 → 转写 → 词典纠错 →（可选）LLM 润色 → 写入剪贴板并粘贴到当前应用**。
菜单栏常驻（不占 Dock），录音时浮窗实时显示转写文本与音量。

## 核心特性

- **全局快捷键**：支持 `Fn`、纯修饰键组合（如 `ctrl+option`）、字母/数字/F1–F12；两种触发模式（按住说话 / 短按开始再短按结束）；设置页内可视化录制
- **ASR 引擎**：豆包流式 2.0（WebSocket 二进制协议，热词直传，实时 partial）/ 智谱 GLM-ASR（文件式，失败兜底）/ OpenAI Realtime（模块已实现）
- **词典纠错**（不依赖大模型）：变体替换 + 正则规范化 + **拼音模糊匹配**（近音音节按 0.5 代价、仅 3 字以上生效），带三层护栏防误替换
- **LLM 润色**：多档案管理、**工具调用强制结构化回传**（不裸输出）、思考链开关与思考强度可选、prompt 可编辑（含 Markdown 预览）
- **交付**：优先 AX 直写目标应用，失败回退 `Cmd+V`；剪贴板可配置为「保留转写」或「用后恢复」
- **历史**：JSONL 记录 + 音频留存（可播放 / 重新转写 / 复制 / 定位），按上限自动裁剪
- **麦克风**：设备优先级、热插拔**事件驱动**刷新（不按秒轮询）、当前设备实时显示

## 技术栈

Tauri 2（Rust）+ 原生 TypeScript + Alpine.js + Vite。无前端框架依赖。
约 7.4k 行源码（`src/` + `src-tauri/src/`）。

## 快速开始

```bash
npm install
npm run tauri dev
```

打包（macOS）：

```bash
npm run tauri build
codesign --force --deep -s "VoiceInput Dev" src-tauri/target/release/bundle/macos/VoiceInput.app
```

- 签名身份 `VoiceInput Dev` 是**本机自签证书**；Bundle ID 固定为 `com.yangyaofei.tauri-app`，
  保持不变以保证系统授权（麦克风 / 辅助功能）在重打包后继续有效。
- 首次构建需联网拉取打了补丁的 `handy-keys`（见 `docs/DEPENDENCIES.md`）。

## 测试

```bash
cd src-tauri && cargo test --lib      # 单元测试（76 项）
./scripts/regression.sh               # 回归闸门：单测 → 文件回放链路 → UI 数据链 → e2e → 打包签名
```

自测通道（环境变量显式激活，不污染正常启动）：
`VOICEMAC_AUTOTEST=ui`（页面同款数据链自检）、`VOICEMAC_AUTOTEST=e2e`（真麦端到端）、
`VOICEMAC_AUTOTEST_FILE=<wav>`（音频文件回放，不碰麦克风）。

判定标准与用例矩阵见 `docs/TESTING.md`、`docs/TEST-MATRIX.md`。

## 运行时数据

`~/Library/Application Support/com.yangyaofei.tauri-app/`

| 文件 | 内容 |
|---|---|
| `config.json` | 全部配置（档案、词典、快捷键、开关） |
| `app.log` | 诊断日志（`[diag]` 后端 / `[ui]` 前端 / 设备与键盘事件） |
| `history.jsonl` | 转写历史（原文 / 最终文本 / 引擎 / 交付方式 / 音频路径） |
| `recordings/` | 录音留存（按上限裁剪） |
| `llm_logs/` | 每次 LLM 调用的请求与响应全文 |

## 文档索引

| 文档 | 内容 |
|---|---|
| `docs/SPEC.md` | 需求与架构说明（功能清单、模块划分、数据流） |
| `docs/DEPENDENCIES.md` | 依赖说明：handy-keys 的两处本地补丁、上游 PR、合并后收尾 |
| `docs/TESTING.md` / `docs/TEST-MATRIX.md` | 测试组织与「每修必加用例」矩阵 |
| `docs/SYNC.md` | 前端内容与后端状态的同步关系（数据项 × 通道 × 时机） |
| `docs/HANDY-COMPAT.md` | 与 Handy 的逐组件对照（浮窗 / 托盘 / 粘贴 / 热键） |
| `docs/DEVIATION-AUDIT.md` | 与参照实现的有意偏差及原因 |
| `docs/FEATURE-BACKLOG.md` | 待办与明确不做的事项（含理由） |
| `docs/SPIKE-STREAMING.md` | 流式 ASR 方案调研（本地/云端，含实测数据） |
| `docs/WINDOW-SEMANTICS.md` | 窗口层级与焦点语义（不抢焦点、浮窗定位） |
| `docs/icon/README.md` | 应用图标：定稿资产、生成脚本、历史决策 |

## 已知边界

- 目前仅 macOS（Windows / Linux 未适配；快捷键底层含 macOS 专用补丁）
- 自签证书与 `VoiceInput Dev` 签名仅本机开发使用
