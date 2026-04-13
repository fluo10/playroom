use serde::{Deserialize, Serialize};

/// ルーム内メンバー情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub name: String,
    /// EndpointId::as_bytes() の 32 バイト
    pub endpoint_id: Vec<u8>,
}

/// 参加者 → ホスト
///
/// `T` はアプリ固有のルーム内メッセージ型。
/// playroom 共通のメッセージ（QueryRoom, Join, Leave, Chat）はここに定義し、
/// ゲーム別メッセージは `InRoom(T)` で運ぶ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage<T = ()> {
    // ── ルーム外（共通） ──
    /// ルーム情報の問い合わせ（応答後切断）
    QueryRoom,
    /// ルーム参加
    Join { name: String },
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
///
/// `T` はアプリ固有のルーム内メッセージ型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMessage<T = ()> {
    // ── ルーム情報応答 ──
    /// ルーム情報（QueryRoom の応答）
    RoomInfo {
        name: String,
        members: Vec<String>,
        host_name: String,
    },
    /// ルーム未作成時の QueryRoom 応答
    NotHosting,

    // ── ルーム管理 ──
    /// 参加成功
    Welcome {
        room_name: String,
        members: Vec<MemberInfo>,
    },
    /// メンバーが参加した
    MemberJoined(MemberInfo),
    /// メンバーが退出した
    MemberLeft { name: String },
    /// ホストがルームを解散した
    RoomClosed,
    /// 操作が拒否された
    Rejected { reason: String },

    // ── ルーム内（共通） ──
    /// チャットメッセージ
    Chat { from: String, content: String },

    // ── ルーム内（アプリ固有） ──
    /// ゲーム別メッセージ
    InRoom(T),
}
