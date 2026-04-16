use serde::{Deserialize, Serialize};

use crate::error::NetworkError;
use crate::framing::{FramedReceiver, FramedSender};
use iroh::EndpointId;

/// iroh QUIC 双方向ストリーム上の型付き送受信セッション。
///
/// `S`: 送信するメッセージ型（このノードが送る）
/// `R`: 受信するメッセージ型（相手ノードが送る）
pub struct PeerSession<S, R> {
    sender: FramedSender<S>,
    receiver: FramedReceiver<R>,
    peer_id: EndpointId,
}

impl<S: Serialize, R: for<'de> Deserialize<'de>> PeerSession<S, R> {
    pub(crate) fn new(
        send: iroh::endpoint::SendStream,
        recv: iroh::endpoint::RecvStream,
        peer_id: EndpointId,
    ) -> Self {
        Self {
            sender: FramedSender::new(send),
            receiver: FramedReceiver::new(recv),
            peer_id,
        }
    }

    pub async fn send(&mut self, msg: &S) -> Result<(), NetworkError> {
        self.sender.send(msg).await
    }

    pub async fn recv(&mut self) -> Result<R, NetworkError> {
        self.receiver.recv().await
    }

    pub fn peer_id(&self) -> EndpointId {
        self.peer_id
    }

    /// 送受信を独立したハーフに分割する（tokio::select! や spawn 用）
    pub fn split(self) -> (FramedSender<S>, FramedReceiver<R>, EndpointId) {
        (self.sender, self.receiver, self.peer_id)
    }
}
