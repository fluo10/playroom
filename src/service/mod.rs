//! UI 非依存のアプリケーションサービス層。
//!
//! CLI と MCP の両方から呼ばれる本体。状態機械とビジネスロジックを保持し、
//! UI 向けの構造化結果（DTO）とイベントストリームを公開する。

use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::{
    self as playroom, AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedReceiver,
    FramedSender, FriendEntry, HostCommand, HostMessage, Invite, MemberInfo, NetworkNode,
    RoomEvent, RoomHost, RoomHostHandle, UserIdentity, UserPublicKeyHex,
};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

pub mod types;

pub use types::*;

pub struct AppService {
    config: Arc<Mutex<AppConfig>>,
    event_tx: mpsc::Sender<AppEvent>,
    /// RoomHost に渡す RoomEvent の中継元（Host 起動時に clone して渡す）
    room_event_tx: mpsc::Sender<RoomEvent>,
    inner: Mutex<Inner>,
}

struct Inner {
    state: AppState,
    identity: Option<Arc<UserIdentity>>,
    host_handle: Option<RoomHostHandle>,
    _host_task: Option<JoinHandle<()>>,
    guest: Option<GuestConnection>,
    _guest_recv_task: Option<JoinHandle<()>>,
    /// ゲスト参加中のメンバーリスト（MemberJoined/Left で更新）
    guest_members: Vec<MemberInfo>,
    _room_event_forwarder: Option<JoinHandle<()>>,
}

struct GuestConnection {
    sender: FramedSender<ClientMessage>,
    #[allow(dead_code)]
    room_name: String,
}

impl AppService {
    /// サービスを起動する。
    ///
    /// config に既に有効な user_id が入っていれば自動的に MainMenu へ遷移して
    /// RoomHost を起動する（MCP の initialize 相当）。
    pub async fn new(
        config: Arc<Mutex<AppConfig>>,
    ) -> AppResult<(Arc<Self>, mpsc::Receiver<AppEvent>)> {
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(64);
        let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);

