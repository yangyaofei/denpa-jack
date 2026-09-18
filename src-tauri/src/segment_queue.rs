//! 分段转写队列(async transcribe queue)
//!
//! # 为什么需要它
//!
//! 松开热键后, ASR 识别 → 词典纠错 → (可选)LLM 润色 → 交付在后台进行。
//! 此期间如果再次按下热键, 旧行为是"直接开新录音、两段并行跑完后处理":
//! - 交付顺序不保证(后一段的模型更快就先到, 听写顺序颠倒)
//! - 粘贴事务是全局单槽(paste_tx::PENDING), 且"新事务先结算旧事务"
//!   → 后一段开始粘贴时会把前一段的事务提前结算, 前一段可能只进剪贴板、
//!     没贴进光标(静默丢文本)
//! - 浮窗只有一份状态快照, 后一段的"录音中"会覆盖前一段的"转写中"
//!
//! # 规则(本轮讨论定稿, 实现与浮窗都必须遵守)
//!
//! 1. **录音独占 + 转写严格串行**: 同一时刻只有一段在录音(协调器保证);
//!    转写由本模块的单 worker 按 FIFO 逐段执行 → 交付顺序恒等于录音顺序。
//! 2. **队列不设上限**: 想录多少段就录多少段; 只有浮窗的*展示*上限(5 行)。
//! 3. **失败不阻塞**: 某段失败只标记该段(state=Failed, 保留音频与上下文),
//!    后续段照常转写、照常交付; 失败段在交付序列里空出。
//! 4. **浮窗存活**(前端按快照判定, 见 src/hud.ts):
//!    - 录音中 / 队列非空(排队或转写中) → 一直显示, 无超时
//!    - 全部交付完成 → 350ms 后收起(与既有"粘贴成功浮窗立即消失"一致)
//!    - 队列里只剩失败 → 2.5s 提示后收起(与既有"录音太短已丢弃"提示同长)
//! 5. **不丢段**: 失败段的音频与原文进历史(delivered="failed"), 可在浮窗重试或在
//!    历史页重跑; 「清空」不是丢数据——排队段的音频与原文同样先进历史
//!    (delivered="cancelled", warning="队列已清空")再出队。
//!
//! # 重试语义
//!
//! - ASR 之后的环节失败(交付/配置等): 段内已有 raw 与音频路径 → 直接重新入队,
//!   只重跑后处理(不重新识别, 快)。
//! - ASR 本身失败(引擎 Error, 没有 raw): 走 rerun_history 重跑识别, 其结果通过
//!   引擎事件重新入队 → 仍然按队列顺序交付。
//!   这类段的音频在入队时就落盘, 保证"能重试"不是空话。
//!
//! # 线程模型
//!
//! - `enqueue*()` 由引擎事件线程调用: 只加锁入队 + 唤醒 worker, 不做重活。
//! - worker 是一条常驻线程: 取最早一个 Queued 段 → 置 Transcribing → 释放锁后
//!   执行 `pipeline::post_process`(同步阻塞) → 回写结果(成功出队/失败标 Failed)。
//! - 浮窗状态由 `views()` 加锁拷贝快照, 不跨线程持有锁。
//! - 托盘"转写中"指示在每次状态变化后统一刷新(有活=黄, 无活=默认)。

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// 段状态(浮窗右栏/存活规则都基于它)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegState {
    /// 已入队, 等 worker 取
    Queued,
    /// worker 正在跑这段(同一时刻最多一段)
    Transcribing,
    /// 后处理失败, 保留在队列里等重试
    Failed,
}

/// 一个待转写段(音频 + 原文 + 会话上下文)
pub struct Segment {
    pub id: u64,
    /// ASR 原文; ASR 阶段失败时为空
    pub raw: String,
    /// 会话交接(音频路径/PCM/焦点/时长/截图/代) —— 重试所需的全部上下文
    pub ho: crate::pipeline::SessionHandoff,
    pub state: SegState,
    /// 失败原因(浮窗提示 + 历史 warning)
    pub reason: Option<String>,
    /// true = ASR 阶段就失败(没有 raw): 重试要重跑识别而不是重跑后处理
    pub asr_failed: bool,
}

