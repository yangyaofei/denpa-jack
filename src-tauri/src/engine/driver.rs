// C37 会话驱动 plumbing: WS 引擎(doubao/local)共用的双输入单通道骨架。
//
// 为什么存在: 实测三路 select 会静默吞掉就绪分支——豆包收尾阶段连必到的
// watchdog 都不醒(C37 事故), 修复后的骨架提炼于此: 命令流与 WS 消息流
// 各自转发进统一通道, 主循环单 recv, 收尾等待用 timeout_at(watchdog)
// 包裹——select 静默挂死在这个结构上不可能发生。
//
// 引擎差异(消息编解码/业务分支)不在此层: 本层只管"两个输入源怎么安全
// 地汇到一个循环里", 各引擎的 run 消费 Inner 自行解析。
use super::contract::Cmd;
use futures_util::Stream;

/// 统一通道的消息: 命令 或 WS 消息(None=流结束/断连, 只发一次)
pub(crate) enum Inner<T> {
    Cmd(Cmd),
    Ws(Option<T>),
}

/// 命令流转发: 外部命令通道 → 统一通道(转发 task 随通道关闭自然退出)
pub(crate) fn forward_cmds<T: Send + 'static>(
    mut rx: tokio::sync::mpsc::Receiver<Cmd>,
    tx: tokio::sync::mpsc::Sender<Inner<T>>,
) {
    tokio::spawn(async move {
        while let Some(c) = rx.recv().await {
            if tx.send(Inner::Cmd(c)).await.is_err() {
                break;
            }
        }
    });
}

/// WS 读取转发: 消息流 → 统一通道; 断连/结束发一次 Ws(None) 后退出
pub(crate) fn forward_ws<T: Send + 'static, S>(
    mut stream: S,
    tx: tokio::sync::mpsc::Sender<Inner<T>>,
) where
    S: Stream<Item = Result<T, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    use futures_util::StreamExt;
    tokio::spawn(async move {
        loop {
            match stream.next().await {
                Some(Ok(m)) => {
                    if tx.send(Inner::Ws(Some(m))).await.is_err() {
                        break;
                    }
                }
                _ => {
                    let _ = tx.send(Inner::Ws(None)).await;
                    break;
                }
            }
        }
    });
}
