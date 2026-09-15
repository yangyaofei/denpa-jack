# Handy 组件语义对照表（抄全清单 v2）
来源: Handy 源码（github.com/cjpais/Handy，30.8k★，Tauri 2 同类；对照时用的本地 clone 已随调研目录清理，需要时重新 clone）
规则: 移植组件先列语义清单再实现; 出 bug 先查本表漏了哪条语义, 不打补丁。

## 1. 浮窗 (hud)
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| tauri-nspanel v2.1 (ahkohd/tauri-nspanel) PanelBuilder | 把 Tauri Window 转原生 NSPanel | Tauri WebviewWindow API | 结构不同 |
| StyleMask::empty().borderless().nonactivating_panel() (overlay.rs:447) | 永不成为 key window/不激活 app | set_focusable(false) (等效 key 语义) | ✅ |
| PanelLevel::Status (:438) | 浮在最上 | always_on_top(true) | ✅ |
| CollectionBehavior canJoinAllSpaces+fullScreenAuxiliary | 全屏 App 上可见 | set_visible_on_all_workspaces(true) | ✅ |
| — | — | 未换 tauri-nspanel: backlog(若 set_focusable 在某 macOS 版本失效则换插件) | 备用 |

## 2. Rust→前端事件（C43 根因区）
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| emit_to("recording_overlay", "mic-level") (overlay.rs:744) + 注释: Tauri 2 的 emit 广播+listener filter 机制 (738-743) | 定向 emit 在 Handy 可用 | emit_to("hud") **实测不达**(Any/AnyLabel 均不达, emit 返回 Ok) | ❌→轮询绕过 |
| — | — | **轮询替代**: HudSnapshot+hud_poll() 150ms 拉取(lib.rs/commands.rs/hud.ts) | ✅ 已验证 |
| backlog: 事件通道真因(疑 capabilities/core:event 粒度或 api/cargo 版本组合) | — | 待深挖, 轮询保底 | 🔍 |

## 3. 托盘
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| Tauri TrayIcon + recreate_tray_icon() hide/re-show (tray.rs:614, tauri#12060 NSStatusItem 静默消失恢复) | 消失后可自愈 | 手写 objc2 NSStatusItem(tray.rs)——**结构上绕开该 bug**; 用户报过"状态栏不在"(多屏=内屏规则, 非消失 bug) | ✅ |

## 4. 热键
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| Carbon RegisterEventHotKey+kEventHotKeyPressed/Released (早前验证, 与 tauri-apps/global-hotkey 同源) | 按下/抬起独立事件 | tauri-plugin-global-shortcut(底层同 Carbon) + CTRL_TX 信号线程(C13/C26: 回调只发信号) | ✅ |

## 5. 粘贴 (paste_tx)
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| macos.rs 336 行全套: 懒承诺声明/回执轮询/QUIET_PERIOD 200ms/RESTORE 8s/FAILED 500ms/Concealment markers/settle 恰一次 | 粘贴可靠性 | paste_tx.rs 整套移植+审计一致 | ✅ |

## 6. 键盘注入
| Handy 实现 | 语义 | 我们的实现 | 状态 |
|---|---|---|---|
| CGEvent 直发(非 enigo keycode_to_string——C39 崩溃根因), chord hold 100ms (#165) | 权限稳定/线程安全 | deliver.rs cg_cmd_key CGEventCreateKeyboardEvent+CGEventPost+CFRelease | ✅ |