/// 给前端的段视图(浮窗渲染用; 只有 state + text, 不带段号等多余信息——用户定则)
#[derive(Clone, Debug, serde::Serialize)]
pub struct SegView {
    /// 录音中 | transcribing | queued | failed
    pub state: String,
    pub text: String,
}

struct Queue {
    items: VecDeque<Segment>,
    next_id: u64,
}

static Q: Mutex<Queue> = Mutex::new(Queue { items: VecDeque::new(), next_id: 1 });
static CV: Condvar = Condvar::new();

/// 启动 worker 线程(在 lib.rs setup 调用一次)
pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        // 1) 取一段(取不到就睡在条件变量上, 不忙等)
        let run = {
            let mut g = Q.lock().unwrap();
            loop {
                // FIFO: 最早的 Queued(重试回队的段也按它在队列里的位置参与排序)
                if let Some(pos) = g.items.iter().position(|s| s.state == SegState::Queued) {
                    g.items[pos].state = SegState::Transcribing;
                    let s = &g.items[pos];
                    break (s.id, s.raw.clone(), s.ho.clone(), s.asr_failed);
                }
                g = CV.wait(g).unwrap();
            }
        };
        let (id, raw, ho, asr_failed) = run;

        // 2) 执行(不持锁): ASR 阶段失败过的段不再跑后处理, 直接判失败
        let outcome = if asr_failed {
            Err("语音识别失败".to_string())
        } else {
            match crate::pipeline::post_process(app.clone(), raw.clone(), ho.clone()) {
                crate::pipeline::Outcome::Delivered | crate::pipeline::Outcome::NoSpeech => Ok(()),
                crate::pipeline::Outcome::Failed(e) => Err(e),
            }
        };

        // 3) 回写: 成功出队; 失败保留(可重试), 并把音频与原文补进历史(不丢段)
        let mut fail_record: Option<(String, crate::pipeline::SessionHandoff, String)> = None;
        {
            let mut g = Q.lock().unwrap();
            if let Some(pos) = g.items.iter().position(|s| s.id == id) {
                match outcome {
                    Ok(()) => {
                        g.items.remove(pos);
                    }
                    Err(e) => {
                        g.items[pos].state = SegState::Failed;
                        g.items[pos].reason = Some(e.clone());
                        // ASR 阶段失败的段在入队时已经落盘+写过历史, 这里只处理后处理失败
                        if !g.items[pos].asr_failed {
                            fail_record = Some((g.items[pos].raw.clone(), g.items[pos].ho.clone(), e));
                        }
                    }
                }
            }
        }
        if let Some((raw, ho, reason)) = fail_record {
            let p = record_failed(&app, &raw, &ho, &reason);
            log_debug(&format!("后处理失败已入历史 audio={:?}", p));
        }
        log_debug("队列状态变化");
    });
}

/// 入队一段 ASR 成功的结果(引擎 Result 事件调用)
///
/// 注意: 这里不再需要 app —— 队列只做入队与唤醒, 后处理由 worker 执行(见 start)。
pub fn enqueue(_app: &tauri::AppHandle, raw: String, ho: Option<crate::pipeline::SessionHandoff>) -> u64 {
    let id = {
        let mut g = Q.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.items.push_back(Segment {
            id,
            raw,
            ho: ho.unwrap_or_default(),
            state: SegState::Queued,
            reason: None,
            asr_failed: false,
        });
        id
    };
    CV.notify_all();
    log_debug("入队");
    id
}

