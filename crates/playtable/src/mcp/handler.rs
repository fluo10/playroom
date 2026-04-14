use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use playroom::{
    self, AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedSender, FriendEntry,
    HostCommand, MemberInfo, NetworkNode, RoomEvent, RoomHostHandle, RoomSummary,
    UserIdentity, UserPublicKeyHex,
};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, Peer, RoleServer, ServerHandler};
use serde_json::Value;
use tokio::sync::Mutex;

/// MCP の状態
#[derive(Debug, Clone, PartialEq)]
enum McpState {
    Initial,
    MainMenu,
    InRoomHost,
    InRoomGuest,
}

struct GuestConnection {
    sender: FramedSender<ClientMessage>,
    #[allow(dead_code)]
    room_name: String,
    #[allow(dead_code)]
    members: Vec<MemberInfo>,
}

pub struct PlaytableMcpHandler {
    name: String,
    identity: Arc<UserIdentity>,
    config: Arc<Mutex<AppConfig>>,
    inner: Mutex<HandlerInner>,
    host_handle: RoomHostHandle,
}

struct HandlerInner {
    state: McpState,
    peer: Option<Peer<RoleServer>>,
    guest_conn: Option<GuestConnection>,
}

impl PlaytableMcpHandler {
    pub fn new(
        name: String,
        config: Arc<Mutex<AppConfig>>,
        identity: Arc<UserIdentity>,
        host_handle: RoomHostHandle,
    ) -> Self {
        Self {
            name,
            identity,
            config,
            inner: Mutex::new(HandlerInner {
                state: McpState::Initial,
                peer: None,
                guest_conn: None,
            }),
            host_handle,
        }
    }

    pub async fn handle_room_event(&self, event: RoomEvent) {
        let message = match &event {
            RoomEvent::MemberJoined { user_id: name, .. } => format!("* {name} joined the room"),
            RoomEvent::MemberLeft { user_id: name, .. } => format!("* {name} left the room"),
            RoomEvent::ChatReceived { from, content } => format!("[chat] {from}: {content}"),
            RoomEvent::RoomCreated { name } => format!("* Room '{name}' created"),
            RoomEvent::RoomClosed => "* Room closed".to_string(),
            RoomEvent::PairingWindowOpened { otp } => {
                format!("* Pairing window open — OTP: {otp}")
            }
            RoomEvent::PairingWindowClosed => "* Pairing window closed".to_string(),
            RoomEvent::PairingCompleted { device_label, .. } => {
                format!("* Paired with new device: {device_label}")
            }
            RoomEvent::SyncCompleted { .. } => "* Sync completed".to_string(),
        };

        let mut inner = self.inner.lock().await;
        if let Some(peer) = &inner.peer {
            let _ = peer
                .notify_logging_message(LoggingMessageNotificationParam {
                    level: LoggingLevel::Info,
                    logger: Some("playroom".to_string()),
                    data: serde_json::json!(message),
                })
                .await;
        }

        if matches!(event, RoomEvent::RoomClosed) {
            if inner.state == McpState::InRoomGuest || inner.state == McpState::InRoomHost {
                inner.state = McpState::MainMenu;
                inner.guest_conn = None;
                if let Some(peer) = &inner.peer {
                    let _ = peer.notify_tool_list_changed().await;
                }
            }
        }
    }

