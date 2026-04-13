use std::borrow::Cow;
use std::sync::Arc;

use playroom::{
    AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedSender, HostCommand,
    HostMessage, MemberInfo, NetworkNode, PeerSession, RoomEvent, RoomHostHandle,
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
    pub fn new(name: String, config: AppConfig, host_handle: RoomHostHandle) -> Self {
        Self {
            name,
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
            RoomEvent::MemberJoined { name } => format!("* {name} joined the room"),
            RoomEvent::MemberLeft { name } => format!("* {name} left the room"),
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
            HostMessage::MemberLeft { name } => {
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
                    "Join a friend's room",
                    json_schema_obj(&[("friend_name", "string", "Friend name to join")]),
                ),
                make_tool("list_friends", "List registered friends", json_schema_empty()),
                make_tool(
                    "add_friend",
                    "Add a friend by EndpointId",
                    json_schema_obj(&[
                        ("endpoint_id", "string", "Hex-encoded EndpointId (64 hex chars)"),
                        ("name", "string", "Friend name"),
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
            "Playroom started! Your EndpointId: {}",
            hex::encode(self.host_handle.endpoint_id.as_bytes())
        ))]))
    }

    async fn handle_list_rooms(&self) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let friends: Vec<_> = inner.config.friends.clone();
        drop(inner);

        if friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered. Use add_friend to add friends first.",
            )]));
        }

        let mut handles = Vec::new();
        for friend in &friends {
            let endpoint_id_hex = friend.endpoint_id.clone();
            let friend_name = friend.name.clone();
            handles.push(tokio::spawn(async move {
                let result = query_single_room(&endpoint_id_hex).await;
                (friend_name, result)
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok((_friend_name, Ok(Some((room_name, members, host_name))))) = handle.await {
                results.push(format!(
                    "[{host_name}] {room_name} ({} members: {})",
                    members.len(),
                    members.join(", ")
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
            "Room '{}' created! You are now the host. Others can join using your EndpointId: {}",
            name,
            hex::encode(self.host_handle.endpoint_id.as_bytes())
        ))]))
    }

    async fn handle_join_room(
        &self,
        args: &Value,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let friend_name = args
            .get("friend_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("friend_name is required", None))?;

        let endpoint_id_hex = {
            let inner = self.inner.lock().await;
            let friend = inner.config.find_friend(friend_name).ok_or_else(|| {
                ErrorData::invalid_params(format!("Friend '{friend_name}' not found"), None)
            })?;
            friend.endpoint_id.clone()
        };

        let bytes = hex::decode(&endpoint_id_hex)
            .map_err(|e| ErrorData::internal_error(format!("Invalid hex: {e}"), None))?;
        let bytes_arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ErrorData::internal_error("Invalid EndpointId length", None))?;
        let endpoint_id = EndpointId::from_bytes(&bytes_arr)
            .map_err(|e| ErrorData::internal_error(format!("Invalid key: {e}"), None))?;
        let addr = EndpointAddr::from(endpoint_id);

        let node = NetworkNode::bind(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Bind error: {e}"), None))?;
        let mut session: PeerSession<ClientMessage<()>, HostMessage<()>> = node
            .connect(addr)
            .await
            .map_err(|e| ErrorData::internal_error(format!("Connection error: {e}"), None))?;

        session
            .send(&ClientMessage::Join {
                name: self.name.clone(),
            })
            .await
            .map_err(|e| ErrorData::internal_error(format!("Send error: {e}"), None))?;

        let response = session
            .recv()
            .await
            .map_err(|e| ErrorData::internal_error(format!("Recv error: {e}"), None))?;

        match response {
            HostMessage::Welcome {
                room_name,
                members,
            } => {
                let (sender, _receiver, _) = session.split();
                let member_list = members
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");

                let mut inner = self.inner.lock().await;
                inner.state = McpState::InRoomGuest;
                inner.guest_conn = Some(GuestConnection {
                    sender,
                    room_name: room_name.clone(),
                    members,
                });
                inner.peer = Some(context.peer.clone());
                let _ = context.peer.notify_tool_list_changed().await;

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Joined room '{room_name}'! Members: {member_list}"
                ))]))
            }
            HostMessage::Rejected { reason } => Ok(CallToolResult::error(vec![Content::text(
                format!("Rejected: {reason}"),
            )])),
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unexpected response: {other:?}"
            ))])),
        }
    }

    async fn handle_list_friends(&self) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        if inner.config.friends.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No friends registered.",
            )]));
        }

        let mut lines = vec!["Friends:".to_string()];
        for f in &inner.config.friends {
            let short_id = &f.endpoint_id[..16.min(f.endpoint_id.len())];
            lines.push(format!("  {} ({short_id}...)", f.name));
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
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("name is required", None))?;

        let bytes = hex::decode(endpoint_id)
            .map_err(|e| ErrorData::invalid_params(format!("Invalid hex: {e}"), None))?;
        if bytes.len() != 32 {
            return Err(ErrorData::invalid_params(
                "EndpointId must be 32 bytes (64 hex chars)",
                None,
            ));
        }

        let mut inner = self.inner.lock().await;
        inner.config.add_friend(name.to_string(), endpoint_id.to_string());
        inner
            .config
            .save()
            .map_err(|e| ErrorData::internal_error(format!("Save error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Friend '{name}' added.",
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

async fn query_single_room(
    endpoint_id_hex: &str,
) -> anyhow::Result<Option<(String, Vec<String>, String)>> {
    let bytes = hex::decode(endpoint_id_hex)?;
    let bytes_arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid EndpointId length"))?;
    let endpoint_id =
        EndpointId::from_bytes(&bytes_arr).map_err(|e| anyhow::anyhow!("Invalid key: {e}"))?;
    let addr = EndpointAddr::from(endpoint_id);

    let node = NetworkNode::bind(None).await?;
    let mut session: PeerSession<ClientMessage<()>, HostMessage<()>> =
        tokio::time::timeout(std::time::Duration::from_secs(5), node.connect(addr))
            .await
            .map_err(|_| anyhow::anyhow!("Connection timeout"))??;

    session.send(&ClientMessage::QueryRoom).await?;

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), session.recv())
        .await
        .map_err(|_| anyhow::anyhow!("Response timeout"))??;

    node.close().await;

    match response {
        HostMessage::RoomInfo {
            name,
            members,
            host_name,
        } => Ok(Some((name, members, host_name))),
        HostMessage::NotHosting => Ok(None),
        _ => Ok(None),
    }
}
