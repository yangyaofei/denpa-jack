# core 结构定稿（v1，待确认）

> 本文是 `core`（应用核心状态）的数据结构定稿。确认后按此实现；实现分三步搬迁（文末），每步系统完整可用。
> 配套讨论：状态清单与 ★/○/✗ 归置已在前轮确认（会话/段/交付/设备/网关/权限/更新/tope 进 core；配置/历史门面可查；崩溃诊断/日志/自测/前端表单态不进）。

## 一、定位与红线

- **core 是所有活状态的家 + 静态数据的门面**。执行模块只连 core；core 只连纯数据模块（settings/history）。
- 依赖方向一条直线：`执行模块 → core → settings/history`。core 不 import 任何执行模块，循环依赖结构上不可能。
- **core 里只有数据与投影，没有任何业务**：不录音、不发请求、不弹窗、不睡等待。
- 并发：无线程。`Mutex<Core>` 一把锁，短临界区（字段赋值/拷贝，微秒级）。写者迁移状态并递增版本号；读者拷贝快照。panic/信号处理路径禁止进入 core 锁。
- 锁中毒策略：`lock().unwrap_or_else(|e| e.into_inner())`——core 是纯数据，中毒后取出继续用是安全的（优于全链路 panic）。

## 二、顶层结构

```rust
/// 应用核心状态：唯一事实源。
/// state   = 可 Clone 的展示/逻辑状态（投影只读这段）
/// resources = 活资源句柄（会话/引擎通道等非 Clone 物，随会话生灭）
/// 一把锁同时保两段，保证"是否在录"这类判断只看 core 一处。
pub struct Core {
    pub state: CoreState,
    pub resources: Resources,
}

pub struct CoreState {
    pub version: u64,            // 全局版本号：任一迁移 +1（驱动浮窗刷新）
    pub session: SessionState,   // 会话域
    pub queue: QueueState,       // 段队列域
    pub deliver_tx: Option<DeliverTxState>, // 交付事务（全局单槽瞬态）
    pub devices: DevicesState,   // 设备域
    pub gateway: GatewayState,   // 内置网关域
    pub perms: PermsState,       // 权限域
    pub update: UpdateState,     // 更新域
    pub toast: Option<Toast>,    // 一次性消息
    pub ctrl: CtrlState,         // 控制域（触发模式镜像/热键录制态/版本计数）
}

pub struct Resources {
    pub session: Option<SessionHandle>, // 当前录音会话（含引擎命令通道、cancel 令牌）
}
```

## 三、各域子结构