    fn tools_for_state(state: &McpState) -> Vec<Tool> {
        match state {
            McpState::Initial => vec![make_tool(
                "start",
                "Start playroom and enter the main menu",
                json_schema_empty(),
            )],
            McpState::MainMenu => vec![
                make_tool("list_rooms", "List rooms hosted by friends", json_schema_empty()),
                make_tool(
                    "create_room",
                    "Create and host a new room",
                    json_schema_obj(&[("name", "string", "Room name")]),
                ),
                make_tool(
                    "join_room",
                    "Join a friend's room (by user_id or user public key hex prefix)",
                    json_schema_obj(&[("friend", "string", "Friend user_id or user public key hex prefix")]),
                ),
                make_tool("list_friends", "List registered friends", json_schema_empty()),
                make_tool(
                    "add_friend",
                    "Add a friend by EndpointId. The friend's user identity will be fetched via QueryRoom.",
                    json_schema_obj(&[(
                        "endpoint_id",
                        "string",
                        "Hex-encoded EndpointId (64 hex chars)",
                    )]),
                ),
                make_tool("list_devices", "List your own paired devices", json_schema_empty()),
                make_tool(
                    "pair_init",
                    "Open a pairing window on this (existing) device. Returns an OTP to enter on the new device.",
                    json_schema_empty(),
                ),
                make_tool(
                    "pair_complete",
                    "Use this on the NEW device to join an existing user identity. Requires the other device's EndpointId and the 6-digit OTP shown there.",
                    json_schema_obj(&[
                        ("endpoint_id", "string", "Hex-encoded EndpointId of the existing device"),
                        ("otp", "string", "6-digit OTP shown on the existing device"),
                        ("device_label", "string", "Label for this new device (e.g. 'Laptop')"),
                    ]),
                ),
                make_tool(
                    "sync_device",
                    "Sync friends/devices with another of your own devices.",
                    json_schema_obj(&[("endpoint_id", "string", "Hex-encoded EndpointId of your other device")]),
                ),
            ],
            McpState::InRoomHost | McpState::InRoomGuest => vec![
                make_tool(
                    "send_message",
                    "Send a chat message to the room",
                    json_schema_obj(&[("content", "string", "Message content")]),
                ),
                make_tool("leave_room", "Leave the current room", json_schema_empty()),
            ],
        }
    }