/// 入队一段 ASR 失败的段(引擎 Error 事件调用)
///
/// 先落盘音频 + 写历史(不丢段), 再以 Failed 状态留在队列里等重试;
/// worker 不会重跑它的后处理(asr_failed=true), 只会把它标成失败。
pub fn enqueue_asr_failed(app: &tauri::AppHandle, reason: String, ho: Option<crate::pipeline::SessionHandoff>) -> u64 {
    let ho = ho.unwrap_or_default();
    // 落盘音频 + 写历史(delivered="failed") —— 保证"能重试"不是空话
    let audio = record_failed(app, "", &ho, &reason);
    let id = {
        let mut g = Q.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        let mut seg_ho = ho.clone();
        seg_ho.audio_path = audio;
        g.items.push_back(Segment { id, raw: String::new(), ho: seg_ho, state: SegState::Failed, reason: Some(reason), asr_failed: true });
        id
    };
    log_debug("ASR 失败段入队(已落盘+入历史)");
    id
}

/// 失败留痕: 音频落盘 + 历史记录(delivered="failed")。返回落盘后的音频路径。
///
/// 规则 5 的落点: 无论失败发生在 ASR 阶段还是后处理阶段, 音频与原文都必须能再找到。
fn record_failed(app: &tauri::AppHandle, raw: &str, ho: &crate::pipeline::SessionHandoff, reason: &str) -> Option<String> {
    use tauri::Manager;
    let mut audio = ho.audio_path.clone();
    if !ho.pcm.is_empty() {
        if let Some(p) = crate::history::save_wav(app, &ho.pcm) {
            app.state::<std::sync::Mutex<crate::AppState>>().lock().unwrap().last_audio = Some(p.clone());
            audio = Some(p);
        }
    }
    let keep = crate::settings::get_config(app.clone()).map(|c| c.keep_audio_count.max(1) as usize).unwrap_or(20);
    crate::history::prune_recordings(app, keep);
    let rec = crate::history::HistoryRecord {
        ts: crate::pipeline::now_local_pub(),
        engine: ho.engine.clone(),
        raw: raw.to_string(),
        final_text: raw.to_string(),
        llm_used: false,
        delivered: "failed".into(),
        audio_path: audio.clone().unwrap_or_default(),
        warning: Some(reason.to_string()),
        duration_ms: ho.duration_ms,
        llm_thinking: None,
    };
    crate::history::append(app, &rec);
    audio
}

/// 空转写留痕(没听清/全程没声音): 只写一条历史(delivered="none"), 不进队列、不留常驻状态。
///
/// 与 record_failed 的唯一区别是 `delivered` 值: "none" = 本来就没有内容可交付, 不是失败。
/// 旧实现(1cbc9d8^ 的 engines.rs:85 Error 分支)就是这个语义, 队列化时误把它并入失败路径(issue #6)。
pub fn record_no_speech(
    app: &tauri::AppHandle,
    ho: Option<crate::pipeline::SessionHandoff>,
    msg: &str,
) -> Option<String> {
    use tauri::Manager;
    let ho = ho.unwrap_or_default();
    let mut audio = ho.audio_path.clone();
    if !ho.pcm.is_empty() {
        if let Some(p) = crate::history::save_wav(app, &ho.pcm) {
            app.state::<std::sync::Mutex<crate::AppState>>().lock().unwrap().last_audio = Some(p.clone());
            audio = Some(p);
        }
    }
    let keep = crate::settings::get_config(app.clone()).map(|c| c.keep_audio_count.max(1) as usize).unwrap_or(20);
    crate::history::prune_recordings(app, keep);
    let rec = crate::history::HistoryRecord {
        ts: crate::pipeline::now_local_pub(),
        engine: ho.engine.clone(),
        raw: String::new(),
        final_text: String::new(),
        llm_used: false,
        delivered: "none".into(),
        audio_path: audio.clone().unwrap_or_default(),
        warning: Some(msg.to_string()),
        duration_ms: ho.duration_ms,
        llm_thinking: None,
    };
    crate::history::append(app, &rec);
    log_debug("空转写已入历史(不进队列)");
    audio
}

