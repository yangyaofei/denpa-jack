# Handy 实现方式偏差审计（用户指令: 逐功能对照 Handy 实现方式, 找偏离→防问题）
审计方法: subagent 逐组件读两边源码; 基线=docs/HANDY-COMPAT.md(已解决不重复)。

## 高风险
1. [粘贴] clipboard_only=true 时 pipeline.rs:221 直接 return "copied" **从未写剪贴板**(Handy clipboard.rs:74 必先写文本) → 只复制模式下转写文本不会写入任何位置
2. [录音→引擎] 桥线程有界 64 通道 try_send 失败即 break(recording.rs:281), Finish 也 try_send 静默丢(recording.rs:354)(Handy transcription.rs:169 无界阻塞 send 保序) → 背压导致音频丢失、文本末尾被截断且无告警 / Finish 丢→会话挂死
3. [paste_tx] flush_pending 经 run_on_main_thread 异步派发(paste_tx.rs:218), Handy macos.rs:186 同步结算后才快照 → 快照可能读到上一条 lazy promise, 恢复写回错误旧内容

## 中风险
4. [取消] 转写中(post_process 已启动) Esc 无取消标记(recording.rs:374 仅 session 存在时写)——Handy actions.rs:818 贴前多点校验 → 文本仍会贴出
5. [收尾] FINALIZE_TIMEOUT 3s(engines.rs:24), Handy 30s(transcription.rs:40) → 慢网下末尾文字丢失
6. [失败音频] Error 路径不落盘 PCM+audio_path 读 last_audio 与异步 save_wav 竞态(engines.rs:87)(Handy history.rs:219 先存后更新) → 失败会话音频丢失，或音频挂到其它会话
7. [历史并发] enforce_limit 读改写与 append 无锁(history.rs:37), Handy SQLite 串行化(history.rs:195) → 重叠交付时记录丢失

## 低风险
8. cpal err_fn 仅 eprintln(audio.rs:107), Handy 有 stream_error+needs_reopen 自愈(recorder.rs:456)
9. 采样格式仅 F32/I16(audio.rs:134), Handy 支持 U8/I8/I16/I32/F32
10. 极简协调器无防抖(键抖动极快连按多启停; macOS auto-repeat 已阻止, 残余风险低)
11. 主线程调度失败兜底 paste_cmd_v 未先写文本(pipeline.rs:233); hud.ts partial 监听注册两次(49,110 诊断残留); 托盘曾设计成随录音染红(`set_recording(true)` 无调用)——2026-09-18 用户裁决改为图标恒定不变，相关调用点与调度函数已删（见 SPEC §3.7）

## Top 5 修复(风险×成本)
1→5 全部本轮修: ①clipboard_only 补写剪贴板 ②桥线程阻塞 send+Finish 失败 emit Error ③flush_pending 同步化 ④Esc 转写中写 CANCELLED_GEN ⑤超时 3s→30s
