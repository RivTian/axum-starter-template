//! 面的「准备完成」回执（启动提交门用它判断能不能提交）。
//!
//! # 为什么回执不带任何负载
//!
//! 一个自然的设计是让 `Ack` 携带面特有的凭证：HTTP 面回传**真实绑定的 `SocketAddr`**
//! （端口为 0 时这是唯一的知情途径）。这里没有这么做——`bind(addr)` 被单独列成了
//! `api` 的公共出口，`app` 在 `launch()` **之前**就同步 bind 完，真实地址从 `TcpListener`
//! 上直接读得到。"唯一的知情途径"这句话在同一份设计里已经被另一条决定推翻了。
//!
//! 既然唯一一个有负载的面不需要负载，回执就退化成一个纯信号。这不是省事：一个带 payload 的
//! 枚举会诱导后来的人把"面想告诉装配层的事"都塞进来，而那些事应当走配置或状态，不该走一条
//! 一次性的启动期通道。
//!
//! # 面报不出一个假身份
//!
//! [`TaskName`] 挂在**接收端**上，发送端根本没有地方能写名字。于是"任务是 A、回执记成 B"
//! 这一整类缺陷在这里不存在——和 [`RuntimeId::of`](crate::task::RuntimeId::of) 从 `Handle`
//! 派生 runtime 标识是同一条手法。

use tokio::sync::oneshot;

use crate::task::TaskName;

/// 面这一侧的回执端。
///
/// 一次性：[`send`](Self::send) 消费自身，所以"回执发两次"在类型上无从发生。
#[derive(Debug)]
pub struct AckSender {
    tx: oneshot::Sender<()>,
}

impl AckSender {
    /// 报告本面已完成自己的准备。
    ///
    /// 发送失败（接收端已经没了）被**刻意忽略**：那意味着提交门已经放弃等待，进程正走在
    /// 启动失败路径上。此时让面因为"回执没发出去"再报一个错，只会在日志里盖住真正的首因。
    pub fn send(self) {
        let _ = self.tx.send(());
    }
}

/// 装配这一侧的回执端。
///
/// 持有它就等于持有"还欠一份回执"这件事——提交门的 `all_received()` 谓词按这些实例的
/// 消耗情况判断，而不是按 `select!` 的分支顺序。
#[derive(Debug)]
pub struct AckReceiver {
    plane: TaskName,
    rx: oneshot::Receiver<()>,
}

impl AckReceiver {
    /// 这份回执属于哪个面。
    #[must_use]
    pub const fn plane(&self) -> TaskName {
        self.plane
    }

    /// 等这个面的回执。
    ///
    /// 返回 `false` 表示面在发出回执之前就没了（返回、失败或 panic 都会 drop 发送端）。
    /// 提交门**不**只靠这一条判断失败：`supervisor.next()` 那一臂会拿到同一件事的退出记录，
    /// 而那条路径带着原因。两条独立的探测指向同一个事实，是有意的冗余。
    pub async fn recv(self) -> bool {
        self.rx.await.is_ok()
    }
}

/// 为一个面建一对回执端。
///
/// 名字由装配方在这里一次写定，之后两端都改不了它。
#[must_use]
pub fn ack_channel(plane: TaskName) -> (AckSender, AckReceiver) {
    let (tx, rx) = oneshot::channel();
    (AckSender { tx }, AckReceiver { plane, rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_sent_ack_arrives_with_the_name_the_assembler_chose() {
        let (tx, rx) = ack_channel(TaskName::Ticker);
        assert_eq!(rx.plane(), TaskName::Ticker);
        tx.send();
        assert!(rx.recv().await);
    }

    #[tokio::test]
    async fn a_dropped_sender_reads_as_a_missing_ack_not_as_a_hang() {
        // 面在发回执之前就没了：等待方必须立刻得到"没有"，而不是一直挂着——
        // 提交门挂住的话，整个进程就卡在 `Starting` 上，连失败都报不出来。
        let (tx, rx) = ack_channel(TaskName::Http);
        drop(tx);
        assert!(!rx.recv().await);
    }

    #[tokio::test]
    async fn sending_into_a_closed_channel_is_silent() {
        // 提交门已经放弃等待时，面这边不该再抛出第二个错误盖住首因。
        let (tx, rx) = ack_channel(TaskName::Ticker);
        drop(rx);
        tx.send();
    }
}