    async fn handle_start(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;
        inner.state = McpState::MainMenu;
        inner.peer = Some(context.peer.clone());
        let _ = context.peer.notify_tool_list_changed().await;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Playroom started! Your user public key: {}\nYour EndpointId: {}",
            hex::encode(self.identity.public_key()),
            hex::encode(self.host_handle.endpoint_id.as_bytes())
        ))]))
    }

    async fn handle_list_rooms(&self) -> Result<CallToolResult, ErrorData> {
        let friends: Vec<FriendEntry> = {
            let cfg = self.config.lock().await;
            cfg.friends.iter().filter(|f| !f.tombstone).cloned().collect()
        };

        if friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered. Use add_friend to add friends first.",
            )]));
        }

        let entries: Vec<(String, playroom::UserPublicKey)> = friends
            .iter()
            .map(|f| {
                let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
                (f.user_id(), key)
            })
            .collect();
        let display_names = entries
            .iter()
            .map(|(id, pk)| playroom::identity::with_key_suffix(id, pk))
            .collect::<Vec<_>>();

        let mut handles = Vec::new();
        for (friend, display) in friends.into_iter().zip(display_names.into_iter()) {
            handles.push(tokio::spawn(async move {
                let result = query_friend_room(&friend).await;
                (display, result)
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok((friend_name, Ok(Some(summary)))) = handle.await {
                results.push(format!(
                    "[{friend_name}] {} ({} members: {})",
                    summary.name,
                    summary.members.len(),
                    summary.members.join(", ")
                ));
            }
        }

        let text = if results.is_empty() {
            "No rooms found.".to_string()
        } else {
            format!("Available rooms:\n{}", results.join("\n"))
        };

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    async fn handle_create_room(
        &self,
        args: &Value,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("My Room");

        self.host_handle
            .commands
            .send(HostCommand::CreateRoom {
                name: name.to_string(),
            })
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut inner = self.inner.lock().await;
        inner.state = McpState::InRoomHost;
        inner.peer = Some(context.peer.clone());
        let _ = context.peer.notify_tool_list_changed().await;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Room '{}' created! Your EndpointId: {}",
            name,
            hex::encode(self.host_handle.endpoint_id.as_bytes())
        ))]))
    }

    async fn handle_join_room(
        &self,
        args: &Value,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let friend_arg = args
            .get("friend")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("friend is required", None))?;

        let friend = {
            let cfg = self.config.lock().await;
            find_friend_by_alias(&cfg, friend_arg).cloned()
        }
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("Friend '{friend_arg}' not found"), None)
        })?;

        let addr = friend
            .cached_self_info
            .as_ref()
            .and_then(|info| info.endpoint_addrs.first().cloned())
            .ok_or_else(|| {
                ErrorData::internal_error(
                    "no known endpoint address for this friend; try list_rooms first to refresh",
                    None,
                )
            })?;

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;

        let joined = playroom::join_room::<()>(&node, addr, &self.identity, self.name.clone())
            .await
            .map_err(|e| ErrorData::internal_error(format!("Join error: {e}"), None))?;

        let entries: Vec<(String, playroom::UserPublicKey)> = joined
            .members
            .iter()
            .map(|m| (m.user_id.clone(), m.user_public_key))
            .collect();
        let display_names = entries
            .iter()
            .map(|(id, pk)| playroom::identity::with_key_suffix(id, pk))
            .collect::<Vec<_>>();
        let member_list = display_names.join(", ");
        let room_name = joined.room_name.clone();

        {
            let mut cfg = self.config.lock().await;
            let _ = cfg.update_friend_self_info(joined.host_self_info);
            let _ = cfg.save();
        }

        let mut inner = self.inner.lock().await;
        inner.state = McpState::InRoomGuest;
        inner.guest_conn = Some(GuestConnection {
            sender: joined.sender,
            room_name: room_name.clone(),
            members: joined.members,
        });
        inner.peer = Some(context.peer.clone());
        let _ = context.peer.notify_tool_list_changed().await;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Joined room '{room_name}'! Members: {member_list}"
        ))]))
    }

    async fn handle_list_friends(&self) -> Result<CallToolResult, ErrorData> {
        let cfg = self.config.lock().await;
        let friends: Vec<&FriendEntry> =
            cfg.friends.iter().filter(|f| !f.tombstone).collect();
        if friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered.",
            )]));
        }

        let entries: Vec<(String, playroom::UserPublicKey)> = friends
            .iter()
            .map(|f| {
                let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
                (f.user_id(), key)
            })
            .collect();
        let display = entries
            .iter()
            .map(|(id, pk)| playroom::identity::with_key_suffix(id, pk))
            .collect::<Vec<_>>();
        let mut lines = vec!["Friends:".to_string()];
        for name in display {
            lines.push(format!("  {name}"));
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    async fn handle_add_friend(&self, args: &Value) -> Result<CallToolResult, ErrorData> {
        let endpoint_id = args
            .get("endpoint_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("endpoint_id is required", None))?;

        let addr = parse_endpoint_addr(endpoint_id)?;

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;
        let result = playroom::query_room::<()>(&node, addr)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Query error: {e}"), None))?;
        node.close().await;

        let key_hex = UserPublicKeyHex::from_bytes(&result.host_self_info.user_public_key);
        let display_user_id = result.host_self_info.user_id.clone();

        let mut cfg = self.config.lock().await;
        cfg.upsert_friend(key_hex);
        let _ = cfg.update_friend_self_info(result.host_self_info);
        cfg.save()
            .map_err(|e| ErrorData::internal_error(format!("Save error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Friend '{display_user_id}' added.",
        ))]))
    }

    async fn handle_list_devices(&self) -> Result<CallToolResult, ErrorData> {
        let cfg = self.config.lock().await;
        let devices: Vec<_> = cfg.my_devices.iter().filter(|d| !d.tombstone).collect();
        if devices.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No devices registered. Use pair_init / pair_complete to pair your devices.",
            )]));
        }
        let mut lines = vec!["Your devices:".to_string()];
        for d in devices {
            let short = playroom::identity::short_bytes_from_hex(&d.endpoint_id)
                .unwrap_or_else(|| "invalid".into());
            lines.push(format!("  {} (#{short})", d.label));
        }
        Ok(CallToolResult::success(vec![Content::text(lines.join("\n"))]))
    }

    async fn handle_pair_init(&self) -> Result<CallToolResult, ErrorData> {
        let otp = playroom::generate_otp();
        self.host_handle
            .commands
            .send(HostCommand::OpenPairingWindow {
                otp: otp.clone(),
                ttl: Duration::from_secs(120),
            })
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Pairing window open for 120s.\n\
             On the new device, run pair_complete with:\n  \
             endpoint_id: {}\n  otp: {otp}",
            hex::encode(self.host_handle.endpoint_id.as_bytes())
        ))]))
    }

    async fn handle_pair_complete(&self, args: &Value) -> Result<CallToolResult, ErrorData> {
        let endpoint_id = args
            .get("endpoint_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("endpoint_id is required", None))?;
        let otp = args
            .get("otp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("otp is required", None))?;
        let device_label = args
            .get("device_label")
            .and_then(|v| v.as_str())
            .unwrap_or("New Device")
            .to_string();

        let addr = parse_endpoint_addr(endpoint_id)?;

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;
        let own_id = node.id();
        let data = playroom::pair_as_new_device(&node, addr, otp.to_string(), device_label.clone())
            .await
            .map_err(|e| ErrorData::internal_error(format!("Pair error: {e}"), None))?;
        node.close().await;

        playroom::persist_paired_data(data, &own_id, device_label.clone())
            .map_err(|e| ErrorData::internal_error(format!("Persist error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Paired successfully! Restart the app to use the new identity.\n\
             Note: this device is labeled '{device_label}' in your device list."
        ))]))
    }

    async fn handle_sync_device(&self, args: &Value) -> Result<CallToolResult, ErrorData> {
        let endpoint_id = args
            .get("endpoint_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("endpoint_id is required", None))?;
        let addr = parse_endpoint_addr(endpoint_id)?;

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;

        let outcome = {
            let mut cfg = self.config.lock().await;
            let outcome = playroom::sync_with_peer(&node, addr, &self.identity, &mut cfg)
                .await
                .map_err(|e| ErrorData::internal_error(format!("Sync error: {e}"), None))?;
            if outcome.changed {
                let _ = cfg.save();
            }
            outcome
        };
        node.close().await;

        let msg = if outcome.changed {
            "Sync completed with changes."
        } else {
            "Sync completed (no changes)."
        };
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    async fn handle_send_message(&self, args: &Value) -> Result<CallToolResult, ErrorData> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("content is required", None))?;

        let mut inner = self.inner.lock().await;
        match inner.state {
            McpState::InRoomHost => {
                let cmd_tx = self.host_handle.commands.clone();
                drop(inner);
                cmd_tx
                    .send(HostCommand::Chat {
                        content: content.to_string(),
                    })
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text("Message sent.")]))
            }
            McpState::InRoomGuest => {
                if let Some(conn) = &mut inner.guest_conn {
                    conn.sender
                        .send(&ClientMessage::<()>::Chat {
                            content: content.to_string(),
                        })
                        .await
                        .map_err(|e| ErrorData::internal_error(format!("Send error: {e}"), None))?;
                    Ok(CallToolResult::success(vec![Content::text("Message sent.")]))
                } else {
                    Err(ErrorData::internal_error("Not connected to a room", None))
                }
            }
            _ => Err(ErrorData::invalid_params("Not in a room", None)),
        }
    }

    async fn handle_leave_room(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;
        match inner.state {
            McpState::InRoomHost => {
                let cmd_tx = self.host_handle.commands.clone();
                inner.state = McpState::MainMenu;
                inner.peer = Some(context.peer.clone());
                drop(inner);

                cmd_tx
                    .send(HostCommand::CloseRoom)
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

                let _ = context.peer.notify_tool_list_changed().await;
                Ok(CallToolResult::success(vec![Content::text("Room closed.")]))
            }
            McpState::InRoomGuest => {
                if let Some(mut conn) = inner.guest_conn.take() {
                    let _ = conn.sender.send(&ClientMessage::<()>::Leave).await;
                }
                inner.state = McpState::MainMenu;
                inner.peer = Some(context.peer.clone());
                drop(inner);

                let _ = context.peer.notify_tool_list_changed().await;
                Ok(CallToolResult::success(vec![Content::text(
                    "Left the room.",
                )]))
            }
            _ => Err(ErrorData::invalid_params("Not in a room", None)),
        }
    }
}