```rust
// ── 会话域 ──────────────────────────────────────────
pub struct SessionState {
    pub recording: bool,          // 是否在录（唯一权威，协调器不再自持布尔）
    pub started_at_ms: u64,
    pub gen: u64,                 // 会话代际号（取消校验用）
    pub engine: String,           // 档案名
    pub mic: String,              // 设备名
    pub max_secs: u64,            // 本段时长上限
    pub level: f32,               // 电平（120ms 刷新）
    pub partial: String,          // 实时回显
    pub silence_warned: bool,     // 静音提醒已触发
    pub tail_buffering: bool,     // 尾音缓冲期
    pub screenshot: Option<String>, // 截图路径（会话期暂存，交接后归段）
}

// ── 段队列域 ──────────────────────────────────────────
pub struct QueueState {
    pub next_id: u64,
    pub segments: Vec<SegmentEntry>, // 按处理顺序；完成/取消的段短暂保留后离场
    pub worker_busy: bool,
}

pub struct SegmentEntry {
    pub id: u64,
    pub state: SegState,
    pub ts: String,               // 时间戳
    pub engine: String,
    pub audio_path: Option<String>,
    pub raw: Option<String>,      // ASR 原文
    pub final_text: Option<String>, // 纠正稿
    pub delivered: Option<String>,  // 交付方式（ax直写/粘贴/仅复制/焦点变化/失败/取消）
    pub llm_used: bool,
    pub failure: Option<String>,  // 失败原因
}

pub enum SegState { Queued, Transcribing, Polishing, Delivering, Done, Failed, Cancelled }

// ── 交付域（瞬态单槽）────────────────────────────────
pub struct DeliverTxState {
    pub started_at_ms: u64,       // 注入时刻（回执过滤起点）
    pub saved_text: Option<String>, // 用户原剪贴板快照
    pub gen: u64,                 // 归属段 id
}

// ── 设备域 ──────────────────────────────────────────
pub struct DevicesState {
    pub mic_list_version: u64,    // 热插拔计数
    pub resolved_mic: Option<String>, // 已解析当前麦克风（缓存）
}

// ── 网关域 ──────────────────────────────────────────
pub struct GatewayState {
    pub running: bool,
    pub port: u16,
    pub healthy_ever: bool,
    pub started_at_ms: Option<u64>,
    pub log_tail: String,
}

// ── 权限域 ──────────────────────────────────────────
pub struct PermsState {
    pub mic: PermValue,           // granted/denied/undetermined
    pub ax: bool,
    pub screen: Option<bool>,     // None=未探测
}

// ── 更新域 ──────────────────────────────────────────
pub struct UpdateState {
    pub phase: UpdatePhase,       // Idle/Checking/Downloading(f32)/Ready(err?)/Installing
    pub latest_version: Option<String>,
}

// ── 消息 ────────────────────────────────────────────
pub struct Toast {
    pub kind: ToastKind,          // Info/Warn/Error
    pub text: String,
    pub at_ms: u64,               // 出现时刻（投影层算消失）
}

// ── 控制域 ──────────────────────────────────────────
pub struct CtrlState {
    pub activation: String,       // hold/toggle 运行态镜像
    pub hotkey_capturing: bool,   // 热键录制进行中
    pub config_version: u64,      // 主窗 800ms 轮询用
    pub history_version: u64,
}
```

## 四、写接口：Event 全量枚举（单入口）

执行者不直接改字段，一律 `core::apply(&app, event)`。写入即迁移 + 版本号 +1。

```rust
pub enum Event {
    // 会话域
    SessionStarted { engine: String, mic: String, max_secs: u64 },
    SessionLevel(f32),
    SessionPartial(String),
    SessionSilenceWarned,
    SessionTailBuffering(bool),
    SessionScreenshot(Option<String>),
    SessionEnded,                  // 数据已交接给段，会话资源释放
    SessionAborted,

    // 段域
    SegEnqueued { ts: String, engine: String, audio_path: Option<String> },
    SegResult { id: u64, raw: String },
    SegPolishing { id: u64 },
    SegDelivered { id: u64, final_text: String, delivered: String, llm_used: bool },
    SegFailed { id: u64, reason: String },
    SegRetried { id: u64 },        // 失败段重新入队（回 Queued）
    SegDismissed { id: u64 },      // 用户关闭失败段
    QueueCleared,                  // 排队段全部转 Cancelled（先留痕后离场）

    // 交付域
    PasteTxStarted { gen: u64, saved_text: Option<String> },
    PasteTxSettled,                // 事务结束（恢复或保留）

    // 设备域
    MicListChanged,                // 热插拔：版本 +1
    MicResolved(String),

    // 网关域
    GatewayStarted { port: u16 },
    GatewayStopped,
    GatewayHealth { healthy: bool },
    GatewayLog(String),            // 追加日志尾部

    // 权限域
    PermsChanged { mic: PermValue, ax: bool, screen: Option<bool> },

    // 更新域
    UpdatePhase(UpdatePhase),
    UpdateLatest(Option<String>),

    // 消息
    Toast { kind: ToastKind, text: String },

    // 控制域
    ActivationSet(String),
    HotkeyCapturing(bool),

    // 版本计数（由 settings/history 的写路径回调）
    ConfigChanged,
    HistoryChanged,
}
```

## 五、读接口：投影函数（纯函数，状态→视图）

