# 优雅性整改清单（对齐 Handy 骨架）

> 标准：每一行代码都应是优雅、自然的。
> 来源：两轮审计（闭包性审计 38 条 + Handy 对照 32 条）合并去重。
> 状态：☐ 待修 / ◐ 部分完成 / ✅ 已修 / ⏸ 延后（需用户确认）

## P1 结构（会话级状态归位）
- ✅1 FALLBACK_TX/LAST_AUDIO 两个会话级 static → AppState 字段
- ✅2 CURRENT_ENGINE 全局 String 删（引擎名已随 SessionHandoff 流动，交叉覆盖竞态 #10）
- ✅3 ProcessingFinished 无用信号：发送方与协调器变体全删（或启用）
- ✅4 StartResult 回滚补齐：效果线程执行 Start 后回发真实结果，失败复位协调器（麦克风被拒场景）

## P2 缺机制
- ✅5 引擎结算 settle 在 doubao/openai_realtime 重复 → 公共 settle（契约只写一次）
- ◐6 收尾超时统一常量：openai 已用 `FINALIZE_TIMEOUT_SECS`（30s）；doubao 仍硬编码 3s（见 docs/SPEC.md 已知不一致）
- ✅7 麦克风解析无缓存：每次按下全枚举（+40~110ms 按键延迟）→ 缓存上次 uid，miss 才枚举
- ✅8 ctrl_stop 同步写盘阻塞效果线程 → save_wav/prune 移到交付线程（pcm 随 handoff 流动）
- ✅9 转写失败不入历史 → Error 分支也 append（重试链闭合）
- ✅10 rerun_history 在主线程同步重放（阻塞 UI）→ 后台线程

## P3 收尾打磨
- ✅11 托盘无"转写中"态 → 三态图标（默认/红=录音/黄=转写）
- ✅12 日志不脱敏 → release 构建截断转写内容（#[cfg(debug_assertions)]）
- ✅13 尾音缓冲 extra_tail_ms（默认 0，cancel-aware）
- ⏸14 首样本就绪通知（蓝牙设备提示音早于采录数百 ms，低影响）
- ⏸15 录音时静音系统输出（功能项，等用户确认要做）

## 审计遗留（细）
- ✅16 doubao 错误帧 JSON 解析失败静默 break → emit Error
- ✅17 restore_clipboard 两处默认值相反（serde default=false / 读取 fallback=true）→ 统一 default_true
- ✅18 settings 无用字段组删除（shortcut_activation/hold_threshold_ms/filler_word_removal/append_trailing_space 等无代码读取的字段）
- ✅19 WAV 44B 头拼装在 history.rs 与 zhipu_file.rs 重复 → 公共函数
- ◐20 编译 warning 清零（现 40 个：unused import/dead code/never read）

## 回归要求
每批修完：cargo check 0 error → dict_test 5/5 → autotest file 交付 ✓ → autotest e2e 按键链 ✓ → 打包签名 ALIVE