impl ServerHandler for PlaytableMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_logging()
                .build(),
        )
        .with_server_info(Implementation::new("playtable", env!("CARGO_PKG_VERSION")))
        .with_instructions("Playroom: P2P multiplayer rooms with chat. Call 'start' to begin.")
    }

    fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<InitializeResult, ErrorData>> + Send + '_ {
        async move {
            context.peer.set_peer_info(request);
            let mut inner = self.inner.lock().await;
            inner.peer = Some(context.peer.clone());
            Ok(self.get_info())
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        async move {
            let inner = self.inner.lock().await;
            let tools = Self::tools_for_state(&inner.state);
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            let name: &str = &request.name;
            let args = request
                .arguments
                .as_ref()
                .map(|a| serde_json::Value::Object(a.clone().into_iter().collect()))
                .unwrap_or(serde_json::Value::Null);

            match name {
                "start" => self.handle_start(context).await,
                "list_rooms" => self.handle_list_rooms().await,
                "create_room" => self.handle_create_room(&args, context).await,
                "join_room" => self.handle_join_room(&args, context).await,
                "list_friends" => self.handle_list_friends().await,
                "add_friend" => self.handle_add_friend(&args).await,
                "list_devices" => self.handle_list_devices().await,
                "pair_init" => self.handle_pair_init().await,
                "pair_complete" => self.handle_pair_complete(&args).await,
                "sync_device" => self.handle_sync_device(&args).await,
                "send_message" => self.handle_send_message(&args).await,
                "leave_room" => self.handle_leave_room(context).await,
                _ => Err(ErrorData::method_not_found::<CallToolRequestMethod>()),
            }
        }
    }

    fn get_tool(&self, _name: &str) -> Option<Tool> {
        None
    }
}

