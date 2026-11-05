//! core —— 应用核心状态(唯一事实源)。设计定稿: docs/CORE-DESIGN.md。
//!
//! # 定位与红线
//! - **所有状态都在这里**(用户定则 2026-11-05: "所有的状态, 所有的")——
//!   会话/段队列/事件/通知/版本计数, 全 App 的活状态只此一份;
//!   展示层(ui/hud)不持有事实, 是 core 的读取者: 各自按快照内容摆放到
//!   对应位置。core 里因此**没有以某个展示端命名的东西**——事件叫
//!   AppEvent 不叫 HudEvent, 通知叫 PendingNotice, 快照叫 snapshot。
//! - 所有活状态的家 + 静态数据的门面。执行模块只连 core;
//!   core 只连纯数据模块(settings/history), 不 import 任何执行模块——依赖一条直线。
//! - core 里只有数据与投影, 没有业务: 不录音/不发请求/不弹窗/不睡眠等待。
//! - **写入必推进 version**(硬约束): 任何被轮询方按 version 去重消费的
//!   通道(投影/事件/通知槽), 写入路径必须 version += 1——否则内容变化
//!   会被去重逻辑静默丢弃(真机回归: 没听清后浮窗永不收窗)。
//! - 无线程: 一把锁短临界区(字段赋值/快照拷贝, 微秒级)。
//!   panic/信号处理路径禁止进入 core 锁。
//!
//! # 搬迁进度(每步系统完整可用, 见 CORE-DESIGN.md 第八节)
//! - [x] 第 1 步: 段队列域(本文件现状)
//! - [x] 第 2 步: 会话域(录音态/电平/在途引擎会话与流式文字)
//! - [ ] 第 3 步: 会话发号/取消集合/激活模式(仍在域 static, 同类问题清单已呈报)
//!
//! # 锁中毒策略
//! `lock()` 在中毒时取出数据继续用: core 是纯数据, 不变量不会被破坏,
//! 优于 `.unwrap()` 的全链路连环 panic(审核 a-11 的结构性解法)。

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

/// 段状态(FIFO 顺序 / 失败重试 / 浮窗展示都基于它)
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
pub enum SegState {
    /// 已入队, 等 worker 取
    Queued,
    /// worker 正在跑这段(同一时刻最多一段); Rerun 任务也用此态(自带会话, worker 跳过)
    Transcribing,
    /// 后处理失败, 保留在队列里等重试
    Failed,
}

/// 段的来源(类型区分, 用户定则): 重试不是来源——是 Failed→Queued 的状态回退。
/// 去向由来源决定: Live→交付管线; Rerun→回写历史条目(不交付不弹 HUD)
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum SegSource {
    /// 首次转换: 热键录音(含失败段重试的重放)
    Live,
    /// 重新转换: 历史页"重转"的后台重算(rerun_ts 定位目标条目)
    Rerun,
}

/// 段实体: 展示态与干活所需的交接上下文是同一段实体的两面, 不拆存——
/// 段的真相只有 core 这一处(segment_queue.rs 是它的执行者, 不再自持队列)。
pub struct SegmentEntry {
    pub id: u64,
    /// 段来源(Live/Rerun)——投影端据此决定展示; 结算端据此决定去向
    pub source: SegSource,
    /// Rerun 段的目标条目定位(其他段 None)
    pub rerun_ts: Option<String>,
    /// ASR 原文; ASR 阶段失败时为空
    pub raw: String,
    /// 会话交接(音频/焦点/代际/截图) —— 重试所需的全部上下文
    pub ho: crate::pipeline::SessionHandoff,
    pub state: SegState,
    /// 失败原因(浮窗提示 + 历史 warning)
    pub reason: Option<String>,
    /// true = ASR 阶段就失败(没有 raw): 重试要重跑识别而不是重跑后处理
    pub asr_failed: bool,
}

// ───────── 段投影（只读视图）─────────
// 投影与数据同居: 谁持有数据谁出视图。消费方(ui/tray/commands/浮窗前端)
// 一律从 core 读, 不越过数据主人去问执行者(segment_queue 是写侧)。