/// 重试一段失败(浮窗按钮/历史页调用)
pub fn retry(app: &tauri::AppHandle, id: u64) -> Result<(), String> {
    let (path, asr_failed) = {
        let mut g = Q.lock().unwrap();
        let pos = g.items.iter().position(|s| s.id == id).ok_or("该段已不在队列中")?;
        if g.items[pos].state != SegState::Failed {
            return Err("该段不是在失败状态".into());
        }
        if g.items[pos].asr_failed {
            // ASR 阶段失败 → 重跑识别(rerun_history 读已落盘 WAV); 结果会作为新段重新入队
            (g.items[pos].ho.audio_path.clone().unwrap_or_default(), true)
        } else {
            g.items[pos].state = SegState::Queued;
            g.items[pos].reason = None;
            (String::new(), false)
        }
    };
    if asr_failed {
        if path.is_empty() {
            return Err("该段没有可用的音频文件".into());
        }
        remove(id);
        crate::commands::rerun_history(app.clone(), path)?;
    } else {
        CV.notify_all();
    }
    log_debug("重试");
    Ok(())
}

/// 清空队列(浮窗「清空」按钮, 前端两步确认后调用)
///
/// 语义: 不转写 ≠ 丢数据 —— 排队中的段先把音频与原文写进历史
/// (delivered="cancelled", warning="队列已清空"), 再出队;
/// 正在转写的那一段不受影响(它已经跑起来了, 让它交付完)。
pub fn clear(app: &tauri::AppHandle) -> usize {
    // 出队 + 备份要清掉的段(用 pop_front 逐个搬运, 段是不可 Clone 的持有权对象)
    let dropped: Vec<Segment> = {
        let mut g = Q.lock().unwrap();
        let mut keep: VecDeque<Segment> = VecDeque::new();
        let mut dropped: Vec<Segment> = Vec::new();
        while let Some(s) = g.items.pop_front() {
            if s.state == SegState::Queued {
                dropped.push(s);
            } else {
                keep.push_back(s);
            }
        }
        g.items = keep;
        dropped
    };
    let n = dropped.len();
    for s in dropped {
        if !s.ho.pcm.is_empty() {
            let _ = crate::history::save_wav(app, &s.ho.pcm);
        }
        let rec = crate::history::HistoryRecord {
            ts: crate::pipeline::now_local_pub(),
            engine: s.ho.engine.clone(),
            raw: s.raw.clone(),
            final_text: s.raw.clone(),
            llm_used: false,
            delivered: "cancelled".into(),
            audio_path: s.ho.audio_path.clone().unwrap_or_default(),
            warning: Some("队列已清空".into()),
            duration_ms: s.ho.duration_ms,
            llm_thinking: None,
        };
        crate::history::append(app, &rec);
    }
    log_debug("清空队列");
    n
}

/// 「关闭」: 不再提示队列里的失败段(浮窗按钮)。
///
/// 语义 = 只停止提示, 不丢数据 —— 失败段的音频与原文在失败当时就已写进历史
/// (`delivered="failed"`), 之后仍可在主窗「历史」里重跑。
/// 已交付/正在转写的段不受影响。
pub fn dismiss_failed() -> usize {
    let n = {
        let mut g = Q.lock().unwrap();
        let before = g.items.len();
        g.items.retain(|s| s.state != SegState::Failed);
        before - g.items.len()
    };
    log_debug("失败段已关闭提示");
    n
}

/// 浮窗视图: (当前段, 队列其余段)
///
/// 当前段 = 正在转写的那段(录音中的"当前"由浮窗用 HUD 快照里的 partial 表示,
/// 见 commands::hud_poll —— 录音中的段还不属于队列)。
pub fn views() -> (Option<SegView>, Vec<SegView>) {
    let g = Q.lock().unwrap();
    let cur = g.items.iter().find(|s| s.state == SegState::Transcribing).map(|s| SegView {
        state: "transcribing".into(),
        text: s.raw.clone(),
    });
    let queue: Vec<SegView> = g
        .items
        .iter()
        .filter(|s| s.state != SegState::Transcribing)
        .map(|s| SegView {
            state: if s.state == SegState::Failed { "failed".into() } else { "queued".into() },
            text: s.raw.clone(),
        })
        .collect();
    (cur, queue)
}

