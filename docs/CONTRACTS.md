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