/// 给前端的段视图(浮窗渲染用; 只有 state + text, 不带段号等多余信息——用户定则)
#[derive(Clone, Debug, serde::Serialize)]
pub struct SegView {
    /// 录音中 | transcribing | queued | failed
    pub state: String,
    /// live | rerun —— 段来源(展示端自决怎么显示, 如重转段标 ↻)
    pub source: String,
    pub text: String,
}

/// 浮窗视图: (当前段, 队列其余段)
///
/// 当前段 = 正在转写的那段(录音中的"当前"由浮窗用 HUD 快照里的 partial 表示,
/// 见 commands::hud_poll —— 录音中的段还不属于队列)。
/// cur 位(正在结算的主链路段)只给 Live——Rerun 段虽是 Transcribing 但
/// 自带会话不占交付位, 显示进队列列表(带 rerun 标记, 展示端自决)。
pub fn views() -> (Option<SegView>, Vec<SegView>) {
    let c = lock();
    views_of(&c)
}

/// HUD 原子投影(2026-11-05 两步演进: ①修撕裂——曾两次独立读 core; ②按用户指正
/// **删除 ui 侧快照镜像层**, poll 直读 core)。一次锁内取齐持久投影+版本。
/// 事件队列不走这里(唯一消费者是 ui 的事件泵: drain 后应用展示规则写一次性槽,
/// poll 合并槽——两条通道各司其职, 互不相偷)。
pub fn snapshot() -> CoreSnapshot {
    let mut c = lock();
    let (mut cur, mut queue) = views_of(&c);
    // 引擎在途但尚未入队的段(Result 未到): 以虚拟"转写中"视图补进投影——
    // 最新一个兜底 cur 位(松手→Result 之间左栏不断档), 其余进队列列表。
    // 已入队的段(views_of 已含)跳过, 按 gen 去重。
    let queued: Vec<u64> = c.queue.items.iter().map(|s| s.ho.gen).collect();
    let inflight: Vec<u64> = c.session.engine_gens.iter().filter(|g| !queued.contains(g)).copied().collect();
    let partial_of = |c: &CoreState, g: u64| {
        c.session.engine_partials.iter().find(|(pg, _)| *pg == g).map(|(_, t)| t.clone()).unwrap_or_default()
    };
    if cur.is_none() {
        if let Some(latest) = inflight.last() {
            let g = *latest;
            cur = Some(SegView { state: "transcribing".into(), source: "live".into(), text: partial_of(&c, g) });
        }
    }
    for g in inflight.iter().rev().skip(1) {
        queue.push(SegView { state: "transcribing".into(), source: "live".into(), text: partial_of(&c, *g) });
    }
    // 一次性通知槽: 取走即清(前端拿到即消费; 写入侧 notice_put 已 bump version,
    // 去重丢弃的安全窗口由"写入必 bump"保证)
    let oneshot = std::mem::take(&mut c.pending_notice);
    CoreSnapshot {
        version: c.version,
        recording: c.session.recording,
        level: c.session.level,
        partial: c.session.engine_gens.last().map(|g| partial_of(&c, *g)).unwrap_or_default(),
        cur,
        queue,
        failed: c.queue.items.iter().any(|s| s.state == SegState::Failed),
        oneshot,
    }
}

/// snapshot 的载荷(poll 直读 core 的持久投影+一次性通知)
pub struct CoreSnapshot {
    pub version: u64,
    pub recording: bool,
    pub level: u32,
    pub partial: String,
    pub cur: Option<SegView>,
    pub queue: Vec<SegView>,
    pub failed: bool,
    pub oneshot: PendingNotice,
}

/// views 的无锁内核(调用方持锁; views/hud_projection 共用)
fn views_of(c: &CoreState) -> (Option<SegView>, Vec<SegView>) {
    let src = |s: &SegmentEntry| if s.source == SegSource::Rerun { "rerun".to_string() } else { "live".to_string() };
    let cur = c
        .queue
        .items
        .iter()
        .find(|s| s.state == SegState::Transcribing && s.source == SegSource::Live)
        .map(|s| SegView { state: "transcribing".into(), source: src(s), text: s.raw.clone() });
    let queue: Vec<SegView> = c
        .queue
        .items
        .iter()
        .filter(|s| !(s.state == SegState::Transcribing && s.source == SegSource::Live))
        .map(|s| SegView {
            state: match s.state {
                SegState::Failed => "failed".into(),
                SegState::Transcribing => "transcribing".into(),
                SegState::Queued => "queued".into(),
            },
            source: src(s),
            text: s.raw.clone(),
        })
        .collect();
    (cur, queue)
}