        // RoomEvent → AppEvent::Room への中継
        let forwarder_tx = event_tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(ev) = room_event_rx.recv().await {
                if forwarder_tx.send(AppEvent::Room(ev)).await.is_err() {
                    break;
                }
            }
        });

        let service = Arc::new(Self {
            config: config.clone(),
            event_tx,
            room_event_tx,
            inner: Mutex::new(Inner {
                state: AppState::Uninitialized,
                identity: None,
                host_handle: None,
                _host_task: None,
                guest: None,
                _guest_recv_task: None,
                guest_members: Vec::new(),
                _room_event_forwarder: Some(forwarder),
            }),
        });

        let cfg_valid = config.lock().await.has_valid_user_id();
        if cfg_valid {
            let identity = Arc::new(
                UserIdentity::load_or_generate()
                    .map_err(|e| AppError::internal(format!("identity: {e}")))?,
            );
            service.start_room_host(identity).await?;
        }

        Ok((service, event_rx))
    }

    pub async fn state(&self) -> AppState {
        self.inner.lock().await.state
    }

    pub fn config(&self) -> &Arc<Mutex<AppConfig>> {
        &self.config
    }

    async fn start_room_host(&self, identity: Arc<UserIdentity>) -> AppResult<RoomHostHandle> {
        let (host, handle) = RoomHost::<()>::start(
            identity.clone(),
            self.config.clone(),
            self.room_event_tx.clone(),
        )
        .await
        .map_err(|e| AppError::internal(format!("host start: {e}")))?;

        let task = tokio::spawn(async move {
            if let Err(e) = host.run().await {
                tracing::error!("Host error: {e}");
            }
        });

        let mut inner = self.inner.lock().await;
        inner.identity = Some(identity);
        inner.host_handle = Some(handle.clone());
        inner._host_task = Some(task);
        inner.state = AppState::MainMenu;
        drop(inner);
        self.emit(AppEvent::StateChanged(AppState::MainMenu)).await;
        Ok(handle)
    }

    async fn emit(&self, event: AppEvent) {
        let _ = self.event_tx.send(event).await;
    }

    fn require_initialized<'a>(
        inner: &'a Inner,
    ) -> AppResult<(Arc<UserIdentity>, &'a RoomHostHandle)> {
        match (&inner.identity, &inner.host_handle) {
            (Some(id), Some(h)) => Ok((id.clone(), h)),
            _ => Err(AppError::NotInitialized),
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // 初期化系
    // ──────────────────────────────────────────────────────────────────────

    pub async fn create_user(&self, user_id: &str) -> AppResult<CreatedUser> {
        if !playroom::identity::is_valid_user_id(user_id) {
            return Err(AppError::invalid(
                "user_id must be 1-32 ASCII alphanumeric characters",
            ));
        }

        {
            let inner = self.inner.lock().await;
            if inner.host_handle.is_some() {
                return Err(AppError::AlreadyInitialized);
            }
        }

        let identity = Arc::new(
            UserIdentity::load_or_generate()
                .map_err(|e| AppError::internal(format!("identity: {e}")))?,
        );

        {
            let mut cfg = self.config.lock().await;
            cfg.user_id = user_id.to_string();
            cfg.save()
                .map_err(|e| AppError::internal(format!("save: {e}")))?;
        }

        let handle = self.start_room_host(identity.clone()).await?;

        Ok(CreatedUser {
            user_id: user_id.to_string(),
            user_public_key: identity.public_key(),
            endpoint_id: *handle.endpoint_id.as_bytes(),
        })
    }

    pub async fn pair_device(
        &self,
        invite_code: &str,
        device_label: &str,
    ) -> AppResult<PairedDeviceResult> {
        {
            let inner = self.inner.lock().await;
            if inner.host_handle.is_some() {
                return Err(AppError::AlreadyInitialized);
            }
        }

        let invite: Invite = invite_code
            .parse()
            .map_err(|e| AppError::invalid(format!("invalid invite code: {e}")))?;

        let label = if device_label.trim().is_empty() {
            "New Device".to_string()
        } else {
            device_label.to_string()
        };

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| AppError::internal(format!("bind: {e}")))?;
        let own_id = node.id();
        let data = playroom::pair_as_new_device(&node, &invite, label.clone())
            .await
            .map_err(|e| AppError::internal(format!("pair: {e}")))?;
        node.close().await;

        playroom::persist_paired_data(data, &own_id, label.clone())
            .map_err(|e| AppError::internal(format!("persist: {e}")))?;

        {
            let mut cfg = self.config.lock().await;
            *cfg = AppConfig::load_or_default();
        }

        let identity = Arc::new(
            UserIdentity::load_or_generate()
                .map_err(|e| AppError::internal(format!("identity: {e}")))?,
        );
        self.start_room_host(identity.clone()).await?;

        let user_id = self.config.lock().await.user_id.clone();
        Ok(PairedDeviceResult {
            user_id,
            user_public_key: identity.public_key(),
            device_label: label,
        })
    }

    // ──────────────────────────────────────────────────────────────────────
    // メインメニュー系
    // ──────────────────────────────────────────────────────────────────────

    pub async fn list_rooms(&self) -> AppResult<RoomListing> {
        let friends: Vec<FriendEntry> = {
            let cfg = self.config.lock().await;
            cfg.friends.iter().filter(|f| !f.tombstone).cloned().collect()
        };

        if friends.is_empty() {
            return Ok(RoomListing { rooms: Vec::new() });
        }

        let displays: Vec<String> = friends
            .iter()
            .map(|f| {
                let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
                playroom::identity::with_key_suffix(&f.user_id(), &key)
            })
            .collect();

        let mut handles = Vec::new();
        for (friend, display) in friends.into_iter().zip(displays.into_iter()) {
            handles.push(tokio::spawn(async move {
                let result = query_friend_room(&friend).await;
                (display, result)
            }));
        }

        let mut rooms = Vec::new();
        for h in handles {
            if let Ok((display, Ok(Some(summary)))) = h.await {
                rooms.push(RoomListingEntry {
                    friend_display: display,
                    room_name: summary.name,
                    members: summary.members,
                });
            }
        }
        Ok(RoomListing { rooms })
    }

    pub async fn create_room(&self, name: &str) -> AppResult<CreatedRoom> {
        let (endpoint_id_bytes, cmd_tx) = {
            let inner = self.inner.lock().await;
            let (_, handle) = Self::require_initialized(&inner)?;
            (*handle.endpoint_id.as_bytes(), handle.commands.clone())
        };

        cmd_tx
            .send(HostCommand::CreateRoom {
                name: name.to_string(),
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        let mut inner = self.inner.lock().await;
        inner.state = AppState::InRoomHost;
        drop(inner);
        self.emit(AppEvent::StateChanged(AppState::InRoomHost)).await;

        Ok(CreatedRoom {
            name: name.to_string(),
            endpoint_id: endpoint_id_bytes,
        })
    }

    pub async fn join_room(self: &Arc<Self>, friend_alias: &str) -> AppResult<JoinedRoomResult> {
        let (identity, user_id) = {
            let inner = self.inner.lock().await;
            let (identity, _) = Self::require_initialized(&inner)?;
            let user_id = self.config.lock().await.user_id.clone();
            (identity, user_id)
        };

        let friend = {
            let cfg = self.config.lock().await;
            find_friend_by_alias(&cfg, friend_alias).cloned()
        }
        .ok_or_else(|| AppError::invalid(format!("Friend '{friend_alias}' not found")))?;

        let addr = friend
            .cached_self_info
            .as_ref()
            .and_then(|info| info.endpoint_addrs.first().cloned())
            .ok_or_else(|| {
                AppError::internal(
                    "no known endpoint address for this friend; try list_rooms first",
                )
            })?;

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| AppError::internal(format!("Bind error: {e}")))?;

        let joined = playroom::join_room::<()>(&node, addr, &identity, user_id)
            .await
            .map_err(|e| AppError::internal(format!("Join error: {e}")))?;

        {
            let mut cfg = self.config.lock().await;
            let _ = cfg.update_friend_self_info(joined.host_self_info);
            let _ = cfg.save();
        }

        let room_name = joined.room_name.clone();
        let members = joined.members.clone();

        let recv_task = spawn_guest_recv_loop(joined.receiver, Arc::downgrade(self));

        let mut inner = self.inner.lock().await;
        inner.guest = Some(GuestConnection {
            sender: joined.sender,
            room_name: room_name.clone(),
        });
        inner._guest_recv_task = Some(recv_task);
        inner.guest_members = members.clone();
        inner.state = AppState::InRoomGuest;
        drop(inner);
        self.emit(AppEvent::StateChanged(AppState::InRoomGuest)).await;

        Ok(JoinedRoomResult { room_name, members })
    }

    pub async fn list_friends(&self) -> AppResult<FriendListing> {
        let cfg = self.config.lock().await;
        let friends = cfg
            .friends
            .iter()
            .filter(|f| !f.tombstone)
            .map(|f| {
                let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
                FriendInfo {
                    user_id: f.user_id(),
                    user_public_key: key,
                    self_info: f.cached_self_info.clone(),
                }
            })
            .collect();
        Ok(FriendListing { friends })
    }

    pub async fn add_friend(&self, endpoint_id_hex: &str) -> AppResult<FriendInfo> {
        let addr = parse_endpoint_addr(endpoint_id_hex)?;
        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| AppError::internal(format!("Bind error: {e}")))?;
        let result = playroom::query_room::<()>(&node, addr)
            .await
            .map_err(|e| AppError::internal(format!("Query error: {e}")))?;
        node.close().await;

        let key_hex = UserPublicKeyHex::from_bytes(&result.host_self_info.user_public_key);
        let user_id = result.host_self_info.user_id.clone();
        let user_public_key = result.host_self_info.user_public_key;
        let self_info = result.host_self_info.clone();

        let mut cfg = self.config.lock().await;
        cfg.upsert_friend(key_hex);
        let _ = cfg.update_friend_self_info(result.host_self_info);
        cfg.save()
            .map_err(|e| AppError::internal(format!("Save error: {e}")))?;

        Ok(FriendInfo {
            user_id,
            user_public_key,
            self_info: Some(self_info),
        })
    }

    pub async fn list_devices(&self) -> AppResult<DeviceListing> {
        let cfg = self.config.lock().await;
        let devices = cfg
            .my_devices
            .iter()
            .filter(|d| !d.tombstone)
            .map(|d| {
                let short = playroom::identity::short_bytes_from_hex(&d.endpoint_id)
                    .unwrap_or_else(|| "invalid".into());
                DeviceInfo {
                    label: d.label.clone(),
                    endpoint_id_hex: d.endpoint_id.clone(),
                    endpoint_id_short: short,
                }
            })
            .collect();
        Ok(DeviceListing { devices })
    }

    pub async fn pair_init(&self) -> AppResult<PairInitResult> {
        let (endpoint_id_bytes, cmd_tx) = {
            let inner = self.inner.lock().await;
            let (_, handle) = Self::require_initialized(&inner)?;
            (*handle.endpoint_id.as_bytes(), handle.commands.clone())
        };

        let invite = Invite::generate(endpoint_id_bytes);
        let ttl = Duration::from_secs(120);
        cmd_tx
            .send(HostCommand::OpenPairingWindow {
                secret: invite.secret,
                ttl,
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        Ok(PairInitResult {
            invite_code: invite.to_string(),
            ttl_secs: ttl.as_secs(),
        })
    }

    pub async fn sync_device(&self, endpoint_id_hex: &str) -> AppResult<SyncResult> {
        let addr = parse_endpoint_addr(endpoint_id_hex)?;
        let identity = {
            let inner = self.inner.lock().await;
            let (identity, _) = Self::require_initialized(&inner)?;
            identity
        };

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| AppError::internal(format!("Bind error: {e}")))?;

        let outcome = {
            let mut cfg = self.config.lock().await;
            let outcome = playroom::sync_with_peer(&node, addr, &identity, &mut cfg)
                .await
                .map_err(|e| AppError::internal(format!("Sync error: {e}")))?;
            if outcome.changed {
                let _ = cfg.save();
            }
            outcome
        };
        node.close().await;

        Ok(SyncResult {
            changed: outcome.changed,
        })
    }

    // ──────────────────────────────────────────────────────────────────────
    // ルーム内系
    // ──────────────────────────────────────────────────────────────────────

    pub async fn send_message(&self, content: &str) -> AppResult<()> {
        let mut inner = self.inner.lock().await;
        match inner.state {
            AppState::InRoomHost => {
                let cmd_tx = inner
                    .host_handle
                    .as_ref()
                    .ok_or_else(|| AppError::internal("no host"))?
                    .commands
                    .clone();
                drop(inner);
                cmd_tx
                    .send(HostCommand::Chat {
                        content: content.to_string(),
                    })
                    .await
                    .map_err(|e| AppError::internal(e.to_string()))?;
                Ok(())
            }
            AppState::InRoomGuest => {
                let conn = inner.guest.as_mut().ok_or(AppError::NotInRoom)?;
                conn.sender
                    .send(&ClientMessage::<()>::Chat {
                        content: content.to_string(),
                    })
                    .await
                    .map_err(|e| AppError::internal(format!("Send error: {e}")))?;
                Ok(())
            }
            _ => Err(AppError::NotInRoom),
        }
    }

    pub async fn leave_room(&self) -> AppResult<()> {
        let mut inner = self.inner.lock().await;
        match inner.state {
            AppState::InRoomHost => {
                let cmd_tx = inner
                    .host_handle
                    .as_ref()
                    .ok_or_else(|| AppError::internal("no host"))?
                    .commands
                    .clone();
                inner.state = AppState::MainMenu;
                drop(inner);
                cmd_tx
                    .send(HostCommand::CloseRoom)
                    .await
                    .map_err(|e| AppError::internal(e.to_string()))?;
                self.emit(AppEvent::StateChanged(AppState::MainMenu)).await;
                Ok(())
            }
            AppState::InRoomGuest => {
                if let Some(mut conn) = inner.guest.take() {
                    let _ = conn.sender.send(&ClientMessage::<()>::Leave).await;
                }
                inner._guest_recv_task = None;
                inner.guest_members.clear();
                inner.state = AppState::MainMenu;
                drop(inner);
                self.emit(AppEvent::StateChanged(AppState::MainMenu)).await;
                Ok(())
            }
            _ => Err(AppError::NotInRoom),
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // 内部：guest 側の受信処理
    // ──────────────────────────────────────────────────────────────────────

    /// ゲスト受信ループから呼ばれる、ルーム関連メッセージの内部処理。
    /// 状態更新後に AppEvent::Guest を emit する。
    async fn on_guest_message(&self, msg: HostMessage) {
        let mut inner = self.inner.lock().await;
        match &msg {
            HostMessage::MemberJoined(info) => {
                inner.guest_members.push(info.clone());
            }
            HostMessage::MemberLeft { user_id, .. } => {
                inner.guest_members.retain(|m| m.user_id != *user_id);
            }
            HostMessage::RoomClosed => {
                inner.guest = None;
                inner.guest_members.clear();
                if inner.state == AppState::InRoomGuest {
                    inner.state = AppState::MainMenu;
                }
            }
            _ => {}
        }
        let is_closed = matches!(msg, HostMessage::RoomClosed);
        drop(inner);
        self.emit(AppEvent::Guest(msg)).await;
        if is_closed {
            self.emit(AppEvent::StateChanged(AppState::MainMenu)).await;
        }
    }
}

fn spawn_guest_recv_loop(
    mut receiver: FramedReceiver<HostMessage>,
    service: Weak<AppService>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let msg = match receiver.recv().await {
                Ok(m) => m,
                Err(_) => HostMessage::RoomClosed,
            };
            let is_closed = matches!(msg, HostMessage::RoomClosed);
            let Some(svc) = service.upgrade() else { break };
            svc.on_guest_message(msg).await;
            if is_closed {
                break;
            }
        }
    })
}

