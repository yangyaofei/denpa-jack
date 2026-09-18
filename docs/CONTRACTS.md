# 跨边界契约（FFI / 框架抽象层）

> 来源：C27（物理/逻辑坐标混算）、C41（NS-y-up 与 CG-y-down 混用）、C26/C39（主线程 API 在回调里直调）。
> 规则：**任何跨边界 API，先确认契约再拼装**。类型系统无法区分两个 f64 的坐标系差异——契约只能靠读文档+实测。

## 坐标系（macOS 三套，互不兼容）
| 体系 | 原点 | y 方向 | 谁在用 |
|---|---|---|---|
| NS（AppKit） | 主屏**左下** | **向上** | NSEvent.mouseLocation / NSScreen.frame / NSWindow.setFrameOrigin |
| CG（CoreGraphics） | 主屏**左上** | **向下** | CGDisplayBounds / CGEvent 位置 / tauri monitor.position() |
| Tauri logical/physical | 封装 CG | 向下 | set_position / monitor.position（**勿与 NS 混算**） |

规则：一次计算只用一套体系，从头到尾。当前坐标操作**集中在 overlay.rs**（全 NS 链路）——新增坐标代码要么进 overlay.rs，要么注明所用体系。

## HUD 单一入口（状态与显隐的唯一所有者）
| 角色 | 允许做什么 | 文件 |
|---|---|---|
| 所有者 | 持有 HUD 状态机（阶段/回显/音量/提示/存活规则）、决定何时显示与隐藏、组装前端快照 | `src/hud.rs` |
| 窗口机制 | 定位、显隐、改尺寸（坐标写入的唯一落点） | `src/overlay.rs` |
| 其他所有模块 | 只能调用 `hud::` 的接口报事实：`recording_started` / `recording_ended` / `partial` / `level` / `polishing` / `notify` / `delivered` / `error` / `hide` / `resize` / `poll` | 其余文件 |

规则：
- 不许再出现共享可变状态（`pub static HUD`、`hud_set` 这类"谁都能改"的入口已删除）；快照只在 `hud::poll()` 里整份拷给前端。
- 不许其他模块直接 show/hide/移动浮窗，也不许自己发 hud 事件（事件通道在打包版不可达，唯一读通道是 `poll()`）。
- 由 `build.rs` 编译期强制：`hud_set` / `HudSnapshot` / `show_hud(` / `hide_hud(` / `show_hud_msg(` 只允许出现在 `hud.rs` 与 `overlay.rs`。

## 线程模型
- NSWindow/NSStatusItem/NSPasteboard/NSSound：**仅主线程**。经 `run_on_main_thread`（参考 tray.rs/paste_tx.rs/overlay.rs 同一模型）
- tauri 窗口方法（show/hide/set_position）：线程安全（内部 dispatch）✓ 可任意线程
- AX API / CGEvent 构造：任意线程 ✓
- 全局快捷键回调：主线程，**只发信号**（C26），禁止回调内部再调用需主线程同步的 API
- tokio：引擎会话运行在专用 current_thread runtime（C35），勿提交到 tauri::async_runtime

## 生命周期
- objc2 的 Retained 自动释放；裸 CGEvent/CF 类型必须 CFRelease
- 录音会话附属物（定时器/循环/桥线程）一律持 `Recording.cancel` 令牌，会话 Drop 时随之作废
- 交接数据只经 `SessionHandoff` 流动，禁止新会话级 static

## 单位
- WAV/PCM 小端：显式 to_le_bytes / from_le_bytes
- 音频链固定 16k mono i16le；采集端下混+重采样只在 audio.rs::ingest
- 时间：tokio Duration 显式 from_secs/from_millis（类型查得住）

## 恰好一次契约
- 引擎结算：Result|Error 恰好一次，公共 `engines::settle`，断连/解析失败/panic 都不许静默
