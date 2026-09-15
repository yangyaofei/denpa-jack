# AGENTS.md — Denpa Jack

未来道具14号机：电波劫持者（Denpa Jack / 未来ガジェット14号機：電波ジャック）。

macOS 菜单栏语音输入：**按住热键说话 → ASR → 词典纠错 →（可选）LLM 润色 → 自动上屏**。
本文件是这个仓库的契约（给在这里工作的人 / agent 看）。用户级工作规则另见 `~/.config/opencode/AGENTS.md`。

## 一句话架构

Rust 后端（`src-tauri/src/`）负责录音、识别、纠错、交付；WebView 前端（`src/`）只负责展示与配置。
两端唯一通道 = Tauri command（调用）+ 事件（推送，含轮询兜底，见 `docs/SYNC.md`）。
完整模块地图与数据流见 `docs/SPEC.md`（架构权威文档）。

## 目录

| 路径 | 内容 |
|---|---|
| `src-tauri/src/` | 后端模块：`audio`（采集/重采样）`recording`（会话）`engines`（ASR 调度）`doubao`/`zhipu_file`（引擎）`dict`（词典纠错）`llm`（润色）`pipeline`（交付）`deliver`（AX/粘贴）`tray`/`overlay`（菜单栏/浮窗）`shortcut`（键盘）`settings`（配置）`permissions`（权限）`mic_watch`/`log`/`history` |
| `src-tauri/build.rs` | 编译期契约闸：违反 `docs/CONTRACTS.md` 的写法直接 panic（坐标 API 只能在 `overlay.rs` 等） |
| `src/` | `main.ts`（主窗）`hud.ts`（浮窗）`style.css`（设计 token 与组件样式）`pages/*.html`（六个页面片段，经 `?raw` 注入） |
| `index.html` / `hud.html` | **两个前端入口**：vite 的 `rollupOptions.input` 指向它们，构建产物供主窗与浮窗两个 WebView 加载 |
| `vite.config.ts` | 前端开发服务器（:1420，与 `tauri.conf.json` 的 `devUrl` 对应）与构建配置（双入口、target chrome105） |
| `tsconfig.json` / `tsconfig.node.json` | TypeScript 配置（前者含 `src/**`，后者给 `vite.config.ts` 用，经 `references` 关联） |
| `docs/` | 全部文档（README 除外）。`SPEC.md` = 需求与架构；`CONTRACTS.md` = 跨边界契约；索引见下 |
| `scripts/regression.sh` | 回归闸门：`cargo test` → 文件回放链路 → 前端检查 → UI 数据链 → e2e 真麦 → 打包签名 |
| `tests/` | `smoke.mjs`（node 冒烟）、`frontend-checks.mjs`（产物/源码静态检查）、`fixtures/`（回归用音频，TTS 合成，不含真人录音） |

## 命令

```bash
cd src-tauri && cargo test --lib        # 单元测试
cargo check                             # 编译检查（build.rs 契约闸会同步执行）
npm run dev                             # 前端开发服务器(:1420)；配合 npm run tauri dev 使用
npm run tauri build                     # 打包（ad-hoc 签名）→ 再用 rcodesign 覆盖签名
./scripts/regression.sh                 # 全链路回归（改交付/识别/录音后跑）
```

签名与权限：见 `docs/CODE-SIGNING.md`（含 p12 生成、rcodesign 用法、TCC 要求与实测证据）。

## 铁律（违反会出线上问题，均有历史教训）

1. **改完必须自测**：`cargo test --lib` + 装机实测；日志在
   `~/Library/Application Support/io.github.yangyaofei.denpajack/app.log`（`[diag]`/`[voice]`/`[ui]`/`[audio]`/`[perm]` 前缀）。
2. **坐标/窗口 API 只能写在 `overlay.rs`**：`build.rs` 会检查，越界直接编译失败（`docs/CONTRACTS.md` 坐标系节）。
3. **会话级状态禁止新增 static**：一律走 `SessionHandoff`（`docs/CONTRACTS.md` 生命周期节）。
4. **配置新增字段必须 `#[serde(default)]`**：否则旧配置读入即清空用户数据。
5. **前端与后端同步 = 事件 + 轮询兜底**：不要新增"只发事件"的同步路径（`docs/SYNC.md`）。
6. **权限只有两项**：麦克风 + 辅助功能；**不需要"输入监控"**（键盘层是 handy-keys，权限门槛=辅助功能）。
7. **数据目录随 bundle id**：`io.github.yangyaofei.denpajack`（改 id 会重置权限，`src-tauri/src/settings.rs` 的 `APP_ID` 是单一来源）。
8. **不要用脚本批量替换源码**：逐处 `edit`，改完看编译输出与日志；`tmp/` 下的本地脚本不参与构建。
9. **提交信息写清"问题 / 发现过程 / 方案 / 实现 / 总结"**：本项目问题多来自真机行为，只写 "fix bug" 会丢失上下文。

## 文档索引

| 文档 | 用途 |
|---|---|
| `docs/SPEC.md` | 需求与架构（功能清单、模块职责、数据流）—— 改架构先改它 |
| `docs/CONTRACTS.md` | 跨边界契约（线程模型、坐标系、生命周期），由 `build.rs` 强制 |
| `docs/CODE-SIGNING.md` | 签名与权限（rcodesign、TCC、实测证据、坑） |
| `docs/TESTING.md` / `docs/TEST-MATRIX.md` | 测试组织 / "每次修复补一条用例"矩阵 |
| `docs/SYNC.md` | 前端内容与后端状态的同步时机 |
| `docs/HANDY-COMPAT.md` / `docs/DEVIATION-AUDIT.md` | 与参考实现 Handy 的逐组件对照 / 有意偏离 |
| `docs/FEATURE-BACKLOG.md` | 待办与明确的非目标（含理由） |
| `docs/SPIKE-STREAMING.md` | 流式 ASR 方案调研（本地与云端，含实测） |
| `docs/WINDOW_SEMANTICS.md` | 窗口层级与焦点语义 |
| `docs/icon/README.md` | 图标资产、生成脚本与决策历史 |
| `docs/REFACTOR.md` | 优雅性清账清单（对齐 Handy 骨架） |
