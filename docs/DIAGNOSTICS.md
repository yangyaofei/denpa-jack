# 崩溃与静默退出诊断

应用偶尔会无声关闭（进程消失、界面消失，但没有任何提示）。本文说明日志覆盖了什么、事后怎么定位、以及怎么复现验证诊断链路本身。

背景：早期版本只记录业务链路（录音、转写、交付），进程死亡不经过这些代码，因此日志会直接中断在最后一条正常操作上。系统侧通常也查不到可用报告 —— 只有拿到进程信号（如 SIGABRT）时 macOS 才会生成 `.ips`，而 ObjC/CoreAudio 抛异常、外部结束进程等情况不会。

## 日志位置

| 内容 | 位置 |
|---|---|
| 主日志 | `<数据目录>/app.log`（默认 `~/Library/Application Support/io.github.yangyaofei.denpajack/app.log`） |
| 会话状态 | `<数据目录>/.session.json`（进程存活时存在；正常退出时删除） |
| 已上报的崩溃报告名单 | `<数据目录>/.crash_reports_seen.json` |
| 系统崩溃报告 | `~/Library/Logs/DiagnosticReports/denpa-jack-*.ips` |

数据目录可在设置页「数据」卡里改（改后日志与历史一起搬过去）。

## 日志里与死亡相关的行

| 前缀 | 含义 |
|---|---|
| `[app] 会话开始 pid=…` | 每次启动写一行 |
| `[app] 正常退出` | 走正常退出路径（托盘退出、⌘Q、退出事件）时写一行，同时删除 `.session.json` |
| `[crash] 上次会话未标记正常退出(疑似崩溃或被强制结束): pid=… 启动时刻=… 距本次启动=… 上次日志最后写入=…` | 本次启动发现上次没有留下正常退出标记；`上次日志最后写入` 约等于上次进程仍然活着的最后时刻 |
| `[crash] panic thread=… at <文件:行:列>: <消息>` + 后续 `[crash]   <回溯行>` | Rust panic，含线程名、源码位置与最多 40 行回溯 |
| `[crash] killed by signal N (SIGxxx)` | 收到致命信号（SIGSEGV/SIGBUS/SIGABRT/SIGILL/SIGFPE/SIGTRAP）。写完这行后交还系统默认处理，因此仍会生成 `.ips` |
| `[crash] 发现系统崩溃报告 <文件名>` + `[crash]   "exception": …` | 启动时扫描到本应用的新 `.ips`，并把关键字段搬进日志 |
| `[hb] uptime=…s rss=…MB threads=…` | 每分钟一次心跳 |
| `[ctx] 截图已采集 path=… bytes=… ms=…` | 屏幕上下文开关开启时，每次录音开始采集一张截图 |
| `[ctx] 截图跳过: 未授予屏幕录制权限…` / `[ctx] 截图失败, 本次不带图纠错: …` | 采集未成功，本次退回纯文本纠错（不阻塞交付） |

## 怎么判读一次静默退出

```bash
LOG=~/Library/Application\ Support/io.github.yangyaofei.denpajack/app.log
grep -nE "\[crash\]|\[app\] 正常退出|会话开始|\[hb\]" "$LOG" | tail -30
```

- 有 `killed by signal`：按信号定性。`SIGSEGV/SIGBUS` 多为内存访问错误（含 C 库/ObjC/CoreAudio 层），`SIGABRT` 多为断言或异常终止。接着去 `~/Library/Logs/DiagnosticReports` 看同名 `.ips` 的 `exception` 与 `termination`，以及线程栈。
- 有 `panic thread=…`：直接看位置与回溯。栈顶几行通常就是出问题的函数。
- 既无信号也无 panic，且心跳在某时刻断开：更可能是被外部结束（`pkill`、系统回收、崩溃前被替换安装包）或进程在日志缓冲之外的位置退出。此时对比 `会话开始` 与最后一条 `[hb]` 的时间差，即可得到大致存活时长。
- 有 `正常退出`：不是崩溃，是正常退出路径（注意托盘菜单的「退出」）。
- 什么都没查到：说明死亡发生在日志系统之外（例如进程启动阶段极早期），用下一节的自测通道确认诊断链路是否仍然有效。

## 自测通道（复现验证诊断链路）

```bash
open -a "/Applications/Denpa Jack.app" --env VOICEMAC_AUTOTEST=panic   # 后台线程 panic → 应出现 [crash] panic + 回溯
open -a "/Applications/Denpa Jack.app" --env VOICEMAC_AUTOTEST=abort   # 自杀 SIGABRT → 应出现 killed by signal 6，并生成 .ips
open -a "/Applications/Denpa Jack.app" --env VOICEMAC_AUTOTEST=quit    # 走正常退出 → 应出现 [app] 正常退出且 .session.json 被删除
```

验证闭环：跑一次 `abort`，等进程退出后再正常启动一次，日志里应出现「上次会话未标记正常退出」与「发现系统崩溃报告」两段 —— 这两段同时出现，说明从"进程死亡"到"下次启动诊断"的链路是通的。

## 实现位置

`src-tauri/src/crash_log.rs`：stderr 落盘、panic hook、信号处理、会话文件、崩溃报告扫描、心跳、自测通道。安装顺序在 `src-tauri/src/main.rs`（先于 Tauri 初始化，覆盖启动阶段的崩溃）；会话开始与心跳在 `src-tauri/src/lib.rs` 的 setup 中；正常退出标记在 `RunEvent::Exit`。
