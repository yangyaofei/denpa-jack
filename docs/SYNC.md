# 前后端同步机制（审计后定版）

## 唯一通道（C43 实测定案）
- **事件通道（Rust emit→前端 listen）在本机打包版不可达**（emit 返回 Ok 但零到达，Any/AnyLabel 同）——已从 main.ts/hud.ts 全量移除（17 个死监听），代码里不再有双路径
- **invoke 轮询 = 唯一同步机制**（invoke 双向已证可靠）

## 数据项×更新时机×通道（全表）
| 前端数据 | 后端源 | 更新时机 | 通道 | 状态 |
|---|---|---|---|---|
| 浮窗状态/partial/level/err/finished | HudSnapshot(hud_set) | 录音/转写/交付各阶段 hud_set+version+1 | hud_poll 轂询 150ms | ✅ C47 幔等全量重绘（空=清） |
| 浮窗清空时机 | hud_hide 清快照 | 关窗瞬间 | 同上+幂等重绘 | ✅ C47 |
| 主窗历史列表 | recent() | append 时 HISTORY_VER+1 | poll_versions 轮询 800ms | ✅ C46 |
| 主窗顶栏状态(引擎/麦/润色) | get_config | save_config 时 CONFIG_VER+1 | 同上 loadCfg | ✅ C46 |
| 统计卡(总条数/润色/粘贴) | 派生自 history | refreshHistory 后重算 | 同列表 | ✅ |
| 热词预览 | budget_hotwords(dict) | cfg 变化时 | loadCfg 内联 refreshHotwords | ✅ 本轮补 |
| 历史详情(selHist) | 列表项快照 | 列表刷新时按 ts 重找 | refreshHistory 内联 | ✅ 本轮补 |
| 设置各页(cfg 镜像) | get_config | 同顶栏(同一实例) | poll_versions | ✅ |
| 当前麦克风名 | get_active_mic | 切换时+loadCfg | invoke | ⚠️ 热插拔不自动（backlog, 需设备监听） |
| 重试/重转结果 | rerun_history | 重跑走 pipeline→asr-final→轮询 | 轮询 | ✅ |

## 防复发
- frontend-checks.mjs: 断言产物含轮询+无死 listen 残留+组件方法无模块顶层裸调
- index.html 双 script（main.ts 双执行=双轮询双监听）已删——新增页面引用必须唯一

## C51b 热键方案对比(闪电说 strings 逆向实锤)
- 闪电说: 纯 CGEventTap 自写(keyboard_macos_configurable.rs)+辅助功能权限+tap 超时自愈(DISABLED by timeout→re-enabled)+legacy dual hotkey mode
- 我们: global-shortcut(Carbon, 免辅助功能)+flags 40ms 轮询(纯修饰/Fn)
- 判定: 方案同构, 各有优劣——它事件驱动延迟低但强依赖辅助功能+有 tap 超时坑(已自愈); 我们零权限零 tap 沦理, 延迟≤40ms 无感。暂不换; 若轮询实测漏帧再升级 CGEventTap
