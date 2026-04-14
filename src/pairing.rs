//! デバイスペアリング（クライアント側）
//!
//! 新規デバイスが既存デバイスからユーザー秘密鍵・名前・フレンド/デバイス一覧を
//! 受け取るための手順をまとめる。OTP は 6 桁数字を想定。
//!
//! 手順：
//!   1. 既存デバイスが `HostCommand::OpenPairingWindow { otp, ttl }` を発行して
//!      ウィンドウを開く
//!   2. 新規デバイスで `pair_as_new_device(...)` を呼ぶ
//!   3. 成功すると `PairedData` が返る。呼び出し元が AppConfig と identity.key
//!      を保存する

use iroh::EndpointAddr;

use crate::client::ClientError;
use crate::config::{AppConfig, FriendEntry, MyDeviceEntry};
use crate::identity::UserIdentity;
use crate::protocol::{ClientMessage, HostMessage};
use crate::session::PeerSession;
use crate::NetworkNode;

/// ペアリング成功時に新デバイスが得るデータ
pub struct PairedData {
    pub identity: UserIdentity,
    pub user_id: String,
    pub friends: Vec<FriendEntry>,
    pub my_devices: Vec<MyDeviceEntry>,
}

/// 6 桁数字の OTP を生成する
pub fn generate_otp() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("OS RNG must be available");
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{:06}", n)
}

/// 新規デバイスとして既存デバイスにペアリング要求を送り、秘密鍵と設定を受け取る。
///
/// この関数が返った後、呼び出し元はこのデバイスの `EndpointId` を `my_devices`
/// に追加する責務がある（既存デバイス側でも同時に追加される）。
pub async fn pair_as_new_device(
    node: &NetworkNode,
    addr: EndpointAddr,
    otp: String,
    device_label: String,
) -> Result<PairedData, ClientError> {
    let mut session: PeerSession<ClientMessage<()>, HostMessage<()>> = node.connect(addr).await?;
    session
        .send(&ClientMessage::PairRequest { otp, device_label })
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
    own_endpoint_id: &iroh::EndpointId,
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
    fn otp_is_six_digits() {
        for _ in 0..100 {
            let otp = generate_otp();
            assert_eq!(otp.len(), 6);
            assert!(otp.chars().all(|c| c.is_ascii_digit()));
        }
    }
}