/// 队列里是否还有活(排队或转写中)——浮窗存活规则与托盘都用它
pub fn busy() -> bool {
    let c = lock();
    c.queue.items.iter().any(|s| s.state == SegState::Queued || s.state == SegState::Transcribing)
}

/// 队列里是否有未处理的失败段
pub fn has_failed() -> bool {
    let c = lock();
    c.queue.items.iter().any(|s| s.state == SegState::Failed)
}

/// 队列里失败段中最早的一个 id(浮窗"重试"按钮用)
pub fn first_failed_id() -> Option<u64> {
    let c = lock();
    c.queue.items.iter().find(|s| s.state == SegState::Failed).map(|s| s.id)
}

/// 段队列域状态
#[derive(Default)]
pub struct QueueState {
    /// 段按处理顺序; 交付成功即离场, 失败段保留待重试
    pub items: VecDeque<SegmentEntry>,
    pub next_id: u64,
}

/// 会话域状态(第 2a 步搬入): "现在是否在录、录的内容"——全 App 最核心的事实,
/// 从此只有 core 一处(HUD 只是它的投影)。
/// 注: 阶段展示语义(transcribing/busy 等展示态)仍归 hud.rs 的展示状态机,
/// 那是"事实的投影规则", 不是事实本身。
/// 流式文字(partial)不在会话上, 在各在途引擎会话上(engine_partials 按 gen 归属)——
/// 旧单槽"会话 partial"时代, 上一段松手后的流式文字会写进下一段录音的槽(串台,
/// 2026-11-05 用户报"队列里第二三条的实时内容不对"的根源)。
pub struct SessionState {
    /// 是否正在录音(按下→松手之间)
    pub recording: bool,
    /// 音量(RMS 0-1000 定点, 录音中 120ms 刷新)
    pub level: u32,
    /// 在途引擎会话代集合(松手→引擎出 Result 之间的窗口)。
    /// Esc 收窗判据=busy||recording||本集合非空——段要等 Result 才入队,
    /// 只看 busy/recording 会在松手后误关 Esc 窗(批4 真机回归, R2 二犯)。
    /// 存 gen 而非计数: Esc 的取消目标之一就是"引擎在途但还没入队"的段
    /// (ctrl_cancel_queued 先查这里再查队列, 取消最新)。
    pub engine_gens: Vec<u64>,
    /// 各在途段的流式识别文字(按 gen; 引擎 Partial 事件写入, 段终态随
    /// engine_pending_remove 一并清除)。HUD 左栏显示最新的, 各段视图显示自己的。
    pub engine_partials: Vec<(u64, String)>,
}

/// 登记一个在途引擎会话(ctrl_stop 交接箱写入后)
pub fn engine_pending_add(gen: u64) {
    if gen == 0 {
        return; // 0=不参与取消的会话(rerun 构造)
    }
    let mut c = lock();
    c.session.engine_gens.push(gen);
}

/// 注销一个在途引擎会话(段终态入队/NoSpeech/失败/引擎死——任一收尾路径)。
/// 流式文字一并清除(段终态后 partial 不再有意义)。
pub fn engine_pending_remove(gen: u64) {
    let mut c = lock();
    c.session.engine_gens.retain(|g| *g != gen);
    c.session.engine_partials.retain(|(g, _)| *g != gen);
}

/// 是否还有在途引擎会话(Esc 收窗三判据之一)
pub fn engine_pending() -> bool {
    let c = lock();
    !c.session.engine_gens.is_empty()
}

/// 最新一个在途引擎会话代(Esc 取消目标优先级最高的段: 比"已入队"的都新)
pub fn engine_pending_latest() -> Option<u64> {
    let c = lock();
    c.session.engine_gens.last().copied()
}

