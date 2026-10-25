//! core —— 应用核心状态(唯一事实源)。设计定稿: docs/CORE-DESIGN.md。
//!
//! # 定位与红线
//! - 所有活状态的家 + 静态数据的门面。执行模块只连 core;
//!   core 只连纯数据模块(settings/history), 不 import 任何执行模块——依赖一条直线。
//! - core 里只有数据与投影, 没有业务: 不录音/不发请求/不弹窗/不睡眠等待。
//! - 无线程: 一把锁短临界区(字段赋值/快照拷贝, 微秒级)。
//!   panic/信号处理路径禁止进入 core 锁。
//!
//! # 搬迁进度(每步系统完整可用, 见 CORE-DESIGN.md 第八节)
//! - [x] 第 1 步: 段队列域(本文件现状)
//! - [ ] 第 2 步: 会话域(SessionState + Resources)
//! - [ ] 第 3 步: toast/设备/网关/权限/更新/控制域 + 配置/历史门面
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

/// 段队列域状态
pub struct QueueState {
    /// 段按处理顺序; 交付成功即离场, 失败段保留待重试
    pub items: VecDeque<SegmentEntry>,
    pub next_id: u64,
}

/// 会话域状态(第 2a 步搬入): "现在是否在录、录的内容"——全 App 最核心的事实,
/// 从此只有 core 一处(HUD 只是它的投影)。
/// 注: 阶段展示语义(transcribing/busy 等展示态)仍归 hud.rs 的展示状态机,
/// 那是"事实的投影规则", 不是事实本身。
pub struct SessionState {
    /// 是否正在录音(按下→松手之间)
    pub recording: bool,
    /// 音量(RMS 0-1000 定点, 录音中 120ms 刷新)
    pub level: u32,
    /// 实时回显(流式引擎的 partial 文本)
    pub partial: String,
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
}

static CORE: Mutex<CoreState> = Mutex::new(CoreState {
    version: 0,
    queue: QueueState { items: VecDeque::new(), next_id: 1 },
    session: SessionState { recording: false, level: 0, partial: String::new() },
    ctrl: CtrlState { config_version: 0, history_version: 0, mic_list_version: 0 },
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

/// 一条新录音开始: 事实置位 + 清上一条的流数据
pub fn session_started() {
    let mut c = lock();
    c.session.recording = true;
    c.session.level = 0;
    c.session.partial.clear();
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

/// 实时回显(逐字, 全量替换语义)
pub fn session_partial(text: &str) {
    let mut c = lock();
    c.session.partial = text.to_string();
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
pub fn get_history(app: &tauri::AppHandle, limit: usize) -> Vec<crate::history::HistoryRecord> {
    crate::history::recent(app, limit)
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

    /// 会话事实迁移: started/ended 置位复位, 流数据生命周期正确
    #[test]
    fn session_lifecycle_transitions() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 复位到"未在录"的已知初态(锁内独占 session 域)
        {
            let mut c = lock();
            c.session = SessionState { recording: false, level: 0, partial: String::new() };
        }
        assert!(!session_recording());
        session_started();
        assert!(session_recording());
        session_level(123);
        session_partial("正在说的话");
        session_ended();
        // 松手后: 事实复位, 但 partial 保留(回显不闪断)
        assert!(!session_recording());
        {
            let c = lock();
            assert_eq!(c.session.level, 123);
            assert_eq!(c.session.partial, "正在说的话");
        }
        // 下一段开始: 流数据必须被清干净(不闪上一条)
        session_started();
        {
            let c = lock();
            assert_eq!(c.session.level, 0);
            assert!(c.session.partial.is_empty());
        }
        session_ended();
    }
}
