# Denpa Jack

[English](README.md) | [中文](README.zh.md)

Denpa Jack 是未来道具研究所（Mirai Gadget Kenkyujo）的**未来道具 14 号机：电波劫持者**
（未来ガジェット14号機：電波ジャック / *Mirai Gajetto Jūyon-gōki: Denpa Jakku*）。
原作里它把录下的声音强制放送到秋叶原的每一块屏幕上；这个项目做其中真正有用的那一半——
**按下热键、说话，把文字强制送进你当前的光标位置**：语音转写 → 词典纠错 →（可选）LLM 润色
→ 经辅助功能接口直写，或回退 `Cmd+V` 粘贴。

macOS 菜单栏常驻（不占 Dock）。录音时浮窗显示实时转写文本与音量条。

## 核心特性

- **全局快捷键**：支持 `Fn`、纯修饰键组合（如 `ctrl+option`）、字母/数字/F1–F12；
  两种触发模式（按住说话 / 短按开始再短按结束）；设置页内可视化录制
- **ASR 引擎**：豆包流式 2.0（WebSocket 二进制协议，热词直传，实时 partial）/
  智谱 GLM-ASR（文件式，失败兜底）/ OpenAI Realtime（模块已实现）
- **词典纠错**（不依赖大模型）：变体替换 + 正则规范化 + **拼音模糊匹配**
  （近音音节按 0.5 代价、仅 3 字以上生效），三层护栏防误替换
- **LLM 润色**：多档案管理、**工具调用强制结构化回传**（不返回纯文本）、
  思考链开关与思考强度可选、prompt 可编辑（含 Markdown 预览）
- **交付**：优先 AX 直写目标应用，失败回退 `Cmd+V`；剪贴板可配置为
  「保留转写」或「用后恢复」
- **历史**：JSONL 记录 + 音频留存（可播放 / 重新转写 / 复制 / 在访达中显示），按上限自动裁剪
- **麦克风**：设备优先级、热插拔**事件驱动**刷新（不做轮询）、当前设备实时显示

## 工作流程

```
热键 ──► cpal 采集（原生格式 → 单声道 → 16 kHz，200 ms 分块）
    ──► ASR 引擎（partial 实时推给浮窗）
    ──► 松开：收尾帧 + 等服务端最终结果
    ──► 词典（规范化 → 变体替换 → 拼音模糊）
    ──► 可选 LLM 润色（工具调用；失败降级为词典结果）
    ──► 交付：AX 直写 → 回退 Cmd+V（按下时锁定目标应用）
    ──► 剪贴板 + 历史（+ 音频文件，按上限裁剪）
```

## 构建

```bash
npm install
npm run tauri dev          # 开发
```

打包（macOS）：

```bash
npm run tauri build
# 直接用证书文件签名（不需要钥匙串、不需要系统信任）
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
```

首次构建需联网：键盘层用的是固定在指定 commit 的**打过补丁的 `handy-keys`**——
见 [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md)。

## 签名与权限

签名用 [rcodesign](https://github.com/indygreg/apple-platform-rs) **直接读 `.p12` 文件**完成：
证书不必导入钥匙串，也不必让系统信任它。`codesign` 做不到这一点——用未信任的自签证书时它
直接拒绝签名（`CSSMERR_TP_NOT_TRUSTED` / `no identity found`）；而 rcodesign 产出的是标准
签名，`codesign --verify` 能通过，TCC 权限也正常工作（本机已端到端验证）。

- Bundle id 固定为 `io.github.yangyaofei.denpajack`；证书是本机自签的 `Denpa Jack Dev`
  （10 年），放在仓库内的 `certs/` 目录（该目录已 gitignore，不会入库）。
  这两样都不要变：macOS 的 TCC 按**代码要求**（identifier + 证书叶指纹）记录权限，
  换 bundle id 或换证书都会导致麦克风 / 辅助功能需要重新授权。
- **在别人的 Mac 上**：本地拷贝（U 盘、`scp`）能直接跑，但权限要在那台机器授权；
  带隔离标记的拷贝（AirDrop、浏览器、邮件）会被 Gatekeeper 拦截，需要
  *系统设置 → 隐私与安全性 → 仍要打开*（`xattr -dr com.apple.quarantine` 同样有效）。
  想让别人下载就能跑，只有 **Developer ID + 公证**一条路。
- CI/CD：rcodesign 也能在 Linux runner 上跑，配合 App Store Connect API Key 还能公证。

完整说明、实测证据与遇到的问题见 [docs/CODE-SIGNING.md](docs/CODE-SIGNING.md)。

## 测试

```bash
cd src-tauri && cargo test --lib      # 单元测试（76 项）
./scripts/regression.sh               # 回归闸门：单测 → 文件回放 → UI 数据链 → e2e → 打包签名
```

自测模式（环境变量显式激活，不污染正常启动）：
`VOICEMAC_AUTOTEST=ui`（与页面相同的数据链自检）、`VOICEMAC_AUTOTEST=e2e`（真实麦克风端到端）、
`VOICEMAC_AUTOTEST_FILE=<wav>`（音频文件回放，不使用麦克风）。

判定标准与用例矩阵见 [docs/TESTING.md](docs/TESTING.md)、[docs/TEST-MATRIX.md](docs/TEST-MATRIX.md)。

## 运行时数据

`~/Library/Application Support/io.github.yangyaofei.denpajack/`

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
| [docs/SPEC.md](docs/SPEC.md) | 需求与架构说明（功能清单、模块划分、数据流） |
| [docs/CODE-SIGNING.md](docs/CODE-SIGNING.md) | 用 rcodesign 直接读 `.p12` 签名、TCC 要求、实测证据与遇到的问题 |
| [docs/CI-CD.md](docs/CI-CD.md) | GitHub Actions：测试流水线、发版流水线、CI 需要的仓库凭据 |
| [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) | 依赖说明：handy-keys 的本地补丁、上游 PR、合并后收尾 |
| [docs/TESTING.md](docs/TESTING.md) / [docs/TEST-MATRIX.md](docs/TEST-MATRIX.md) | 测试组织与「每修必加用例」矩阵 |
| [docs/SYNC.md](docs/SYNC.md) | 前端内容与后端状态的同步关系（数据项 × 通道 × 时机） |
| [docs/HANDY-COMPAT.md](docs/HANDY-COMPAT.md) | 与 Handy 的逐组件对照（浮窗 / 托盘 / 粘贴 / 热键） |
| [docs/DEVIATION-AUDIT.md](docs/DEVIATION-AUDIT.md) | 与参照实现的有意偏差及原因 |
| [docs/FEATURE-BACKLOG.md](docs/FEATURE-BACKLOG.md) | 待办与明确不做的事项（含理由） |
| [docs/SPIKE-STREAMING.md](docs/SPIKE-STREAMING.md) | 流式 ASR 方案调研（本地/云端，含实测数据） |
| [docs/WINDOW-SEMANTICS.md](docs/WINDOW-SEMANTICS.md) | 窗口层级与焦点语义（不抢占焦点、浮窗定位） |
| [docs/icon/README.md](docs/icon/README.md) | 应用图标：定稿资产、生成脚本、历史决策 |

## 已知边界

- 目前仅 macOS（快捷键底层含 macOS 专用补丁）
- 自签证书：本机无摩擦，别的机器有摩擦（见上）