/// 在途段的流式识别文字(引擎 Partial 事件桥写入; gen=0 忽略——rerun 不进 HUD)
pub fn engine_partial(gen: u64, text: &str) {
    if gen == 0 {
        return;
    }
    let mut c = lock();
    match c.session.engine_partials.iter_mut().find(|(g, _)| *g == gen) {
        Some(slot) => slot.1 = text.to_string(),
        None => c.session.engine_partials.push((gen, text.to_string())),
    }
}

// ───────── 测试辅助(仅单测可见; 与 TEST_SESSION_LOCK 配套防会话态用例互踩)─────────
#[cfg(test)]
pub fn test_reset() {
    let mut c = lock();
    c.session = SessionState { recording: false, level: 0, engine_gens: Vec::new(), engine_partials: Vec::new() };
    c.queue = QueueState::default();
}

/// 造一个最小在途段入队(取消链用例用; gen 写进 ho)
#[cfg(test)]
pub fn test_enqueue(state: SegState, gen: u64) {
    let mut c = lock();
    let id = c.queue.next_id;
    c.queue.next_id += 1;
    c.queue.items.push_back(SegmentEntry {
        id,
        source: SegSource::Live,
        rerun_ts: None,
        raw: String::new(),
        ho: crate::pipeline::SessionHandoff { gen, ..Default::default() },
        state,
        reason: None,
        asr_failed: false,
    });
    c.version += 1;
}

/// 控制域: 轮询版本计数(第 3 步收口)——"数据变了"的信号统一住 core,
/// poll_versions 从 core 一次读齐。
pub struct CtrlState {
    pub config_version: u64,
    pub history_version: u64,
    pub mic_list_version: u64,
}

/// 核心状态: 全部可投影的数据(投影函数只读这里)
pub struct CoreState {
    /// 全局版本号: 任一状态迁移 +1, 轮询方据此判断"有没有新东西"
    pub version: u64,
    /// 段队列域(第 1 步搬入)
    pub queue: QueueState,
    /// 会话域(第 2a 步搬入)
    pub session: SessionState,
    /// 控制域: 版本计数(第 3 步收口)
    pub ctrl: CtrlState,
    /// 浮窗事件槽(批4): capture/model/pipeline 不再直调 ui——提示类事件进这里,
    /// hud 自己的泵线程 drain 后分派(延迟≤100ms, 提示类无感)。cap 32 防御性截断。
    pub hud_events: VecDeque<AppEvent>,
    /// 一次性通知槽(泵写入 → 前端 poll 取走): msg/err/finished 是"待消费的事件
    /// 结果", 不是状态镜像——持久数据(partial/level/段列表)一律投影直读。
    /// 住 core(2026-11-05 从 ui static 迁入): 写入必 bump version——前端以
    /// version 去重快照, 槽写入若不 bump, 提示会被去重丢弃(真机: 没听清后
    /// 浮窗停在"转写中"永不收窗)。
    pub pending_notice: PendingNotice,
}

/// 一次性通知槽的载荷(见 CoreState.pending_notice 注释)
#[derive(Default)]
pub struct PendingNotice {
    pub msg: String,
    pub finished: Option<serde_json::Value>,
    pub err: Option<String>,
    pub polishing: bool,
}

/// 通知槽写入(唯一入口, 泵调用; 展示保护规则在泵的 apply_event 里判 recording)。
/// 每次(批)写入 bump version——"槽内容变了"就是投影变了。
pub fn notice_put(f: impl FnOnce(&mut PendingNotice)) {
    let mut c = lock();
    f(&mut c.pending_notice);
    c.version += 1;
}

/// 浮窗提示事件(批4 反向通知走 core 的第一批)。
/// 显隐类(recording_started/ended/hide)仍是同步效果经 ui 门面直调——
/// 浮窗出现/收起不能滞后, 提示内容变化 100ms 无感, 两类分治。
pub enum AppEvent {
    Notify(String),
    Error(String),
    NoSpeech(String),
    /// 交付完成(payload 结构与 hud::delivered 原参数一致)
    Delivered(serde_json::Value),
    Polishing,
}

