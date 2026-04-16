//! デバイスペアリング（クライアント側）
//!
//! 既存デバイスが `Invite`（EndpointId + 短命シークレットを Base32 で連結した
//! 固定長 77 文字のコード）を発行し、新規デバイスがそれを入力してペアリング要求
//! を送る。成功すればユーザー秘密鍵・user_id・フレンド/デバイスリストが返される。

use std::fmt;
use std::str::FromStr;

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, EndpointId};

use crate::client::ClientError;
use crate::config::{AppConfig, FriendEntry, MyDeviceEntry};
use crate::identity::UserIdentity;
use crate::protocol::{ClientMessage, HostMessage};
use crate::session::PeerSession;
use crate::NetworkNode;

/// ペアリング招待に使う短命シークレットのバイト長（128 bit）
pub const INVITE_SECRET_LEN: usize = 16;
/// Invite エンコード時の文字数（固定 77 文字：48 バイトの Base32 NOPAD）
pub const INVITE_CODE_LEN: usize = 77;

/// 招待コード。既存デバイスが発行し新規デバイスに手渡す。
///
/// エンコード形式：`endpoint_id` 32 バイト + `secret` 16 バイトを連結した
/// 48 バイトを BASE32_NOPAD（RFC 4648、A-Z / 2-7）でエンコードした 77 文字。
/// 区切り文字なし、記号なしなのでダブルクリックで選択・コピーできる。
#[derive(Debug, Clone)]
pub struct Invite {
    /// 招待側デバイスの EndpointId（32 バイト）
    pub endpoint_id: [u8; 32],
    /// 短命シークレット（128 bit）
    pub secret: [u8; INVITE_SECRET_LEN],
}

impl Invite {
    /// 新しい招待コードを生成する。EndpointId は呼び出し元（既存デバイス）が
    /// 与え、secret は OS RNG から 16 バイトを取る。
    pub fn generate(endpoint_id: [u8; 32]) -> Self {
        let mut secret = [0u8; INVITE_SECRET_LEN];
        getrandom::fill(&mut secret).expect("OS RNG must be available");
        Self {
            endpoint_id,
            secret,
        }
    }

    /// iroh `EndpointAddr` に変換する（接続先）
    pub fn endpoint_addr(&self) -> Result<EndpointAddr, InviteError> {
        let eid = EndpointId::from_bytes(&self.endpoint_id)
            .map_err(|_| InviteError::InvalidEndpointId)?;
        Ok(EndpointAddr::from(eid))
    }
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = [0u8; 48];
        bytes[..32].copy_from_slice(&self.endpoint_id);
        bytes[32..].copy_from_slice(&self.secret);
        write!(f, "{}", BASE32_NOPAD.encode(&bytes))
    }
}

impl FromStr for Invite {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != INVITE_CODE_LEN {
            return Err(InviteError::InvalidLength);
        }
        let bytes = BASE32_NOPAD
            .decode(s.as_bytes())
            .map_err(|_| InviteError::InvalidEncoding)?;
        if bytes.len() != 48 {
            return Err(InviteError::InvalidLength);
        }
        let mut endpoint_id = [0u8; 32];
        let mut secret = [0u8; INVITE_SECRET_LEN];
        endpoint_id.copy_from_slice(&bytes[..32]);
        secret.copy_from_slice(&bytes[32..]);
        Ok(Self {
            endpoint_id,
            secret,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InviteError {
    #[error("invite code must be exactly {} characters", INVITE_CODE_LEN)]
    InvalidLength,
    #[error("invite code contains invalid Base32 characters")]
    InvalidEncoding,
    #[error("invite code embeds an invalid EndpointId")]
    InvalidEndpointId,
}

/// ペアリング成功時に新デバイスが得るデータ
pub struct PairedData {
    pub identity: UserIdentity,
    pub user_id: String,
    pub friends: Vec<FriendEntry>,
    pub my_devices: Vec<MyDeviceEntry>,
}

/// 新規デバイスとして既存デバイスにペアリング要求を送り、秘密鍵と設定を受け取る。
pub async fn pair_as_new_device(
    node: &NetworkNode,
    invite: &Invite,
    device_label: String,
) -> Result<PairedData, ClientError> {
    let addr = invite
        .endpoint_addr()
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let mut session: PeerSession<ClientMessage<()>, HostMessage<()>> = node.connect(addr).await?;
    session
        .send(&ClientMessage::PairRequest {
            secret: invite.secret,
            device_label,
        })
        .await?;

    let resp = session.recv().await?;
    match resp {
        HostMessage::PairAccepted {
            user_secret_key,
            user_id,
            friends,
            my_devices,
        } => {
            let identity = UserIdentity::from_secret_bytes(&user_secret_key);
            Ok(PairedData {
                identity,
                user_id,
                friends,
                my_devices,
            })
        }
        HostMessage::Rejected { reason } => Err(ClientError::Rejected(reason)),
        _ => Err(ClientError::Protocol("expected PairAccepted".into())),
    }
}

/// `PairedData` を永続化する。
///
/// - `identity.key` に秘密鍵を書き込む（既存があれば上書き）
/// - `AppConfig` の user_id / friends / my_devices を受信内容で上書きし、自デバイスの
///   `own_endpoint_id` + `own_label` も my_devices に追加して保存する
pub fn persist_paired_data(
    data: PairedData,
    own_endpoint_id: &EndpointId,
    own_label: String,
) -> anyhow::Result<()> {
    data.identity.save()?;

    let mut config = AppConfig::load_or_default();
    config.user_id = data.user_id;
    config.friends = data.friends;
    config.my_devices = data.my_devices;
    config.upsert_my_device(hex::encode(own_endpoint_id.as_bytes()), own_label);
    config.save()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_roundtrip() {
        let original = Invite {
            endpoint_id: [0x42; 32],
            secret: [0x37; INVITE_SECRET_LEN],
        };
        let encoded = original.to_string();
        assert_eq!(encoded.len(), INVITE_CODE_LEN);
        // 記号が含まれず、ダブルクリック可能であることを確認
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric()));

        let parsed: Invite = encoded.parse().expect("round-trip");
        assert_eq!(parsed.endpoint_id, original.endpoint_id);
        assert_eq!(parsed.secret, original.secret);
    }

    #[test]
    fn invite_generated_has_fixed_length() {
        let invite = Invite::generate([1u8; 32]);
        assert_eq!(invite.to_string().len(), INVITE_CODE_LEN);
    }

    #[test]
    fn invite_rejects_wrong_length() {
        assert!(matches!(
            "SHORT".parse::<Invite>(),
            Err(InviteError::InvalidLength)
        ));
    }

    #[test]
    fn invite_rejects_invalid_chars() {
        // 77 文字だが Base32 に存在しない文字 '!' を含む
        let bad: String = std::iter::repeat('!').take(INVITE_CODE_LEN).collect();
        assert!(matches!(
            bad.parse::<Invite>(),
            Err(InviteError::InvalidEncoding)
        ));
    }

    #[test]
    fn invite_secret_is_random_between_generations() {
        let a = Invite::generate([1u8; 32]);
        let b = Invite::generate([1u8; 32]);
        assert_ne!(a.secret, b.secret);
    }
}