/// 队列里是否还有活(排队或转写中)——浮窗存活规则与托盘都用它
pub fn busy() -> bool {
    let g = Q.lock().unwrap();
    g.items.iter().any(|s| s.state == SegState::Queued || s.state == SegState::Transcribing)
}

/// 队列里是否有未处理的失败段
pub fn has_failed() -> bool {
    let g = Q.lock().unwrap();
    g.items.iter().any(|s| s.state == SegState::Failed)
}

/// 队列里失败段中最早的一个 id(浮窗"重试"按钮用)
pub fn first_failed_id() -> Option<u64> {
    let g = Q.lock().unwrap();
    g.items.iter().find(|s| s.state == SegState::Failed).map(|s| s.id)
}

/// 出队一段(重试用; ASR 重跑前把原段移掉)
fn remove(id: u64) {
    let mut g = Q.lock().unwrap();
    if let Some(pos) = g.items.iter().position(|s| s.id == id) {
        g.items.remove(pos);
    }
}

/// 测试辅助: 队列长度/段 id 列表(单测用, 不参与运行时逻辑)
#[cfg(test)]
pub fn debug_ids() -> Vec<(u64, SegState)> {
    let g = Q.lock().unwrap();
    g.items.iter().map(|s| (s.id, s.state)).collect()
}

#[cfg(test)]
pub fn debug_reset() {
    let mut g = Q.lock().unwrap();
    g.items.clear();
    g.next_id = 1;
}

fn log_debug(what: &str) {
    let g = Q.lock().unwrap();
    let queued = g.items.iter().filter(|s| s.state == SegState::Queued).count();
    let trans = g.items.iter().filter(|s| s.state == SegState::Transcribing).count();
    let failed = g.items.iter().filter(|s| s.state == SegState::Failed).count();
    drop(g);
    crate::log::elog(&format!("[queue] {what}: queued={queued} transcribing={trans} failed={failed}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_states() {
        debug_reset();
        // 直接构造 Segment 不依赖 AppHandle: 只验证队列的排序/状态机部分
        {
            let mut g = Q.lock().unwrap();
            for id in 1..=3u64 {
                g.items.push_back(Segment { id, raw: format!("seg{id}"), ho: Default::default(), state: SegState::Queued, reason: None, asr_failed: false });
            }
            g.next_id = 4;
        }
        let ids = debug_ids();
        assert_eq!(ids, vec![(1, SegState::Queued), (2, SegState::Queued), (3, SegState::Queued)]);
        // 取最早的一个 → Transcribing
        {
            let mut g = Q.lock().unwrap();
            let pos = g.items.iter().position(|s| s.state == SegState::Queued).unwrap();
            g.items[pos].state = SegState::Transcribing;
        }
        let ids = debug_ids();
        assert_eq!(ids, vec![(1, SegState::Transcribing), (2, SegState::Queued), (3, SegState::Queued)]);
        // busy/has_failed 的判定
        assert!(busy() && !has_failed());
        debug_reset();
        assert!(!busy() && !has_failed());
    }

    #[test]
    fn failed_segment_keeps_position_and_does_not_block() {
        debug_reset();
        {
            let mut g = Q.lock().unwrap();
            g.items.push_back(Segment { id: 1, raw: "a".into(), ho: Default::default(), state: SegState::Failed, reason: Some("boom".into()), asr_failed: false });
            g.items.push_back(Segment { id: 2, raw: "b".into(), ho: Default::default(), state: SegState::Queued, reason: None, asr_failed: false });
        }
        // 失败段留在队列里(可重试), 后面的段照常可被取走(不阻塞)
        assert!(has_failed());
        assert_eq!(first_failed_id(), Some(1));
        let (cur, q) = views();
        assert!(cur.is_none());
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].state, "failed");
        assert_eq!(q[1].state, "queued");
        debug_reset();
    }
}
