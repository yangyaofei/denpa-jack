# 窗口语义对照表（抄全清单——根治"来回反复"）
来源: Handy crates/handy/src/macos/overlay.rs:7 FloatingPanel + Swift 版 FloatingPanel.swift
| Handy/Swift 语义 | 作用 | Tauri 实现 | 状态 |
|---|---|---|---|
| NSPanel + .nonactivatingPanel | 浮窗永不成为 key window, 不激活 app | set_focusable(false) (overlay.rs show_hud) | ✅ |
| .borderless | 无标题栏 | decorations(false) (lib.rs 创建) | ✅ |
| .canJoinAllSpaces + .fullScreenAuxiliary | 全屏 App 上浮窗可见 | set_visible_on_all_workspaces(true) | ✅ |
| level = .statusBar | 浮在最上 | always_on_top(true) | ✅ |
| 不进窗口列表 | 不出现在 Mission Control | skip_taskbar(true) | ✅ |
规则: 移植组件时先列语义清单再实现; 新 bug 先查对照表是否漏语义, 不打补丁。
