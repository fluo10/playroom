//! クライアント側のヘルパ。
//!
//! ホストへの接続、`QueryRoom` / `JoinRequest` → チャレンジ応答 → `Welcome`
//! の流れをラップする。

use iroh::EndpointAddr;

use crate::identity::UserIdentity;
use crate::protocol::{
    ClientMessage, FriendSelfInfo, HostMessage, MemberInfo, RoomSummary, PROTOCOL_VERSION,
};
use crate::session::PeerSession;
use crate::{FramedReceiver, FramedSender, NetworkError, NetworkNode};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("network error: {0}")]
    Network(#[from] NetworkError),
    #[error("host rejected: {0}")]
    Rejected(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("signature verification failed")]
    InvalidSignature,
}

/// QueryRoom の応答
pub struct QueryRoomResult {
    pub host_self_info: FriendSelfInfo,
    pub room: Option<RoomSummary>,
}

/// 参加成功時に得られる状態
pub struct JoinedRoom<T>
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    pub room_name: String,
    pub members: Vec<MemberInfo>,
    pub host_self_info: FriendSelfInfo,
    pub sender: FramedSender<ClientMessage<T>>,
    pub receiver: FramedReceiver<HostMessage<T>>,
}

/// ホストに QueryRoom を送信して応答を得る。検証済みの FriendSelfInfo を返す。
pub async fn query_room<T>(
    node: &NetworkNode,
    addr: EndpointAddr,
) -> Result<QueryRoomResult, ClientError>
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    let mut session: PeerSession<ClientMessage<T>, HostMessage<T>> = node.connect(addr).await?;
    session.send(&ClientMessage::QueryRoom).await?;
    let resp = session.recv().await?;
    match resp {
        HostMessage::QueryRoomResponse {
            host_self_info,
            room,
        } => {
            if !host_self_info.verify() {
                return Err(ClientError::InvalidSignature);
            }
            Ok(QueryRoomResult {
                host_self_info,
                room,
            })
        }
        HostMessage::Rejected { reason } => Err(ClientError::Rejected(reason)),
        _ => Err(ClientError::Protocol(
            "unexpected response to QueryRoom".into(),
        )),
    }
}

/// ホストへ参加要求を送り、チャレンジに署名して完了する。
pub async fn join_room<T>(
    node: &NetworkNode,
    addr: EndpointAddr,
    identity: &UserIdentity,
    user_id: String,
) -> Result<JoinedRoom<T>, ClientError>
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    let mut session: PeerSession<ClientMessage<T>, HostMessage<T>> = node.connect(addr).await?;
    session
        .send(&ClientMessage::JoinRequest {
            user_id,
            user_public_key: identity.public_key(),
            protocol_version: PROTOCOL_VERSION,
        })
        .await?;

    // Challenge を受ける
    let nonce = match session.recv().await? {
        HostMessage::Challenge { nonce } => nonce,
        HostMessage::Rejected { reason } => return Err(ClientError::Rejected(reason)),
        _ => return Err(ClientError::Protocol("expected Challenge".into())),
    };

    // 署名して返す
    let sig = identity.sign(&nonce);
    session
        .send(&ClientMessage::JoinResponse { signature: sig })
        .await?;

    // Welcome を受ける
    let resp = session.recv().await?;
    let (sender, receiver, _peer) = session.split();
    match resp {
        HostMessage::Welcome {
            room_name,
            members,
            host_self_info,
        } => {
            if !host_self_info.verify() {
                return Err(ClientError::InvalidSignature);
            }
            Ok(JoinedRoom {
                room_name,
                members,
                host_self_info,
                sender,
                receiver,
            })
        }
        HostMessage::Rejected { reason } => Err(ClientError::Rejected(reason)),
        _ => Err(ClientError::Protocol("expected Welcome".into())),
    }
}