fn parse_endpoint_addr(endpoint_id_hex: &str) -> AppResult<EndpointAddr> {
    let bytes = hex::decode(endpoint_id_hex)
        .map_err(|e| AppError::invalid(format!("Invalid hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(AppError::invalid(
            "EndpointId must be 32 bytes (64 hex chars)",
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    let eid = EndpointId::from_bytes(&arr)
        .map_err(|e| AppError::invalid(format!("Invalid key: {e}")))?;
    Ok(EndpointAddr::from(eid))
}

fn find_friend_by_alias<'a>(config: &'a AppConfig, alias: &str) -> Option<&'a FriendEntry> {
    if let Some(f) = config.friends.iter().find(|f| {
        !f.tombstone
            && f.cached_self_info
                .as_ref()
                .map(|i| i.user_id == alias)
                .unwrap_or(false)
    }) {
        return Some(f);
    }
    config
        .friends
        .iter()
        .find(|f| !f.tombstone && f.user_public_key.0.starts_with(alias))
}

async fn query_friend_room(
    friend: &FriendEntry,
) -> anyhow::Result<Option<playroom::RoomSummary>> {
    let addr = friend
        .cached_self_info
        .as_ref()
        .and_then(|info| info.endpoint_addrs.first().cloned())
        .ok_or_else(|| anyhow::anyhow!("no known endpoint address"))?;

    let node = NetworkNode::bind(None).await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        playroom::query_room::<()>(&node, addr),
    )
    .await
    .map_err(|_| anyhow::anyhow!("query timeout"))??;
    node.close().await;

    Ok(result.room)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppConfig;

    /// 未初期化 config からサービスを立てると、state は Uninitialized で、
    /// disk I/O は一切発生しない（has_valid_user_id が false を返すので早期 return）。
    #[tokio::test]
    async fn new_with_empty_config_stays_uninitialized() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        assert_eq!(service.state().await, AppState::Uninitialized);
    }

    #[tokio::test]
    async fn send_message_without_room_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.send_message("hi").await.unwrap_err();
        assert!(matches!(err, AppError::NotInRoom));
    }

    #[tokio::test]
    async fn leave_room_without_room_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.leave_room().await.unwrap_err();
        assert!(matches!(err, AppError::NotInRoom));
    }

    #[tokio::test]
    async fn list_rooms_uninitialized_returns_empty_not_error() {
        // Uninitialized 時に list_rooms は NotInitialized を返さず、
        // friends が空なので空リストを返す（メインメニュー前でも参照可能）。
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let listing = service.list_rooms().await.unwrap();
        assert!(listing.rooms.is_empty());
    }

    #[tokio::test]
    async fn create_room_uninitialized_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.create_room("test").await.unwrap_err();
        assert!(matches!(err, AppError::NotInitialized));
    }

    #[tokio::test]
    async fn pair_init_uninitialized_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.pair_init().await.unwrap_err();
        assert!(matches!(err, AppError::NotInitialized));
    }

    #[tokio::test]
    async fn sync_device_uninitialized_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        // 不正な endpoint_id でも NotInitialized が先に出る
        let err = service
            .sync_device("00".repeat(32).as_str())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotInitialized));
    }

    #[tokio::test]
    async fn join_room_uninitialized_errors() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.join_room("alice").await.unwrap_err();
        assert!(matches!(err, AppError::NotInitialized));
    }

    #[tokio::test]
    async fn create_user_rejects_invalid_user_id() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.create_user("").await.unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_)));

        let err = service.create_user("has spaces").await.unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_)));

        // 33 文字は上限超過
        let err = service.create_user(&"a".repeat(33)).await.unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_)));
    }

    #[tokio::test]
    async fn pair_device_rejects_invalid_invite() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service
            .pair_device("not-a-real-invite", "Laptop")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_)));
    }

    #[tokio::test]
    async fn add_friend_rejects_invalid_endpoint_hex() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        let err = service.add_friend("not-hex").await.unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_)));

        let err = service.add_friend("00ff").await.unwrap_err();
        assert!(matches!(err, AppError::InvalidArg(_))); // 長さ不正
    }

    #[tokio::test]
    async fn list_friends_and_devices_empty_when_uninitialized() {
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (service, _events) = AppService::new(config).await.unwrap();
        assert!(service.list_friends().await.unwrap().friends.is_empty());
        assert!(service.list_devices().await.unwrap().devices.is_empty());
    }
}
