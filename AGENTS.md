# AGENTS.md — Denpa Jack

未来道具14号机：电波劫持者（Denpa Jack / 未来ガジェット14号機：電波ジャック）。

macOS 菜单栏语音输入：**按住热键说话 → ASR → 词典纠错 →（可选）LLM 润色 → 自动写入当前光标位置**。
本文件是这个仓库的契约（给在这里工作的人 / agent 看）。用户级工作规则另见 `~/.config/opencode/AGENTS.md`。

## 一句话架构

Rust 后端（`src-tauri/src/`）按**四域**组织：`ui`（展示）/ `capture`（采集+决策）/ `pipeline`（流程编排）/ `model`（模型能力），加基座（`core` 单锁状态中枢+版本计数、`config`、`util`、`log`、`crash_log`）与装配壳（`lib`/`commands`/`autotest`/`transcription_coordinator`）。
跨域只走两条通道：**门面符号**（意图动词，build.rs 门面闸强制白名单）与 **core**（反向通知=写状态+版本计数，消费方轮询自取）。
数据流：热键 → 协调器 → `capture::recording`（采集+会话）→ `model::speech`（ASR）→ `pipeline::segment_queue`（串行化→词典纠错→可选 LLM→交付 `pipeline::delivery`）；浮窗提示经 `core::hud_emit` 事件泵，浮窗显隐走 `ui` 门面。
结构契约的权威文档 = `docs/ARCHITECTURE.md`（四域树/门面清单/命名律/调用律）；`SPEC.md` 记行为事实。

## 目录

