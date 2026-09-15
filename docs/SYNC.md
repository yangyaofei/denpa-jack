# 前后端同步机制

## 同步方式（C43 实测确认，后续有补充）

- 事件通道（Rust `emit` → 前端 `listen`）在本机打包版**不可靠**：`emit` 返回 `Ok` 但前端收不到（`Any` / `AnyLabel` 都试过），所以状态同步的主路径是 invoke 轮询（invoke 双向已实测可靠）。
- 事件只保留少数一次性推送：`config-changed`（配置保存后）、`permissions`（权限检查结果）。其余状态一律走轮询。
- 历史上移除的 17 个监听都是 ASR 过程事件（`asr-partial` / `asr-result` / `asr-error` …），它们现在全部由轮询替代。

## 数据项×更新时机×通道（全表）

| 前端数据 | 后端源 | 更新时机 | 通道 | 状态 |
|---|---|---|---|---|
| 浮窗状态/partial/level/err/finished | HudSnapshot(hud_set) | 录音/转写/交付各阶段 hud_set+version+1 | hud_poll 轮询 150ms | ✅ C47 幂等全量重绘（空=清） |
| 浮窗清空时机 | hud_hide 清快照 | 关窗瞬间 | 同上+幂等重绘 | ✅ C47 |
| 主窗历史列表 | recent() | append 时 HISTORY_VER+1 | poll_versions 轮询 800ms | ✅ C46 |
| 主窗顶栏状态(引擎/麦/润色) | get_config | save_config 时 CONFIG_VER+1 | 同上 loadCfg | ✅ C46 |
| 统计卡(总条数/润色/粘贴) | 派生自 history | refreshHistory 后重算 | 同列表 | ✅ |
| 热词预览 | budget_hotwords(dict) | cfg 变化时 | loadCfg 内联 refreshHotwords | ✅ |
| 历史详情(selHist) | 列表项快照 | 列表刷新时按 ts 重找 | refreshHistory 内联 | ✅ |
| 设置各页(cfg 镜像) | get_config | 同顶栏(同一实例) | poll_versions | ✅ |
| 当前麦克风名 | get_active_mic / list_mics | 设备插拔(CoreAudio 监听) + 切换设备 | poll_versions.mics 800ms | ✅ |
| 权限状态 | check_permissions | 启动检查 + 手动"重新检测" | invoke + `permissions` 事件 | ✅ |
| 重试/重转结果 | rerun_history | 重跑走 pipeline → 历史刷新 | 轮询 | ✅ |

## 防复发

- `tests/frontend-checks.mjs`：断言产物含轮询、组件方法不在模块顶层裸调、页面引用唯一。
- `index.html` 曾重复引用 `main.ts`（双执行 = 双轮询双监听），已删除——新增页面引用必须唯一。

## 键盘方案现状（C51b 对比 → C54 改版）

- 现在：键盘层用 handy-keys（Handy 同款，内部是 CGEventTap），权限门槛 = **辅助功能**；Fn 与各种组合键统一走它，录音期间的 esc 由 global-shortcut（Carbon）动态注册。
- 对比对象的做法（闪电说）：自写 CGEventTap + 辅助功能 + tap 被系统禁用后自动重启用。
- 历史实现（已删除）：自研 Carbon 热键 + flags 40ms 轮询，那版不需要辅助功能、也不依赖事件监听；C54 改版后整个删除。
