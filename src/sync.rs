//! デバイス間同期（クライアント側）
//!
//! 自分の別デバイスに接続し、自身の署名付き SyncPayload を送って、相手からの
//! 応答とマージする。相手のペイロードは自分の user_public_key で検証できる
//! （同一ユーザーの devices なら同じ秘密鍵を保有している）。

use iroh::EndpointAddr;

use crate::client::ClientError;
use crate::config::AppConfig;
use crate::identity::UserIdentity;
use crate::protocol::{ClientMessage, HostMessage, SyncPayload};
use crate::session::PeerSession;
use crate::NetworkNode;

/// 同期結果の概要
pub struct SyncOutcome {
    /// マージで何か変化があったか
    pub changed: bool,
}

/// 指定した自デバイスと同期する。config はマージ結果で更新され、必要なら保存する。
pub async fn sync_with_peer(
    node: &NetworkNode,
    addr: EndpointAddr,
    identity: &UserIdentity,
    config: &mut AppConfig,
) -> Result<SyncOutcome, ClientError> {
    // 自分のペイロードを作成
    let payload = SyncPayload::sign_new(
        identity,
        config.my_devices.clone(),
        config.friends.clone(),
    );

    let mut session: PeerSession<ClientMessage<()>, HostMessage<()>> = node.connect(addr).await?;
    session.send(&ClientMessage::SyncRequest(payload)).await?;

    let resp = session.recv().await?;
    match resp {
        HostMessage::SyncResponse(peer_payload) => {
            // 相手のペイロードを自分の public_key で検証（同一ユーザー確認）
            if !peer_payload.verify_with(&identity.public_key()) {
                return Err(ClientError::InvalidSignature);
            }
            let peer_snapshot = AppConfig {
                user_id: config.user_id.clone(),
                my_devices: peer_payload.my_devices,
                friends: peer_payload.friends,
            };
            let changed = config.merge_from_peer(&peer_snapshot);
            Ok(SyncOutcome { changed })
        }
        HostMessage::Rejected { reason } => Err(ClientError::Rejected(reason)),
        _ => Err(ClientError::Protocol("expected SyncResponse".into())),
    }
}
