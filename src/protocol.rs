use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::config::{FriendEntry, MyDeviceEntry};
use crate::identity::{
    self, UserIdentity, UserPublicKey, SIGNATURE_LENGTH, USER_PUBLIC_KEY_LENGTH,
};

/// プロトコルバージョン。互換性破壊変更時にインクリメント。
pub const PROTOCOL_VERSION: u32 = 1;

/// ルーム内メンバー情報（表示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    /// ユーザーが自分で決めた識別子（英数字、表示時に衝突したら鍵プレフィクスで区別）
    pub user_id: String,
    /// ユーザー公開鍵（32 バイト）。複数デバイスで同じ値になる。
    pub user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
    /// デバイスを識別する EndpointId の生バイト（32 バイト）
    pub endpoint_id: Vec<u8>,
}

/// ルーム概要（QueryRoomResponse 内で使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSummary {
    /// ルーム名
    pub name: String,
    /// メンバーの user_id リスト（衝突解決前）
    pub members: Vec<String>,
}

/// フレンドが自分自身について発信する情報。
///
/// 書き手はフレンド自身（秘密鍵保有者）のみ。`version` を単調増加させ、
/// 署名は `user_public_key || name || endpoint_addrs || version` の
/// postcard エンコード結果に対して行う。
///
/// 受信側は `verify()` で検証してから信頼する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendSelfInfo {
    pub user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
    /// このユーザー自身が決めた user_id（英数字）
    pub user_id: String,
    /// このユーザーが所有する iroh デバイスの EndpointAddr 一覧
    pub endpoint_addrs: Vec<EndpointAddr>,
    /// フレンド側で単調増加するバージョン番号（unix ms 等）
    pub version: u64,
    /// 上記フィールドに対する署名（user_public_key で検証）
    #[serde(with = "BigArray")]
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl FriendSelfInfo {
    /// 新規に自分自身の情報を署名付きで作成する。
    pub fn sign_new(
        identity: &UserIdentity,
        user_id: String,
        endpoint_addrs: Vec<EndpointAddr>,
        version: u64,
    ) -> Self {
        let user_public_key = identity.public_key();
        let payload = SignedPayload {
            user_public_key,
            user_id: &user_id,
            endpoint_addrs: &endpoint_addrs,
            version,
        };
        let bytes = postcard::to_allocvec(&payload).expect("postcard serialize");
        let signature = identity.sign(&bytes);
        Self {
            user_public_key,
            user_id,
            endpoint_addrs,
            version,
            signature,
        }
    }

    /// 署名を検証する。
    pub fn verify(&self) -> bool {
        let payload = SignedPayload {
            user_public_key: self.user_public_key,
            user_id: &self.user_id,
            endpoint_addrs: &self.endpoint_addrs,
            version: self.version,
        };
        let Ok(bytes) = postcard::to_allocvec(&payload) else {
            return false;
        };
        identity::verify_signature(&self.user_public_key, &bytes, &self.signature)
    }
}

/// 署名対象のペイロード（シリアライズ専用、内部型）
#[derive(Serialize)]
struct SignedPayload<'a> {
    user_public_key: UserPublicKey,
    user_id: &'a str,
    endpoint_addrs: &'a [EndpointAddr],
    version: u64,
}

/// デバイス間同期ペイロード（署名付き）。
///
/// 同一ユーザーのデバイスだけが署名できる（user_secret_key を保有している）。
/// 受信側は自分の user_public_key で検証することで「同一ユーザーの別デバイス
/// からの正当な同期要求」であることを確認する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub my_devices: Vec<MyDeviceEntry>,
    pub friends: Vec<FriendEntry>,
    #[serde(with = "BigArray")]
    pub signature: [u8; SIGNATURE_LENGTH],
}

const SYNC_SIGN_DOMAIN: &[u8] = b"playroom-sync-v1";

impl SyncPayload {
    pub fn sign_new(
        identity: &UserIdentity,
        my_devices: Vec<MyDeviceEntry>,
        friends: Vec<FriendEntry>,
    ) -> Self {
        let bytes = Self::signing_bytes(&my_devices, &friends);
        let signature = identity.sign(&bytes);
        Self {
            my_devices,
            friends,
            signature,
        }
    }

    /// 指定されたユーザー公開鍵で検証する。
    pub fn verify_with(&self, user_public_key: &UserPublicKey) -> bool {
        let bytes = Self::signing_bytes(&self.my_devices, &self.friends);
        identity::verify_signature(user_public_key, &bytes, &self.signature)
    }

    fn signing_bytes(my_devices: &[MyDeviceEntry], friends: &[FriendEntry]) -> Vec<u8> {
        #[derive(Serialize)]
        struct Body<'a> {
            domain: &'a [u8],
            my_devices: &'a [MyDeviceEntry],
            friends: &'a [FriendEntry],
        }
        let body = Body {
            domain: SYNC_SIGN_DOMAIN,
            my_devices,
            friends,
        };
        postcard::to_allocvec(&body).expect("postcard serialize")
    }
}

// ───────────────────────────────────────────────────────────────────────────
// ルームプロトコル
// ───────────────────────────────────────────────────────────────────────────

