//! core —— 应用核心状态（唯一事实源）。
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

/// 段状态(一生单列表: 段实体 spawn 即建, 状态流转全在 queue.items 里)。
/// 展示/取消/worker 全基于它。
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
pub enum SegState {
    /// 按下中(边录边转): partial 实时更新在段上
    Recording,
    /// 引擎在途(松手后等 Result); rerun 任务也用此态(自带会话, worker 跳过)
    Transcribing,
    /// Result 已到(raw 在段上), 等 worker 结算
    Queued,
    /// worker 结算中(纠错→LLM→交付)
    Settling,
    /// 结算失败, 保留在队列里等重试
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
/// **段 spawn 即建**(2026-11-06 定则): 按下热键时创建(state=Recording),
/// 一生(Recording→Transcribing→Queued→Settling→出队/Failed)都在
/// queue.items 里流转——不再有"引擎在途但不是段"的幽灵期, 流式文字
/// (partial)与最终文字(raw)同在段上, cur/queue 是这份列表的纯投影。
pub struct SegmentEntry {
    pub id: u64,
    /// 段来源(Live/Rerun)——投影端据此决定展示; 结算端据此决定去向
    pub source: SegSource,
    /// Rerun 段的目标条目定位(其他段 None)
    pub rerun_ts: Option<String>,
    /// 流式识别文字(Recording/Transcribing 态期间引擎 Partial 事件实时更新)
    pub partial: String,
    /// ASR 最终文字; ASR 阶段失败时为空
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

/// core 全量投影(一次锁取齐; HUD 等展示端是读者, 按内容自行摆放)。
/// cur/queue 的组装只在这里(唯一点): cur=最新 live 段, queue=其余(时间序)。
/// 事件队列不走这里(唯一消费者是 ui 的事件泵: drain 后应用展示规则写通知槽,
/// poll 合并通知槽——两条通道各司其职, 互不相偷)。
pub fn snapshot() -> CoreSnapshot {
    let mut c = lock();
    let (cur, queue) = views_of(&c);
    // 一次性通知槽: 取走即清(前端拿到即消费; 写入侧 notice_put 已 bump version,
    // 去重丢弃的安全窗口由"写入必 bump"保证)
    let oneshot = std::mem::take(&mut c.pending_notice);
    CoreSnapshot {
        version: c.version,
        recording: c.session.recording,
        level: c.session.level,
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
    pub cur: Option<SegView>,
    pub queue: Vec<SegView>,
    pub failed: bool,
    pub oneshot: PendingNotice,
}

/// 投影的无锁内核(调用方持锁)。段一生单列表后, 投影规则只有一条:
/// **live 段按时间序, cur=最新(用户最后说的那条), queue=其余**。
/// rerun 在此层滤掉(展示端不知道 rerun 存在); 段文字 raw 优先、
/// Recording/Transcribing 态用 partial(流式)。
fn views_of(c: &CoreState) -> (Option<SegView>, Vec<SegView>) {
    let text_of = |s: &SegmentEntry| {
        if s.raw.is_empty() {
            s.partial.clone()
        } else {
            s.raw.clone()
        }
    };
    let st = |s: &SegmentEntry| {
        match s.state {
            SegState::Recording => "recording",
            SegState::Transcribing => "transcribing",
            SegState::Queued => "queued",
            SegState::Settling => "transcribing", // 结算中对外显示同"转写中"
            SegState::Failed => "failed",
        }
        .to_string()
    };
    let live: Vec<&SegmentEntry> = c.queue.items.iter().filter(|s| s.source == SegSource::Live).collect();
    let mut it = live.iter().rev();
    let cur = it.next().map(|s| SegView { state: st(s), source: "live".into(), text: text_of(s) });
    // 其余段按时间序(旧→新)
    let mut rest: Vec<&SegmentEntry> = it.rev().cloned().collect();
    rest.reverse(); // it 已是时间序, 再反转保持旧→新
    let queue = rest.iter().map(|s| SegView { state: st(s), source: "live".into(), text: text_of(s) }).collect();
    (cur, queue)
}

/// 队列里是否还有活(录音/引擎在途/排队/结算中)——浮窗存活规则与托盘都用它
pub fn busy() -> bool {
    let c = lock();
    c.queue.items.iter().any(|s| {
        s.source == SegSource::Live
            && matches!(s.state, SegState::Recording | SegState::Transcribing | SegState::Queued | SegState::Settling)
    })
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

/// 会话域状态: "现在是否在录"——全 App 最核心的事实, 从此只有 core 一处。
/// 流式文字不再在会话上(2026-11-06 段实体 spawn 即建): partial 是段字段,
/// 各段视图显示自己的字, 单槽串台问题在结构上不可能了。
pub struct SessionState {
    /// 是否正在录音(按下→松手之间)
    pub recording: bool,
    /// 音量(RMS 0-1000 定点, 录音中 120ms 刷新)
    pub level: u32,
}

// ───────── 段生命周期接口(一生单列表, 2026-11-06 重构)─────────
// 旧"幽灵存储"(engine_gens+engine_partials)删除: 段 spawn 即建, 以下接口
// 是段状态流转的唯一入口(执行各域调用, 全部走 core 锁)。

/// 段诞生: 引擎 spawn 时创建(ctrl_start, state=Recording)。gen=0 忽略(rerun 不走此口)。
pub fn open_live_segment(gen: u64) {
    if gen == 0 {
        return;
    }
    let mut c = lock();
    let id = c.queue.next_id;
    c.queue.next_id += 1;
    c.queue.items.push_back(SegmentEntry {
        id,
        source: SegSource::Live,
        rerun_ts: None,
        partial: String::new(),
        raw: String::new(),
        ho: crate::pipeline::SessionHandoff::default(),
        state: SegState::Recording,
        reason: None,
        asr_failed: false,
    });
    let len = c.queue.items.len();
    let _ = len;
    // gen 记在 ho 上(交接箱写入时 ctrl_stop 会整体覆盖 ho)
    if let Some(s) = c.queue.items.back_mut() {
        s.ho.gen = gen;
    }
    c.version += 1;
}

/// 引擎 Partial 事件: 更新该段的流式文字(按 gen 找段)。gen=0 忽略。
pub fn segment_stream(gen: u64, text: &str) {
    if gen == 0 {
        return;
    }
    let mut c = lock();
    if let Some(s) = c.queue.items.iter_mut().find(|s| s.ho.gen == gen && s.source == SegSource::Live) {
        s.partial = text.to_string();
        c.version += 1;
    }
}

/// 松手: 该段从 Recording 推进到 Transcribing(引擎在途, 等 Result)
pub fn segment_engine_waiting(gen: u64) {
    let mut c = lock();
    if let Some(s) = c.queue.items.iter_mut().find(|s| s.ho.gen == gen && s.state == SegState::Recording) {
        s.state = SegState::Transcribing;
        c.version += 1;
    }
}

/// 引擎 Result 到达: 该段 Transcribing→Queued, 最终文字落到 raw
pub fn segment_result(gen: u64, raw: &str) {
    let mut c = lock();
    if let Some(s) = c.queue.items.iter_mut().find(|s| s.ho.gen == gen && s.state == SegState::Transcribing) {
        s.state = SegState::Queued;
        s.raw = raw.to_string();
        s.partial.clear();
        c.version += 1;
    }
}

/// 引擎失败: 该段 Transcribing→Failed(ASR 阶段失败, raw 空)
pub fn segment_engine_failed(gen: u64, reason: &str) {
    let mut c = lock();
    if let Some(s) = c.queue.items.iter_mut().find(|s| s.ho.gen == gen && s.state == SegState::Transcribing) {
        s.state = SegState::Failed;
        s.reason = Some(reason.to_string());
        s.asr_failed = true;
        s.partial.clear();
        c.version += 1;
    }
}

/// 空转写(没听清): 该段删除(不进失败队列, 无常驻状态)
pub fn segment_discard(gen: u64) {
    let mut c = lock();
    if let Some(pos) = c.queue.items.iter().position(|s| s.ho.gen == gen && s.source == SegSource::Live) {
        c.queue.items.remove(pos);
        c.version += 1;
    }
}

/// 是否还有引擎在途段(Esc 收窗判据; Recording/Transcribing 任一)
pub fn engine_pending() -> bool {
    let c = lock();
    c.queue.items.iter().any(|s| matches!(s.state, SegState::Recording | SegState::Transcribing) && s.source == SegSource::Live)
}

/// 最新一个未终态 live 段的 gen(Esc 取消目标: 最新=列表尾找起)
pub fn latest_live_gen() -> Option<u64> {
    let c = lock();
    c.queue
        .items
        .iter()
        .rev()
        .find(|s| {
            s.source == SegSource::Live
                && matches!(s.state, SegState::Recording | SegState::Transcribing | SegState::Queued | SegState::Settling)
                && s.ho.gen != 0
        })
        .map(|s| s.ho.gen)
}

// ───────── 测试辅助(仅单测可见; 与 TEST_SESSION_LOCK 配套防会话态用例互踩)─────────
#[cfg(test)]
pub fn test_reset() {
    let mut c = lock();
    c.session = SessionState { recording: false, level: 0 };
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
        partial: String::new(),
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
    session: SessionState { recording: false, level: 0 },
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

    /// 会话事实迁移: started/ended 置位复位
    #[test]
    fn session_lifecycle_transitions() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut c = lock();
            c.session = SessionState { recording: false, level: 0 };
        }
        assert!(!session_recording());
        session_started();
        assert!(session_recording());
        session_level(123);
        session_ended();
        assert!(!session_recording());
        {
            let c = lock();
            assert_eq!(c.session.level, 123);
        }
    }

    /// 段一生单列表(2026-11-06 重构): spawn 即建段, 状态流转 Recording→
    /// Transcribing→Queued, 流式文字实时可读, cur=最新段。
    #[test]
    fn segment_lifecycle_single_list() {
        let _g = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        test_reset();
        session_started();
        open_live_segment(5);
        segment_stream(5, "边说边转的字");
        let s = snapshot();
        assert!(s.recording);
        assert_eq!(s.cur.as_ref().map(|c| c.text.as_str()), Some("边说边转的字"), "录音中 cur.text=流式文字");
        assert_eq!(s.cur.as_ref().map(|c| c.state.as_str()), Some("recording"));
        // 松手: Recording→Transcribing, 文字不闪断
        session_ended();
        segment_engine_waiting(5);
        let s = snapshot();
        assert_eq!(s.cur.as_ref().map(|c| c.text.as_str()), Some("边说边转的字"));
        assert_eq!(s.cur.as_ref().map(|c| c.state.as_str()), Some("transcribing"));
        // Result: Transcribing→Queued, 最终文字落 raw
        segment_result(5, "最终识别文字");
        let s = snapshot();
        assert_eq!(s.cur.as_ref().map(|c| c.text.as_str()), Some("最终识别文字"));
        assert_eq!(s.cur.as_ref().map(|c| c.state.as_str()), Some("queued"));
    }

    /// 串台回归(2026-11-05 用户报"队列里第二三条的实时内容不对"):
    /// A 段在途(松手后引擎继续出字)时 B 段开始录音——
    /// cur(左栏)=最新段 B 的字; A 只出现在 queue 里, 文字是 A 自己的。
    #[test]
    fn segments_are_isolated_and_ordered() {
        let _g = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        test_reset();
        // A: spawn→松手→引擎在途
        open_live_segment(1);
        segment_stream(1, "A 段的识别文字");
        segment_engine_waiting(1);
        // B: 按下新录音
        session_started();
        open_live_segment(2);
        segment_stream(2, "B 段的字");
        let a = snapshot();
        // cur=最新段(B), 字是 B 自己的
        assert_eq!(a.cur.as_ref().map(|c| c.text.as_str()), Some("B 段的字"));
        // A 在 queue 里, 字是 A 自己的
        assert_eq!(a.queue.len(), 1);
        assert_eq!(a.queue[0].text, "A 段的识别文字");
        // 单条录音时不出现右栏: 只有 A 时 queue 必须为空
        let s2 = {
            test_reset();
            open_live_segment(3);
            segment_stream(3, "唯一一条");
            snapshot()
        };
        assert!(s2.queue.is_empty(), "单条时队列栏为空(不同步显示同一段)");
        assert_eq!(s2.cur.as_ref().map(|c| c.text.as_str()), Some("唯一一条"));
    }

    /// Esc 取消目标: 最新未终态 live 段; 空转(no-op)
    #[test]
    fn latest_live_gen_for_cancel() {
        let _g = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        test_reset();
        assert_eq!(latest_live_gen(), None);
        open_live_segment(3);
        segment_engine_waiting(3);
        open_live_segment(9);
        assert_eq!(latest_live_gen(), Some(9), "最新段优先");
    }
}
