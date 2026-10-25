# 架构：活动目标探测、截图与浮窗

## 一句话

按下热键时**探测一次**"当前正在用的东西"（焦点窗口 → 指针窗口 → 指针所在屏），截图与浮窗共用这一份结果。

## 模块与职责

| 模块 | 职责 | 关键函数 |
|---|---|---|
| `src-tauri/src/active_target.rs` | **唯一的**活动目标探测 | `detect_active_target() -> ActiveTarget` |
| `src-tauri/src/screen_context.rs` | 截图（只做截图）+ 窗口列表两个原子 | `window_at()`、`focused_window()`、`pick_window_at()`（纯规则）、`capture()`、`capture_with_sck()` |
| `src-tauri/src/delivery/` | 交付层（`ax.rs` AX 能力 / `paste.rs` 粘贴事务）；`ax.rs` 附带提供 AX 焦点信息 | `focused_element_info() -> Option<FocusInfo{pid, frame}>`（在 `ax.rs`） |
| `src-tauri/src/overlay.rs` | 光标、显示器、浮窗定位（AppKit） | `cursor_point()`、`display_target_at(point)`、`main_display_target()`、`position_hud(app, Option<&DisplayTarget>)` |
| `src-tauri/src/hud.rs` | 浮窗内容与显隐 | `recording_started(app, &DisplayTarget)` |
| `src-tauri/src/recording.rs` | 接线：一次探测，两处消费 | 见下 |

## 数据流

```
recording.rs（按下热键）
  mode   = parse_target(cfg.detect_active_target)     // off / active_window / active_display
  target = active_target::detect_active_target()      // ActiveTarget { display, window }
        │
        ├─ 截图线程：screen_context::capture_and_prune(dir, "ctx", &target, mode)
        │      capture() → capture_with_sck()          // SCK 枚举 → 过滤器 → 截图 → PNG → 清理旧图
        │
        └─ 浮窗：hud::recording_started(app, &target.display)
               → overlay::position_hud(app, Some(display)) → position_on_main()
```

`ActiveTarget`：

```rust
pub struct ActiveTarget {
    pub display: DisplayTarget,       // 一定有值
    pub window: Option<WindowRef>,    // 焦点/指针下的普通窗口；没有则 None
}
```

## 探测规则（三级）

文件：`src-tauri/src/active_target.rs`

1. **焦点窗口**（键盘光标所在）：`screen_context::focused_window()`
   - AX 系统级焦点元素 → `AXWindow` 的 `AXPosition`/`AXSize` → 在 CoreGraphics 窗口列表里按框匹配（±2px）。
   - 拿不到窗口框（Electron 常见）→ 用焦点元素的 pid，取该应用在 CG 列表里最前面的合格窗口。
   - AX 整体不可用（实测 OpenChamber 返回 `kAXErrorCannotComplete`，-25204）→ 回退"全系统最前面的普通窗口"。
2. **指针窗口**：`window_at(cursor_point())`——指针命中的最前面合格窗口。
3. **指针所在屏**：`display_target_at(cursor_point())`，`window = None`。

合格窗口 = 层 0、非本进程、≥200×150（过滤浮层、自己的窗口、工具提示）。

## 截图

文件：`src-tauri/src/screen_context.rs`

- 实现：ScreenCaptureKit（进程内，macOS 14+）：`SCShareableContent` → `SCContentFilter` → `SCScreenshotManager.captureImage` → ImageIO 写 PNG。
- 窗口模式：`SCContentFilter(desktopIndependentWindow:)`。
- 屏幕模式：`SCContentFilter(display:excludingWindows:)`，排除**本应用自己的全部窗口**（浮窗不入镜）；不设 `NSWindow.sharingType`。
- 超时：枚举 5s、截图 5s；目标窗口不在可截列表时返回错误（只记日志，本次不带图，不阻塞交付）。
- 落盘：`<数据目录>/screen_context/screen-{millis}-ctx.png`，保留最近 200 张。
- 旧实现已删除：`screencapture` 命令行（外部进程不可控，实测最长一次卡 134.6s）。

## 浮窗

文件：`src-tauri/src/overlay.rs`

- `position_hud(app, Some(display))`：把浮窗放到目标显示器上；**屏内位置规则不变**，由配置 `overlay_position` 决定（bottom / top / center）。
- 目标显示器 → `NSScreen`：CG 全局坐标（y 向下，原点=主屏左上）换算成 NS 全局坐标（y 向上，原点=主屏左下），再按 frame 匹配（见 `ns_frame_of` / `screen_rect_for`）。
- 不在主线程时通过 `run_on_main_thread` 派发（AppKit 要求）。

## 坐标系约定（重要）

| 场景 | 坐标系 | 说明 |
|---|---|---|
| 窗口列表、显示器、光标 | CG 全局 | y 向下，原点在主显示器左上；`CGDirectDisplayID` |
| 浮窗定位 | AppKit（NS） | y 向上，原点在主显示器左下；`NSScreen.frame` |
| AX 窗口框 | 同 CG 全局 | `AXPosition`/`AXSize` 与 `CGWindowBounds` 一致（±2px 内可匹配） |

历史上混用两套坐标出过跨屏错位（`overlay.rs` 顶部注释的 C41），新增代码必须显式换算，不要直接把 CG 数值塞进 NS API。

## 依赖关系

- `active_target.rs` 依赖 `screen_context`（窗口）与 `overlay`（光标/显示器）。
- `screen_context.rs` 的 `capture` 依赖 `active_target::ActiveTarget`（只读 target，不再自行挑选）。
- `delivery/ax.rs` 提供 AX 焦点信息，不依赖上述模块。
- 无循环初始化、无全局可变状态；每次探测都是独立快照。
