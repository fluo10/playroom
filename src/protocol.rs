use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::identity::{
    self, UserIdentity, UserPublicKey, SIGNATURE_LENGTH, USER_PUBLIC_KEY_LENGTH,
};

/// プロトコルバージョン。互換性破壊変更時にインクリメント。
pub const PROTOCOL_VERSION: u32 = 1;

/// ルーム内メンバー情報（表示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub name: String,
    /// ユーザー公開鍵（32 バイト）。複数デバイスで同じ値になる。
    pub user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
    /// デバイスを識別する EndpointId の生バイト（32 バイト）
    pub endpoint_id: Vec<u8>,
}

/// ルーム概要（QueryRoomResponse 内で使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSummary {
    pub name: String,
    /// メンバーの表示名リスト
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
    pub name: String,
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
        name: String,
        endpoint_addrs: Vec<EndpointAddr>,
        version: u64,
    ) -> Self {
        let user_public_key = identity.public_key();
        let payload = SignedPayload {
            user_public_key,
            name: &name,
            endpoint_addrs: &endpoint_addrs,
            version,
        };
        let bytes = postcard::to_allocvec(&payload).expect("postcard serialize");
        let signature = identity.sign(&bytes);
        Self {
            user_public_key,
            name,
            endpoint_addrs,
            version,
            signature,
        }
    }

    /// 署名を検証する。
    pub fn verify(&self) -> bool {
        let payload = SignedPayload {
            user_public_key: self.user_public_key,
            name: &self.name,
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
    name: &'a str,
    endpoint_addrs: &'a [EndpointAddr],
    version: u64,
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
        name: String,
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
        name: String,
        user_public_key: [u8; USER_PUBLIC_KEY_LENGTH],
    },
    RoomClosed,
    Rejected { reason: String },

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
            "Alice".to_string(),
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
            "Alice".to_string(),
            vec![],
            1,
        );
        // 名前を書き換えると署名検証が失敗する
        info.name = "Mallory".to_string();
        assert!(!info.verify());
    }

    #[test]
    fn friend_self_info_wrong_key_rejected() {
        let alice = UserIdentity::generate();
        let eve = UserIdentity::generate();
        let mut info = FriendSelfInfo::sign_new(
            &alice,
            "Alice".to_string(),
            vec![],
            1,
        );
        // 署名はAliceのものだが公開鍵をEveのものに差し替え → 検証失敗
        info.user_public_key = eve.public_key();
        assert!(!info.verify());
    }
}
