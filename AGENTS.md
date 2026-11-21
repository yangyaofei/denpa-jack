# AGENTS.md — Denpa Jack

未来道具14号机：电波劫持者（Denpa Jack / 未来ガジェット14号機：電波ジャック）。
macOS 菜单栏语音输入：**按住热键说话 → ASR → 词典纠错 →（可选）LLM 润色 → 自动写入当前光标位置**。

> 文档原则：架构、跨边界契约、同步机制、依赖补丁与诊断规则**一律随源码写在包/模块顶部注释中**；
> 改任何模块前先读其顶部注释。外部只保留 [README.md](README.md)（用户入口）、本文（协作契约）与 [docs/CI-CD.md](docs/CI-CD.md)（密钥/签名/CI/发版运维）。

## 1. 模块地图（详细契约见各文件顶部注释）

| 路径 | 职责与核心注释所在 |
|---|---|
| `src-tauri/src/lib.rs` | 架构总览（四域+基座+装配壳）、调用律、门面命名律、`send_ctrl`/`send_cancel`/`send_autostop` |
| `src-tauri/build.rs` | 编译期强制闸（坐标系 / HUD 单一入口 / 主线程 AppKit / 四域门面白名单）+ `tauri_build::build()` |
| `src-tauri/src/core.rs` | 全局单锁状态中枢（唯一事实源：段队列、版本计数、事件与通知槽） |
| `src-tauri/src/config/` | 档案制配置（新增字段必须 `#[serde(default)]`）、`paths.rs`（`APP_ID` 单一来源）、`migrate.rs` |
| `src-tauri/src/crash_log.rs` | 进程死亡诊断（stderr 落盘 / panic / 信号 / `.session.json` / `.ips` / 心跳 / 静默退出判读） |
| `src-tauri/src/capture/` | 采集与感知：`key`（handy-keys 热键与 Esc）、`recording`、`audio`/`feedback`/`mic_watch`、`screen`/`active_target`、`focus`、`permissions` |
| `src-tauri/src/pipeline/` | 流程编排：`segment_queue`（FIFO 串行队列）、`mod`（`post_process` 结算）、`delivery/`（AX 直写 / Cmd+V 粘贴）、`history`、`rerun` |
| `src-tauri/src/model/` | 模型能力：`speech/`（`contract`/`doubao`/`zhipu`/`local`/`gateway`/`driver`）、`dict`、`llm` |
| `src-tauri/src/ui/` | 展示：`hud`（状态机+事件泵）、`overlay`（NS 全链路坐标与五项窗口语义）、`tray`/`tray_events`、`update` |
| `src/` + `index.html` + `hud.html` | 主窗（Overlay 顶栏 + 六页片段 `src/pages/*.html` + 按页域拆分 `src/app/*.ts`，前后端同步契约见 `src/app/common.ts`）与录音浮窗（`src/hud.ts`） |
| `tests/` + `scripts/` | `tests/frontend-checks.mjs`（构建/源码/六处版本号静态断言）、`scripts/regression.sh`（本地全链路回归）、`scripts/package-updater.sh` |
| `docs/CI-CD.md` + `docs/icon/` | 外部运维（签名/TCC/Secrets/CI/Release）与图标母版资产（生成说明见 `docs/icon/bake_app_icon.py`） |

## 2. 常用命令

```bash
# 前端门禁（类型检查 + 构建 + 静态断言）
npx tsc --noEmit && npm run build && node tests/frontend-checks.mjs

# Rust 门禁（单测 + 编译与 build.rs 契约闸 + 诊断探针）
cd src-tauri && cargo test --lib && cargo check && cargo check --examples

# 本地打包 + rcodesign 签名
npm run tauri build -- --bundles app
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"

# 全链路回归（改交付/识别/录音后跑）
./scripts/regression.sh

# 自测通道（autotest.rs，结果看数据目录 app.log）
cd src-tauri && VOICEMAC_AUTOTEST_FILE="../tests/fixtures/sample_short_16k.wav" cargo run
cd src-tauri && VOICEMAC_AUTOTEST=ui cargo run    # 需先启动 npm run dev (:1420)
```

## 3. 铁律

1. **改完必须自测**：`npx tsc --noEmit` + `npm run build` + `node tests/frontend-checks.mjs` + `cargo test --lib` + 装机验证；日志在 `~/Library/Application Support/io.github.yangyaofei.denpajack/app.log`。
2. **坐标/窗口 API 只准写在 `ui/overlay.rs`**：`build.rs` 编译期强制拦截。
3. **会话级状态禁止新增 `static`**：交接一律经 `SessionHandoff`，活状态统一进 `core`。
4. **配置新增字段必须 `#[serde(default)]`**：防止旧 `config.json` 反序列化清空用户数据。
5. **前后端同步以轮询为主路径**：打包版事件通道不可靠，禁止新增"只发事件"的同步路径（见 `src/app/common.ts`）。
6. **必需权限只有两项**：麦克风 + 辅助功能（屏幕录制为可选，**不需要输入监控**）。
7. **数据目录随 `APP_ID`**：取路径一律经 `config::data_dir(app)`（`data_dir` 变更重启生效，`config.json` 固定在默认目录作锚点）。
8. **禁止脚本批量替换源码**：逐处编辑并核查编译输出与测试；`certs/` 与 `tmp/` 永不入库。
9. **修 bug 先查历史回归**：先用 `git log` / `git diff` 判断是否回归，禁止在原逻辑上叠第二套绕过逻辑。
10. **提交信息结构**：写清「问题 / 发现过程 / 实现方案 / 实现过程 / 总结」；发版版本号六处一致（`frontend-checks.mjs` 断言）。