| 路径 | 内容 |
|---|---|
| `src-tauri/src/` | 四域+基座+装配壳（2026-11 结构重构，见 `docs/ARCHITECTURE.md`）：**基座** `core`（全局状态单锁+版本计数+段投影+HUD 事件槽）`config/`（主文件+`paths`+`migrate`）`util`（跨域纯函数，如 wav 头）`log`/`crash_log`；**capture/** `key`（热键三规则+Esc 监听）`recording`（会话状态机）`audio`/`feedback`/`mic_watch`（麦克风）`screen`（截图/窗口）`active_target`（活动目标）`focus`（前台/焦点感知）`permissions`；**pipeline/** `mod`（结算总装 post_process）`segment_queue`（分段队列）`rerun`（历史重转编排）`delivery/`（`ax` 直写/`paste` 回执粘贴/键击）`history`；**model/** `speech/`（`contract`+`doubao`/`zhipu`/`local` 三引擎+`gateway` 本地网关+`driver` WS 骨架+`transcribe_session` 驱动器）`dict`（词典纠错/热词预算）`llm`（refine 纠错管线/自测）；**ui/** `hud`（浮窗状态机+事件泵）`overlay`（窗口/坐标）`tray`/`tray_events`/`update`；**装配壳** `lib`（效果分派+send_ctrl）`commands` `autotest` `transcription_coordinator` |
| `src-tauri/build.rs` | 编译期契约闸（**递归**扫描）：坐标 API 只能在 `overlay.rs`、hud 快照 API 白名单、主线程 API 必须有调度标记、**门面闸**（跨域 use 必须命中 FACADE 白名单，豁免表已清零）——违反直接 panic（`docs/CONTRACTS.md` + `docs/ARCHITECTURE.md`） |
| `src/` | `main.ts`（主窗入口：组合+启动，44 行）`app/`（主窗组件按页拆分：`types` AppThis 形状 / `util` / `common` 横切 / `asr`/`llm`/`dict`/`history`/`general`/`about` 六页域）`hud.ts`（浮窗）`style.css`（设计 token 与组件样式）`pages/*.html`（六个页面片段，经 `?raw` 注入） |
| `index.html` / `hud.html` | **两个前端入口**：vite 的 `rollupOptions.input` 指向它们，构建产物供主窗与浮窗两个 WebView 加载 |
| `vite.config.ts` | 前端开发服务器（:1420，与 `tauri.conf.json` 的 `devUrl` 对应）与构建配置（双入口、target chrome105） |
| `tsconfig.json` / `tsconfig.node.json` | TypeScript 配置（前者含 `src/**`，后者给 `vite.config.ts` 用，经 `references` 关联） |
| `docs/` | 全部文档（README 除外）。`ARCHITECTURE.md` = 结构契约（四域/门面/闸）；`SPEC.md` = 行为事实；`CONTRACTS.md` = 跨边界技术契约；索引见下 |
| `handover/` | 交接文档（产品要求/改动流程/验证体系/发布运维/协作纪律/教训——零上下文入门用） |
| `REQUIREMENTS.md` | 2026-10 全库审核转化出的需求清单与 D 编号决议记录 |
| `scripts/regression.sh` | 回归闸门：`cargo test` → 文件回放链路 → 前端检查 → UI 数据链 → e2e 真麦 → 打包签名 |
| `tests/` | `smoke.mjs`（node 冒烟）、`frontend-checks.mjs`（产物/源码静态检查，**断言与源码路径强耦合——搬文件必须同步**）、`fixtures/`（回归用音频，TTS 合成，不含真人录音） |
| `certs/` | 签名资产与密钥（`.p12` / 密码 / `secrets.env`）——**gitignored，不入库**；CI 里由 repository secrets 注入，见 `docs/CI-CD.md` |
| `.github/workflows/` | `ci.yml`（测试）、`release.yml`（打 tag 即构建+签名+发 Release） |

## 命令

```bash
cd src-tauri && cargo test --lib        # 单元测试
cargo check                             # 编译检查（build.rs 契约闸会同步执行）
npm run dev                             # 前端开发服务器(:1420)；配合 npm run tauri dev 使用
npm run tauri build                     # 打包（ad-hoc 签名）→ 再用 rcodesign 覆盖签名
./scripts/regression.sh                 # 全链路回归（改交付/识别/录音后跑）

# 自测通道（autotest.rs；改动相关链路后按需跑, 结果看数据目录 app.log 而非 stdout）
cd src-tauri && VOICEMAC_AUTOTEST=file VOICEMAC_AUTOTEST_FILES="a.wav,b.wav" cargo run   # 文件回放: 喂 wav→引擎→交付全链
cd src-tauri && VOICEMAC_AUTOTEST=timelimit VOICEMAC_AUTOTEST_MAX_SECS=3 cargo run       # 真麦超长截断+哑火回归(PASS 4/4 看日志)
cd src-tauri && npm run dev &   # ui 通道前置: debug 构建走 devUrl, 无 dev server 时 WebView 空白=假绿!
VOICEMAC_AUTOTEST=ui cargo run  # 主窗数据链 vs 磁盘真值([ui-chain] PASS 看日志)
VOICEMAC_DATA_DIR=./tmpdata VOICEMAC_AUTOTEST=file ...   # 隔离数据目录(不脏真实历史)

# CI/CD：推 v* tag 即在 GitHub Actions 上构建+签名+发 Release（见 docs/CI-CD.md）
git tag v0.2.0 && git push origin v0.2.0
```

签名与权限：见 `docs/CODE-SIGNING.md`（含 p12 生成、rcodesign 用法、TCC 要求与实测证据）；
CI 需要的仓库凭据（Secrets）与发版流程见 `docs/CI-CD.md`。

## 铁律（违反会在实际使用中出问题，均有历史教训）

1. **改完必须自测**：`cargo test --lib` + 装机实测；日志在
   `~/Library/Application Support/io.github.yangyaofei.denpajack/app.log`（`[diag]`/`[voice]`/`[ui]`/`[audio]`/`[perm]` 前缀）。
2. **坐标/窗口 API 只能写在 `overlay.rs`**：`build.rs` 会检查，越界直接编译失败（`docs/CONTRACTS.md` 坐标系节）。
3. **会话级状态禁止新增 static**：一律走 `SessionHandoff`（`docs/CONTRACTS.md` 生命周期节）。
4. **配置新增字段必须 `#[serde(default)]`**：否则旧配置读入即清空用户数据。
5. **前端与后端同步 = 事件 + 轮询兜底**：不要新增"只发事件"的同步路径（`docs/SYNC.md`）。
6. **权限只有两项**：麦克风 + 辅助功能；**不需要"输入监控"**（键盘层是 handy-keys，权限门槛=辅助功能）。
7. **数据目录随 bundle id**：默认 `~/Library/Application Support/io.github.yangyaofei.denpajack`
   （改 id 会重置权限，`src-tauri/src/config/paths.rs` 的 `APP_ID` 是单一来源）；
   数据根目录可用配置 `data_dir` 改（history / recordings / llm_logs / app.log 都跟着走，
   `config.json` 固定在默认位置当锚点）。取路径一律走 `config::data_dir(app)`
   （启动解析一次恒定——**改 data_dir 不中途切换，重启生效**，前端保存时会提示），
   不要自己拼 `app_data_dir()`。
8. **不要用脚本批量替换源码**：逐处 `edit`，改完看编译输出与日志；`tmp/` 下的本地脚本不参与构建。
9. **提交信息写清"问题 / 发现过程 / 方案 / 实现 / 总结"**：本项目问题多来自真机行为，只写 "fix bug" 会丢失上下文。
10. **修 bug 先查历史行为（回归优先）**：动手前先用 `git log -S<符号>` / `git diff <已知正常版本>..HEAD -- <文件>`
    确认"这个行为以前有没有"。以前**没有** → 是最近改动引入的回归，去改引入它的那段逻辑、回到原来的做法；
    以前**有** → 才是新问题，单独修。禁止在原逻辑上再叠一层新实现绕过（一层问题叠成两套并存逻辑，
    现象不再能唯一归因，下次更难查）。

## 编号约定（文档与代码注释里会出现）

| 编号 | 含义 | 说明 |
|---|---|---|
| `C<n>` | 变更 / 教训编号 | 按时间递增；含义在出现处的上下文里写明（如 "C39 已实测确认的崩溃"） |
| `B<n>` | 缺陷编号 | 同上 |
| `L1`–`L7` | 证据分级 | `L1` 本机实测 / `L2` 自跑原型 / `L3` 源码分析 / `L4` 官方文档 / `L5` 社区反馈 / `L6` 博客教程 / `L7` 直觉；关键结论要求 `L1`–`L3` |

## 文档索引

| 文档 | 用途 |
|---|---|
| `docs/SPEC.md` | 需求与架构（功能清单、模块职责、数据流）—— 改架构先改它 |
| `docs/CONTRACTS.md` | 跨边界契约（线程模型、坐标系、生命周期），由 `build.rs` 强制 |
| `docs/CODE-SIGNING.md` | 签名与权限（rcodesign、TCC、实测证据、常见问题） |
| `docs/DIAGNOSTICS.md` | 崩溃与静默退出诊断（日志能覆盖什么、事后怎么判读、自测通道） |
| `docs/TESTING.md` / `docs/TEST-MATRIX.md` | 测试组织 / "每次修复补一条用例"矩阵 |
| `docs/SYNC.md` | 前端内容与后端状态的同步时机 |
| `docs/HANDY-COMPAT.md` / `docs/DEVIATION-AUDIT.md` | 与参考实现 Handy 的逐组件对照 / 有意偏离 |
| `docs/FEATURE-BACKLOG.md` | 待办与明确的非目标（含理由） |
| `docs/SPIKE-STREAMING.md` | 流式 ASR 方案调研（本地与云端，含实测） |
| `docs/WINDOW-SEMANTICS.md` | 窗口层级与焦点语义 |
| `docs/icon/README.md` | 图标资产、生成脚本与决策历史 |
| `docs/REFACTOR.md` | 优雅性清理清单（对齐 Handy 骨架） |
