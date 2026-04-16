use playroom::{FriendSelfInfo, HostMessage, MemberInfo, RoomEvent, UserPublicKey};

/// AppService の状態。MCP の tool セットや CLI のプロンプトを切り替える材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Uninitialized,
    MainMenu,
    InRoomHost,
    InRoomGuest,
}

/// UI 向けイベント。CLI/MCP が購読する。
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// ホストしているルームで発生したイベント
    Room(RoomEvent),
    /// ゲスト参加中に受信したホストからのメッセージ
    Guest(HostMessage),
    /// 状態遷移（MCP の tool_list_changed、CLI のプロンプト切替に使用）
    StateChanged(AppState),
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidArg(String),
    #[error("not initialized; call create_user or pair_device first")]
    NotInitialized,
    #[error("already initialized")]
    AlreadyInitialized,
    #[error("not in a room")]
    NotInRoom,
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn invalid<S: Into<String>>(s: S) -> Self {
        Self::InvalidArg(s.into())
    }
    pub fn internal<S: Into<String>>(s: S) -> Self {
        Self::Internal(s.into())
    }
}

pub type AppResult<T> = Result<T, AppError>;

// ── メソッド戻り値の DTO ──

#[derive(Debug, Clone)]
pub struct CreatedUser {
    pub user_id: String,
    pub user_public_key: [u8; 32],
    pub endpoint_id: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct PairedDeviceResult {
    pub user_id: String,
    pub user_public_key: [u8; 32],
    pub device_label: String,
}

#[derive(Debug, Clone)]
pub struct CreatedRoom {
    pub name: String,
    pub endpoint_id: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct JoinedRoomResult {
    pub room_name: String,
    pub members: Vec<MemberInfo>,
}

#[derive(Debug, Clone)]
pub struct RoomListing {
    pub rooms: Vec<RoomListingEntry>,
}

#[derive(Debug, Clone)]
pub struct RoomListingEntry {
    pub friend_display: String,
    pub room_name: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FriendListing {
    pub friends: Vec<FriendInfo>,
}

#[derive(Debug, Clone)]
pub struct FriendInfo {
    pub user_id: String,
    pub user_public_key: UserPublicKey,
    pub self_info: Option<FriendSelfInfo>,
}

#[derive(Debug, Clone)]
pub struct DeviceListing {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub label: String,
    pub endpoint_id_hex: String,
    pub endpoint_id_short: String,
}

#[derive(Debug, Clone)]
pub struct PairInitResult {
    pub invite_code: String,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub changed: bool,
}