/// 发一条浮窗提示事件(capture/model/pipeline 调——方向合法: 一律写 core)
pub fn emit(e: AppEvent) {
    let mut c = lock();
    if c.hud_events.len() >= 32 {
        c.hud_events.pop_front();
    }
    c.hud_events.push_back(e);
    c.version += 1;
}

/// hud 泵线程专用: 取走全部积压事件
pub fn drain_events() -> Vec<AppEvent> {
    let mut c = lock();
    c.hud_events.drain(..).collect()
}

static CORE: Mutex<CoreState> = Mutex::new(CoreState {
    version: 0,
    queue: QueueState { items: VecDeque::new(), next_id: 1 },
    session: SessionState { recording: false, level: 0, engine_gens: Vec::new(), engine_partials: Vec::new() },
    ctrl: CtrlState { config_version: 0, history_version: 0, mic_list_version: 0 },
    hud_events: VecDeque::new(),
    pending_notice: PendingNotice {
        msg: String::new(),
        finished: None,
        err: None,
        polishing: false,
    },
});

/// 取 core 锁(中毒时取出数据继续——见模块头"锁中毒策略")
pub fn lock() -> MutexGuard<'static, CoreState> {
    CORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 跨模块碰 core.session 的测试互斥锁(测试并行时 session 是单例字段,
/// 生命周期测试与投影测试同时动它会闪断)
#[cfg(test)]
pub static TEST_SESSION_LOCK: Mutex<()> = Mutex::new(());

/// 当前全局版本号(读一次, 微秒级)
pub fn version() -> u64 {
    lock().version
}

// ── 会话域写接口(执行者迁移事实的唯一入口) ────────────────────────

/// 一条新录音开始: 事实置位 + 清上一条的流数据。
/// (engine_partials 不清——旧段在途的流式文字仍要在队列视图里显示, 终态时各自清)
pub fn session_started() {
    let mut c = lock();
    c.session.recording = true;
    c.session.level = 0;
    c.version += 1;
}

/// 这一条录完了(松手): 事实复位。partial 不清——结果还没到,
/// 清空回显就是用户看到的"瞬间消失再出现"(hud.rs 同名规则的事实版)。
pub fn session_ended() {
    let mut c = lock();
    c.session.recording = false;
    c.version += 1;
}

/// 音量(RMS 0-1000 定点)
pub fn session_level(lv: u32) {
    let mut c = lock();
    c.session.level = lv;
    c.version += 1;
}

/// 当前是否在录(展示层判定用的事实源)
pub fn session_recording() -> bool {
    lock().session.recording
}

// ── 版本计数(第 3 步收口: settings/history/mic_watch 的写路径调 bump, 读走各自转发) ──

pub fn config_version() -> u64 {
    lock().ctrl.config_version
}
pub fn bump_config() {
    lock().ctrl.config_version += 1;
}
pub fn history_version() -> u64 {
    lock().ctrl.history_version
}
pub fn bump_history() {
    lock().ctrl.history_version += 1;
}
/// R1: 进行中的重转 ts 列表——从队列投影(source=Rerun 的在途段), 不再另持集合。
/// 单一事实源: 重转任务入队/出队本身就是这个"集合"的增删
pub fn rerun_active() -> Vec<String> {
    lock().queue.items.iter().filter(|s| s.source == SegSource::Rerun).filter_map(|s| s.rerun_ts.clone()).collect()
}
pub fn mic_list_version() -> u64 {
    lock().ctrl.mic_list_version
}
pub fn bump_mic_list() {
    lock().ctrl.mic_list_version += 1;
}

// ── 门面: 静态数据经 core 可查(转发给纯数据模块; core 不 import 执行模块) ──

/// 配置快照(门面: 转发 settings; 调用方不需要知道 settings 的存在)
pub fn get_config(app: &tauri::AppHandle) -> Result<crate::config::Config, String> {
    crate::config::get_config(app.clone())
}

/// 最近历史(门面: 转发 history)
pub fn get_history(app: &tauri::AppHandle, limit: usize) -> Vec<crate::pipeline::history::HistoryRecord> {
    crate::pipeline::history::recent(app, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 读取某 gen 的流式文字(测试辅助, 同 snapshot 内部闭包)
    fn partial_of(c: &CoreState, g: u64) -> String {
        c.session.engine_partials.iter().find(|(pg, _)| *pg == g).map(|(_, t)| t.clone()).unwrap_or_default()
    }

    /// 中毒策略: 取出数据继续, 不 panic
    #[test]
    fn poisoned_lock_yields_data() {
        let m: Mutex<u64> = Mutex::new(7);
        let _ = std::panic::catch_unwind(|| {
            let _g = m.lock().unwrap();
            panic!("poison it");
        });
        let recovered = m.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*recovered, 7);
    }

    /// 会话事实迁移: started/ended 置位复位, 流数据生命周期正确
    #[test]
    fn session_lifecycle_transitions() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 复位到"未在录"的已知初态(锁内独占 session 域)
        {
            let mut c = lock();
            c.session = SessionState { recording: false, level: 0, engine_gens: Vec::new(), engine_partials: Vec::new() };
        }
        assert!(!session_recording());
        session_started();
        assert!(session_recording());
        session_level(123);
        engine_partial(7, "正在说的话");
        session_ended();
        // 松手后: 事实复位, 但该段的流式文字保留(回显不闪断)
        assert!(!session_recording());
        {
            let c = lock();
            assert_eq!(c.session.level, 123);
            assert_eq!(partial_of(&c, 7), "正在说的话");
        }
        // 段终态: 流式文字随 engine_pending_remove 清除
        engine_pending_add(7);
        engine_pending_remove(7);
        {
            let c = lock();
            assert!(partial_of(&c, 7).is_empty());
        }
        session_ended();
    }

    /// 串台回归(2026-11-05 用户报"队列里第二三条的实时内容不对"):
    /// A 段在途(松手后引擎继续出字)时 B 段开始录音——
    /// B 的回显槽不显示 A 的字; A 的字只出现在 A 自己的段视图里。
    /// 录音中回显: spawn 即在途登记+流式文字写入 → snapshot 立即可读
    /// (真机回归: 在途登记曾挂在松手时, 录音中 snapshot 读不到流式文字)
    #[test]
    fn engine_partial_readable_while_recording() {
        let _g = TEST_SESSION_LOCK.lock().unwrap();
        test_reset();
        session_started();
        engine_pending_add(5);
        engine_partial(5, "边说边转的字");
        assert!(session_recording());
        assert_eq!(snapshot().partial, "边说边转的字", "录音中就能读到本段流式文字");
        // 松手(不注销)仍可读; 终态注销后不再有
        session_ended();
        assert_eq!(snapshot().partial, "边说边转的字");
        engine_pending_remove(5);
        assert_eq!(snapshot().partial, "");
    }

    /// 串台回归(2026-11-05 用户报"队列里第二三条的实时内容不对"):
    /// A 段在途(松手后引擎继续出字)时 B 段开始录音——
    /// B 的回显槽不显示 A 的字; A 的字只出现在 A 自己的段视图里。
    #[test]
    fn engine_partials_are_isolated_per_gen() {
        let _g = TEST_SESSION_LOCK.lock().unwrap();
        test_reset();
        engine_pending_add(1); // A: 松手后在途
        engine_partial(1, "A 段的识别文字");
        session_started(); // B 按下
        engine_pending_add(2); // B 引擎会话
        engine_partial(2, "B 段的字");
        let a = snapshot();
        // HUD 左栏(recording 态)显示的是最新段(B)的字, 不是 A 的
        assert_eq!(a.partial, "B 段的字");
        // A 以"转写中"视图出现在投影里(cur 或 queue), 文字是 A 自己的
        let all: Vec<String> = std::iter::once(a.cur)
            .flatten()
            .chain(a.queue.into_iter())
            .map(|s| s.text)
            .collect();
        assert!(all.contains(&"A 段的识别文字".to_string()), "A 的字在段视图: {all:?}");
        assert!(all.contains(&"B 段的字".to_string()));
        // B 终态后, 左栏不再显示 B(最新在途=A)
        engine_pending_remove(2);
        assert_eq!(snapshot().partial, "A 段的识别文字");
    }
}
