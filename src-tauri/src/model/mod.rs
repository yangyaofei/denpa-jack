//! model 域：模型能力的接驳层（docs/ARCHITECTURE.md 四域之一）。
//!
//! 门面（外部只许经 model:: 调这些）：
//!   - speech::resolve(cfg) · speech::transcribe(...) — ASR 引擎分派与识别
//!   - llm::refine(raw, cfg, profile, shot) — 纠错+润色管线（词典→LLM→降级）
//!   - llm::selftest(profile) / list_models — LLM 侧自检与档案
//!   - dict::budget_hotwords — 热词预算（speech 编排细节, 内部用）
//!   - 类型 speech::{Cmd, AsrEvent, Handoff}
//!
//! 内部（域外不可见，改组不影响调用方）：
//!   - speech/ — trait+四引擎接入(doubao/zhipu/local)+gateway(本地网关进程)+driver(WS 骨架)
//!   - dict.rs — 词典纠错(确定性文本变换)
//!   - llm.rs — OpenAI 兼容调用+纠错管线编排
//!
//! 禁止：感知环境（麦克风/焦点/截图都由 capture 决定后传入）；直接交付（结果交 pipeline）。
pub mod dict;
pub mod llm;
pub mod speech;

// 批3 门面收口: 外部经 crate::model::<门面符号> 调用。白名单由 build.rs 门面闸强制。
// ───────── 门面(点名导出)─────────
// model::transcribe — 提交一段转写(录音松开后; 原 spawn_session)
// model::retranscribe — 历史重转(后台重算回写, 原 spawn_rerun_session)
// model::resolve_engine — 配置→可用引擎档案+key(原 resolve_asr)
// model::cancel_current — 取消转写中的会话(Esc 中止)
// model::refine — 纠错管线(词典→LLM→降级); selftest/list_models — LLM 自检
pub use speech::{cancel_current, retranscribe, resolve_engine, transcribe, AsrEvent, Cmd};
pub use llm::{list_models, refine, selftest};
pub use dict::budget_hotwords; // 会话构造时算热词(recording/commands 传参; 显式数据流, 不下沉)