/// 参加者 → ホスト
///
/// `T` はアプリ固有のルーム内メッセージ型。
/// playroom 共通のメッセージはここに定義し、ゲーム別メッセージは `InRoom(T)` で運ぶ。
///
/// 接続時の手順：
///   1. Client → `QueryRoom` もしくは `JoinRequest { ... }`
///   2. QueryRoom の場合: Host が `RoomInfo` / `NotHosting` を返して切断
///   3. JoinRequest の場合: Host が `Challenge { nonce }` を返す
///   4. Client → `JoinResponse { signature }`
///   5. Host が署名検証 → `Welcome` または `Rejected`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage<T = ()> {
    // ── ルーム外（共通） ──
    /// ルーム情報の問い合わせ（応答後切断）
    QueryRoom,
    /// ルーム参加要求（チャレンジを受け取るため）
    JoinRequest {
        /// 参加する際に表示される user_id（英数字）
        user_id: String,
        user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
        protocol_version: u32,
    },
    /// チャレンジへの署名応答
    JoinResponse {
        #[serde(with = "BigArray")]
        signature: [u8; SIGNATURE_LENGTH],
    },
    /// 退出
    Leave,

    // ── デバイス間ペアリング ──
    /// 新デバイスから既存デバイスへのペアリング要求（first message）。
    /// OTP は 6 桁数字を想定。
    PairRequest {
        otp: String,
        device_label: String,
    },

    // ── デバイス間同期 ──
    /// 同期要求（first message）。相手は自分の user_public_key で署名検証する。
    SyncRequest(SyncPayload),

    // ── ルーム内（共通） ──
    /// チャットメッセージ
    Chat { content: String },

    // ── ルーム内（アプリ固有） ──
    /// ゲーム別メッセージ
    InRoom(T),
}

/// ホスト → 参加者
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMessage<T = ()> {
    // ── ルーム情報応答 ──
    /// QueryRoom への応答。ホストのアイデンティティ（署名付き）と、ホスト中の
    /// ルーム情報（ホストしていなければ None）を返す。
    QueryRoomResponse {
        host_self_info: FriendSelfInfo,
        room: Option<RoomSummary>,
    },

    // ── 認証 ──
    /// 参加要求に対するチャレンジ（クライアントは秘密鍵で署名して返す）
    Challenge { nonce: [u8; 32] },

    // ── ルーム管理 ──
    Welcome {
        room_name: String,
        members: Vec<MemberInfo>,
        /// ホスト自身の FriendSelfInfo（接続したクライアント側で保存）
        host_self_info: FriendSelfInfo,
    },
    MemberJoined(MemberInfo),
    MemberLeft {
        user_id: String,
        user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
    },
    RoomClosed,
    Rejected { reason: String },

    // ── ペアリング応答 ──
    /// OTP 検証成功。ユーザー秘密鍵・名前・フレンド/デバイスリストを同梱して
    /// 新デバイスに引き渡す。iroh QUIC による暗号化通信上で送信。
    PairAccepted {
        #[serde(with = "BigArray")]
        user_secret_key: [u8; 32],
        user_id: String,
        friends: Vec<FriendEntry>,
        my_devices: Vec<MyDeviceEntry>,
    },

    // ── 同期応答 ──
    SyncResponse(SyncPayload),

    // ── ルーム内（共通） ──
    Chat { from: String, content: String },

    // ── ルーム内（アプリ固有） ──
    InRoom(T),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::UserIdentity;

    #[test]
    fn friend_self_info_sign_verify() {
        let id = UserIdentity::generate();
        let info = FriendSelfInfo::sign_new(
            &id,
            "alice".to_string(),
            vec![],
            42,
        );
        assert!(info.verify());
    }

    #[test]
    fn friend_self_info_tampered_rejected() {
        let id = UserIdentity::generate();
        let mut info = FriendSelfInfo::sign_new(
            &id,
            "alice".to_string(),
            vec![],
            1,
        );
        // user_id を書き換えると署名検証が失敗する
        info.user_id = "mallory".to_string();
        assert!(!info.verify());
    }

    #[test]
    fn sync_payload_sign_verify() {
        let id = UserIdentity::generate();
        let payload = SyncPayload::sign_new(&id, vec![], vec![]);
        assert!(payload.verify_with(&id.public_key()));

        // 別人の公開鍵では検証失敗
        let other = UserIdentity::generate();
        assert!(!payload.verify_with(&other.public_key()));
    }

    #[test]
    fn sync_payload_tampered_rejected() {
        use crate::config::{FriendEntry, MyDeviceEntry};
        let id = UserIdentity::generate();
        let mut payload = SyncPayload::sign_new(&id, vec![], vec![]);
        // 内容を後から改竄すると署名検証が失敗する
        payload.my_devices.push(MyDeviceEntry {
            endpoint_id: "dev".into(),
            label: "Evil".into(),
            added_at: 1,
            last_modified: 1,
            tombstone: false,
        });
        assert!(!payload.verify_with(&id.public_key()));
    }

    #[test]
    fn friend_self_info_wrong_key_rejected() {
        let alice = UserIdentity::generate();
        let eve = UserIdentity::generate();
        let mut info = FriendSelfInfo::sign_new(
            &alice,
            "alice".to_string(),
            vec![],
            1,
        );
        // 署名はAliceのものだが公開鍵をEveのものに差し替え → 検証失敗
        info.user_public_key = eve.public_key();
        assert!(!info.verify());
    }
}
