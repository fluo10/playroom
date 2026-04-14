use std::borrow::Cow;
use std::sync::Arc;

use playroom::{
    self, AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedSender, FriendEntry,
    HostCommand, HostMessage, MemberInfo, NetworkNode, RoomEvent, RoomHostHandle, RoomSummary,
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
    inner: Mutex<HandlerInner>,
    host_handle: RoomHostHandle,
}

struct HandlerInner {
    state: McpState,
    config: AppConfig,
    peer: Option<Peer<RoleServer>>,
    guest_conn: Option<GuestConnection>,
}

impl PlaytableMcpHandler {
    pub fn new(
        name: String,
        config: AppConfig,
        identity: Arc<UserIdentity>,
        host_handle: RoomHostHandle,
    ) -> Self {
        Self {
            name,
            identity,
            inner: Mutex::new(HandlerInner {
                state: McpState::Initial,
                config,
                peer: None,
                guest_conn: None,
            }),
            host_handle,
        }
    }

    pub async fn handle_room_event(&self, event: RoomEvent) {
        let message = match &event {
            RoomEvent::MemberJoined { name, .. } => format!("* {name} joined the room"),
            RoomEvent::MemberLeft { name, .. } => format!("* {name} left the room"),
            RoomEvent::ChatReceived { from, content } => format!("[chat] {from}: {content}"),
            RoomEvent::RoomCreated { name } => format!("* Room '{name}' created"),
            RoomEvent::RoomClosed => "* Room closed".to_string(),
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

    #[allow(dead_code)]
    pub async fn handle_guest_message(&self, msg: HostMessage) {
        let mut inner = self.inner.lock().await;

        let notification_text = match &msg {
            HostMessage::Chat { from, content } => {
                Some(format!("[chat] {from}: {content}"))
            }
            HostMessage::MemberJoined(info) => {
                if let Some(conn) = &mut inner.guest_conn {
                    conn.members.push(info.clone());
                }
                Some(format!("* {} joined the room", info.name))
            }
            HostMessage::MemberLeft { name, .. } => {
                if let Some(conn) = &mut inner.guest_conn {
                    conn.members.retain(|m| m.name != *name);
                }
                Some(format!("* {name} left the room"))
            }
            HostMessage::RoomClosed => {
                inner.state = McpState::MainMenu;
                inner.guest_conn = None;
                Some("* Room closed by host".to_string())
            }
            _ => None,
        };

        let should_notify_tools_changed = matches!(msg, HostMessage::RoomClosed);

        if let (Some(text), Some(peer)) = (notification_text, &inner.peer) {
            let _ = peer
                .notify_logging_message(LoggingMessageNotificationParam {
                    level: LoggingLevel::Info,
                    logger: Some("playroom".to_string()),
                    data: serde_json::json!(text),
                })
                .await;
            if should_notify_tools_changed {
                let _ = peer.notify_tool_list_changed().await;
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
                    "Join a friend's room (by petname or user public key hex)",
                    json_schema_obj(&[("friend", "string", "Friend petname or user public key hex")]),
                ),
                make_tool("list_friends", "List registered friends", json_schema_empty()),
                make_tool(
                    "add_friend",
                    "Add a friend by EndpointId (bootstrap address). The friend's user identity will be fetched via QueryRoom.",
                    json_schema_obj(&[
                        ("endpoint_id", "string", "Hex-encoded EndpointId (64 hex chars)"),
                        ("petname", "string", "Optional nickname for the friend"),
                    ]),
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
            let inner = self.inner.lock().await;
            inner
                .config
                .friends
                .iter()
                .filter(|f| !f.tombstone)
                .cloned()
                .collect()
        };

        if friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered. Use add_friend to add friends first.",
            )]));
        }

        let mut handles = Vec::new();
        for friend in friends {
            let display = friend.display_name();
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

        // フレンドを検索（petname 優先、見つからなければ user_public_key hex として解釈）
        let friend = {
            let inner = self.inner.lock().await;
            find_friend_by_alias(&inner.config, friend_arg).cloned()
        }
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("Friend '{friend_arg}' not found"), None)
        })?;

        // 接続先 EndpointAddr: cached_self_info があればそこから、なければエラー
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

        let member_list = joined
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let room_name = joined.room_name.clone();

        let mut inner = self.inner.lock().await;
        // ホスト情報のキャッシュも更新しておく
        let _ = inner.config.update_friend_self_info(joined.host_self_info);
        let _ = inner.config.save();

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
        let inner = self.inner.lock().await;
        let friends: Vec<&FriendEntry> = inner
            .config
            .friends
            .iter()
            .filter(|f| !f.tombstone)
            .collect();
        if friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered.",
            )]));
        }

        let mut lines = vec!["Friends:".to_string()];
        for f in friends {
            let short = &f.user_public_key.0[..8.min(f.user_public_key.0.len())];
            lines.push(format!("  {} (#{short})", f.display_name()));
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
        let petname = args
            .get("petname")
            .and_then(|v| v.as_str())
            .map(String::from);

        let bytes = hex::decode(endpoint_id)
            .map_err(|e| ErrorData::invalid_params(format!("Invalid hex: {e}"), None))?;
        if bytes.len() != 32 {
            return Err(ErrorData::invalid_params(
                "EndpointId must be 32 bytes (64 hex chars)",
                None,
            ));
        }
        let mut bytes_arr = [0u8; 32];
        bytes_arr.copy_from_slice(&bytes);
        let eid = EndpointId::from_bytes(&bytes_arr)
            .map_err(|e| ErrorData::invalid_params(format!("Invalid key: {e}"), None))?;
        let addr = EndpointAddr::from(eid);

        // ブートストラップ接続してフレンドのアイデンティティを取得
        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;
        let result = playroom::query_room::<()>(&node, addr)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Query error: {e}"), None))?;
        node.close().await;

        let key_hex = UserPublicKeyHex::from_bytes(&result.host_self_info.user_public_key);
        let display = petname.clone().unwrap_or_else(|| result.host_self_info.name.clone());

        let mut inner = self.inner.lock().await;
        inner.config.upsert_friend(key_hex, petname);
        let _ = inner.config.update_friend_self_info(result.host_self_info);
        inner
            .config
            .save()
            .map_err(|e| ErrorData::internal_error(format!("Save error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Friend '{display}' added.",
        ))]))
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

/// petname または user_public_key の hex プレフィックスでフレンドを探す。
fn find_friend_by_alias<'a>(config: &'a AppConfig, alias: &str) -> Option<&'a FriendEntry> {
    // petname 完全一致
    if let Some(f) = config.friends.iter().find(|f| {
        !f.tombstone && f.petname.as_deref() == Some(alias)
    }) {
        return Some(f);
    }
    // cached self info 名前一致
    if let Some(f) = config.friends.iter().find(|f| {
        !f.tombstone
            && f.cached_self_info
                .as_ref()
                .map(|i| i.name == alias)
                .unwrap_or(false)
    }) {
        return Some(f);
    }
    // user_public_key hex 前方一致
    config
        .friends
        .iter()
        .find(|f| !f.tombstone && f.user_public_key.0.starts_with(alias))
}

/// フレンドに対して QueryRoom を送り、ルーム情報を取得する。
async fn query_friend_room(
    friend: &FriendEntry,
) -> anyhow::Result<Option<RoomSummary>> {
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
