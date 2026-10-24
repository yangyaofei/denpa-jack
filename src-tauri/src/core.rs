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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegState {
    /// 已入队, 等 worker 取
    Queued,
    /// worker 正在跑这段(同一时刻最多一段)
    Transcribing,
    /// 后处理失败, 保留在队列里等重试
    Failed,
}

/// 段实体: 展示态与干活所需的交接上下文是同一段实体的两面, 不拆存——
/// 段的真相只有 core 这一处(segment_queue.rs 是它的执行者, 不再自持队列)。
pub struct SegmentEntry {
    pub id: u64,
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

/// 核心状态: 全部可投影的数据(投影函数只读这里)
pub struct CoreState {
    /// 全局版本号: 任一状态迁移 +1, 轮询方据此判断"有没有新东西"
    pub version: u64,
    /// 段队列域(第 1 步搬入)
    pub queue: QueueState,
}

static CORE: Mutex<CoreState> = Mutex::new(CoreState {
    version: 0,
    queue: QueueState { items: VecDeque::new(), next_id: 1 },
});

/// 取 core 锁(中毒时取出数据继续——见模块头"锁中毒策略")
pub fn lock() -> MutexGuard<'static, CoreState> {
    CORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 当前全局版本号(读一次, 微秒级)
pub fn version() -> u64 {
    lock().version
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
}
