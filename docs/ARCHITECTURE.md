# ARCHITECTURE.md — 四域结构与门面契约

> 本文是结构重构（2026-11 起）的权威契约：目录怎么分、门面怎么设、怎么被 build.rs 强制。
> 与 `CONTRACTS.md`（跨边界技术契约：线程/坐标系/生命周期）互补。
> 阅读顺序：AGENTS.md（工作规则）→ 本文（结构契约）→ SPEC.md（行为事实）。

## 0. 一句话

四个能力域 + 根基座：**ui（展示）· capture（采集+决策）· pipeline（流程编排）· model（模型能力）**。
流程是串线检查工具，不是目录顺序——每个概念按"能力"归域，用产品流程验证不漏人。

```
src-tauri/src/
│  main.rs  lib.rs  commands.rs  autotest.rs     装配壳（入口+command+自测）
│  core.rs  log.rs  util.rs  crash_log.rs        基座（状态中枢+日志+时间戳+崩溃诊断）
├─ ui/        展示：hud(状态机+overlay 几何) · tray(菜单+事件) · update
├─ capture/   采集+决策：key(热键+三规则) · audio(recording+feedback+mic_watch) ·
│             screen(目标探测+截图+LLM 编码) · focus(前台快照) · permissions(守门)
├─ pipeline/  流程：flow(队列+Live/Rerun 编排) · settle(结算总装) ·
│             deliver(paste/ax/keytap) · history(store+replay)
└─ model/     模型：speech(trait+分派+driver=工程层; doubao/zhipu/local/gateway=接入层) ·
              text(dict→polish→未来 summary/context-inject)
```

串线自检（每个环节在树上指到唯一居所）：

> 按下 → capture/key 决策 → capture/audio 收音(+screen 截图) → pipeline/flow 提交
> → model/speech 识别 → model/text 纠错润色 → pipeline/settle 总装
> → pipeline/deliver 落屏 → pipeline/history 记录 → ui 显示（core 投影）

## 1. 门面律（模块自治契约）

每域一个门面（mod.rs re-export），外部只许走门面符号；域内实现私有，改组不动调用方。

### 命名律

> 门面动词必须是**意图**（transcribe / submit / cancel / snapshot / resolve / refine）；
> **机制词**（spawn / run / enqueue / channel / worker / thread）与**机制类型**（cmd_rx 等通道对象）
> 禁止出现在公共签名里。
> 检验：把签名读给不知道内部实现的人——他懂＝合格；他问"什么是 rx"＝漏了实现。

### 调用律

```
装配层(lib 效果分派 · commands · autotest) → 可调四域任意门面
capture → pipeline → core            顺流程单向
pipeline ⇄ model                     双向经门面(编排调能力: transcribe_session; 能力交结果: submit_live)
ui      → 只读 core 投影              浮窗内容=core 事件+状态(hud 泵消费), 浮窗显隐=同步效果(ui 门面, 分派层与 capture 可调)
反向通知 → 一律走 core（写状态+版本计数，消费方轮询自取）
域内自由 → settle 调 deliver/history 不过门面（同域）
```

### 门面清单（迁移完成后的目标形态）

| 域 | 公共出口 |
|---|---|
| model | `resolve(cfg)` · `transcribe(session, sink)`（session.feed/finish）· `retranscribe(wav)` · `refine(raw,cfg,profile,shot)` · `selftest(profile)` · 类型 `Cmd/AsrEvent/Handoff` |
| capture | `send_ctrl(sig)` 唯一入口 · `esc_cancel_window(open/close)` · `resolve_mic/list_mics` · `focus_snapshot()` · `active_target(cfg)` · `take_screenshot(cfg)` · `perm_state/ensure_permissions` |
| pipeline | `init(app)` 装配专用 · `submit_live(session)` · `submit_rerun(ts,wav)` · `cancel_latest()` · `deliver(text,focus,mode)` · `history: recent/save/update_by_ts/read_all` · 类型 `SegView` |
| ui | `hud: show_partial/show_final/show_delivered/polishing/error/dismiss/notify` · `overlay: show/hide/reposition+DisplayTarget` · `tray: init/status` · `update: check/install` |

明确收回内部的（曾在外、已判内部）：`send_current`（通道工具，外部用 `cancel_current`）、
`enqueue_*`（队列是内部结构，对外是 submit_live/submit_failed/submit_rerun）。
`send_ctrl` 留 lib.rs 根（信号路由是装配层概念，热键/托盘/试音统一从壳进）。
`budget_hotwords` 保留门面（会话构造时显式传参，数据流可见）。

### 门面闸与豁免登记

build.rs 强制跨域引用白名单（FACADE 表）；存量违规走 EXEMPT 登记表，
每条注明批 4 去向（浮窗内容走 core / rerun 编排归位 / esc 自查 / DisplayTarget 归位），
批 4/5 逐条清空。豁免表内容见 `src-tauri/build.rs` 门面闸一节。

## 2. 保障机制（不靠自觉）

1. **门面外 pub 收回**：域内实现模块私有；门面符号在域 mod.rs re-export——外部路径只有一条。
2. **build.rs 门面闸**：跨域 `use crate::他域::` 只许命中门面白名单（机制同坐标 API 闸，越界编译失败）。
3. **core 中介**：反向通知唯一通道是 core（写状态+版本计数）；现有 hud_poll / poll_versions 通道扩用。

## 3. 域内注释规范（每个域 mod.rs 头部必写）

```rust
//! <域名>：一句话能力定义（"从世界拿信号并决策"这类能力语言，不是技术清单）
//!
//! 门面（外部只许调这些）：
//!   - fn_x(…) — 意图说明（不写实现）
//! 内部（域外不可见，改组不影响调用方）：
//!   - 子模块一句话职责
//!
//! 禁止：调用 <他域> 内部符号；持有跨域可变状态（状态一律进 core）
```

## 4. 迁移批次（零行为变化 · 独立 commit · 装机验证）

| 批 | 内容 | 状态 |
|---|---|---|
| 1 | 投影归位 core（views/has_failed/busy 等）· 焦点族搬 capture/focus（首个域目录） | ✅ 34177d4 |
| 2 | 四域目录全部立起：git mv 归位+引用切换+门面 re-export+域注释 | ✅ 5fa1d3b |
| 3 | 门面签名按命名律改造（transcribe/submit_live 意图名 + 收回机制符号）+ build.rs 门面闸 | ✅ f673933 |
| 4 | 清豁免：Esc 自查 / 浮窗提示走 core 泵 / rerun 编排归位 pipeline | ✅ 本批 |
| 5 | 文档对齐：SPEC §架构 · AGENTS.md 目录表 · handover/01 速查；豁免表尾项(wav_header/delivery/gateway)评估 | 待 |

批 1 特例说明：recording/pipeline/engine 内的 hud 直调（同步提示）随批 2 域搬迁时统一改
"内容走 core + 显隐走分派层"，避免同批文件改两遍。

## 5. 历史

- 2026-11-02 用户裁决：四域封顶（"瘦树"）；流程串线不排序；机制词禁入门面；门面=模块自治契约。
- 此前：阶段A（engine/delivery/config 立目录+trait 化）、阶段B（骨架统一/时区化/前端拆页）已完成，
  详见 git log 与 handover/。