| 投影 | 消费者 | 说明 |
|---|---|---|
| `hud_snapshot(&Core, now_ms) -> HudSnapshot` | 浮窗（150ms 轮询） | 含全部展示数据 + **建议收起时刻**（350ms/2.5s 规则全在此计算，前端照做） |
| `tray_status(&Core) -> TrayStatus` | 托盘状态行 | 录音中/待命/转写中 |
| `main_status(&Core) -> MainStatus` | 主窗 | 试音按钮态、录音标记 |
| `poll_versions(&Core) -> Versions` | 主窗 800ms 轮询 | config/history/mic 三计数 |
| `get_config(app) -> Config` | 任何模块 | **门面**：转发 settings（会话开始时另拍快照随链路传） |
| `get_history(app) -> Vec<HistoryRecord>` | 主窗/重跑 | **门面**：转发 history |

投影规则：只读 `CoreState`，不碰 `Resources`；不产生副作用；时间规则（收起倒计时）以 `now_ms` 入参计算，写进快照。

## 六、版本号策略

- 全局 `version`：任一 Event 迁移 +1 → 浮窗轮询对比即知有无新内容。
- `config_version` / `history_version` / `mic_list_version`：独立计数，主窗按需比对（沿用现前端 800ms 机制，前端无需改）。

## 七、不进 core 的（重申）

- 配置/历史**内容**：磁盘文件是家，core 只做门面与版本计数。
- 崩溃诊断（会话标记/心跳）、日志设施、自测通道：独立设施。
- 前端表单输入中间态：前端自己的，提交（保存配置）才进 core 视野。
- 浮窗窗口的显示/隐藏/位置/宽度：窗口操作归 overlay（执行），其"应该显示吗"由投影给出。

## 八、实施：三步搬迁（每步完整可验证）

> 进度：**第 1 步已完成**（core 空壳 + 段队列搬入 + HUD 队列区投影 + 漏刷修复）。
> **第 2 步已完成**（2a 会话展示态 + 2b B5 消息化/哑火修复 + 2c 托盘状态行与主窗按钮投影）。
> **第 3 步（修订后范围）已完成**：版本计数收口（config/history/mic 三计数住 core，poll_versions 从 core 一次读齐）+ 配置/历史门面函数。
>
> 第 3 步**修订**（相对初版清单，经实现验证后的归置修正）：
> - **toast/一次性消息不进 core**——它是瞬时事件不是持续状态（消费即消失），留在展示域（hud.rs）；"谁都能发提示"的统一事件入口在归位批做。
> - **网关/权限/更新状态暂不搬**——各自单一消费者（网关卡/权限卡/关于页），搬迁收益低改动面大；留待有第二个消费者时再收。
> - **交付事务（paste_tx 单槽）暂不搬**——单消费者且逻辑自洽，归位批统一处理。
> - MIC_CACHE（麦克风解析缓存）是执行性能缓存而非状态，留在 recording 域。
>
> **core 计划至此闭环**：会话/段队列/版本计数三类核心事实住 core，浮窗、托盘、主窗、自测全部读同一事实源。

| 步 | 搬入 | 改造读者 | 验证 |
|---|---|---|---|
| 1 | QueueState + SegEvent 族 + 交付事务 | HUD 队列区改投影（hud_poll 读 hud_snapshot） | cargo test / smoke / 实机：多段录音、重试、清空、浮窗刷新 |
| 2 | SessionState 全部 + Resources | HUD 主区、托盘状态行、主窗试音按钮、协调器记忆（recording 标志改读 core） | 实机：录/停/Esc/超长截断/静音提醒，托盘与按钮跟随 |
| 3 | toast/设备/网关/权限/更新/控制域 + 门面函数 | poll_versions 收口、settings/history 写路径挂版本回调 | 实机：换配置即时生效路径、网关卡、权限横幅 |

每步之后：`cargo test --lib` + `tests/smoke.mjs` + 实机走查（按 docs/TEST-MATRIX.md 对应项）；停在任何一步系统均完整可用。

## 九、开放问题（实现时定，不阻塞结构）

- Event 枚举与分域方法两种写法的取舍：定稿用枚举（单入口、可日志、未来可回放）。
- `SegmentEntry` 完成后的离场时机：当前设计"Done 后立即离队，仅最后一段在 HUD 短暂展示"——保持现状语义。
- 前端 hud.ts 的收起排程逻辑在步骤 2 移除（规则已在投影里）。