// ── Helper functions ──

fn parse_endpoint_addr(endpoint_id_hex: &str) -> Result<EndpointAddr, ErrorData> {
    let bytes = hex::decode(endpoint_id_hex)
        .map_err(|e| ErrorData::invalid_params(format!("Invalid hex: {e}"), None))?;
    if bytes.len() != 32 {
        return Err(ErrorData::invalid_params(
            "EndpointId must be 32 bytes (64 hex chars)",
            None,
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    let eid = EndpointId::from_bytes(&arr)
        .map_err(|e| ErrorData::invalid_params(format!("Invalid key: {e}"), None))?;
    Ok(EndpointAddr::from(eid))
}

fn make_tool(name: &str, description: &str, input_schema: Arc<JsonObject>) -> Tool {
    let mut tool = Tool::default();
    tool.name = Cow::Owned(name.to_string());
    tool.description = Some(Cow::Owned(description.to_string()));
    tool.input_schema = input_schema;
    tool
}

fn json_schema_empty() -> Arc<JsonObject> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {}
        }))
        .unwrap(),
    )
}

fn json_schema_obj(props: &[(&str, &str, &str)]) -> Arc<JsonObject> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, typ, desc) in props {
        properties.insert(
            name.to_string(),
            serde_json::json!({
                "type": typ,
                "description": desc,
            }),
        );
        required.push(serde_json::Value::String(name.to_string()));
    }
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }))
        .unwrap(),
    )
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

async fn query_friend_room(friend: &FriendEntry) -> anyhow::Result<Option<RoomSummary>> {
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
