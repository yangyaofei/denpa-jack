// 引擎公共契约: 命令与事件类型(四个引擎共用的接口语言)。
// 从 doubao.rs 提出(阶段A trait 化): 类型归契约文件, 引擎不再互相 import——
// 豆包是引擎之一, 不是其他引擎的类型提供方。
//
// 会话生命线: Feed* → Finish → 恰好一次结算(Result | NoSpeech | Error)
// (见 docs/CONTRACTS.md 恰好一次节; Abort 丢弃会话不结算)。

/// 会话命令(ctrl_start 的音频桥/松手/取消 → 引擎会话)
pub enum Cmd {
    Feed(Vec<u8>),
    Finish,
    Abort,
}

#[derive(Debug, Clone)]
pub enum AsrEvent {
    Partial(String),
    Result(String),
    /// 转写为空(没听清/全程没声音)。不是失败: 只提示 + 写一条历史, 不进队列、不常驻(issue #6)
    NoSpeech(String),
    Error(String),
}
