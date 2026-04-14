use std::sync::Arc;

use anyhow::Result;
use playroom::{
    AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedReceiver, FramedSender, FriendEntry,
    HostCommand, HostMessage, MemberInfo, NetworkNode, RoomEvent, RoomHost, RoomSummary,
    UserIdentity, UserPublicKeyHex,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// クライアントの状態
enum ClientState {
    MainMenu,
    InRoomHost,
    InRoomGuest {
        sender: FramedSender<ClientMessage>,
        room_name: String,
        members: Vec<MemberInfo>,
    },
}

pub async fn run(name: String) -> Result<()> {
    let mut config = AppConfig::load_or_default();
    let identity = Arc::new(UserIdentity::load_or_generate()?);
    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);

    // RoomHost を起動
    let (host, handle) = RoomHost::<()>::start(name.clone(), identity.clone(), room_event_tx).await?;
    let host_cmd_tx = handle.commands.clone();
    let my_endpoint_id = handle.endpoint_id;

    println!("=== Playroom CLI ===");
    println!("Your name: {name}");
    println!("Your user key: {}", hex::encode(identity.public_key()));
    println!("Your EndpointId: {}", hex::encode(my_endpoint_id.as_bytes()));
    println!();
    print_main_menu_help();

    tokio::spawn(async move {
        if let Err(e) = host.run().await {
            eprintln!("Host error: {e}");
        }
    });

    // ゲスト接続時のメッセージ受信チャネル
    let (guest_msg_tx, mut guest_msg_rx) = mpsc::channel::<HostMessage>(64);

    let mut state = ClientState::MainMenu;
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        match &state {
            ClientState::MainMenu => eprint!("> "),
            ClientState::InRoomHost => eprint!("[host] > "),
            ClientState::InRoomGuest { room_name, .. } => eprint!("[{room_name}] > "),
        }

        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim().to_string();
                if line.is_empty() { continue; }

                match &mut state {
                    ClientState::MainMenu => {
                        match handle_main_menu_command(
                            &line,
                            &mut config,
                            &identity,
                            &host_cmd_tx,
                            &my_endpoint_id,
                            &guest_msg_tx,
                        ).await {
                            Ok(Some(new_state)) => {
                                state = new_state;
                                match &state {
                                    ClientState::InRoomHost => print_room_help(),
                                    ClientState::InRoomGuest { .. } => print_room_help(),
                                    _ => {}
                                }
                            }
                            Ok(None) => {}
                            Err(CommandAction::Quit) => break,
                            Err(CommandAction::Error(e)) => eprintln!("Error: {e}"),
                        }
                    }
                    ClientState::InRoomHost => {
                        match handle_room_host_command(&line, &host_cmd_tx).await {
                            Ok(Some(new_state)) => {
                                state = new_state;
                                print_main_menu_help();
                            }
                            Ok(None) => {}
                            Err(CommandAction::Quit) => break,
                            Err(CommandAction::Error(e)) => eprintln!("Error: {e}"),
                        }
                    }
                    ClientState::InRoomGuest { sender, members, .. } => {
                        match handle_room_guest_command(&line, sender, members).await {
                            Ok(Some(new_state)) => {
                                state = new_state;
                                print_main_menu_help();
                            }
                            Ok(None) => {}
                            Err(CommandAction::Quit) => break,
                            Err(CommandAction::Error(e)) => eprintln!("Error: {e}"),
                        }
                    }
                }
            }
            Some(event) = room_event_rx.recv() => {
                match event {
                    RoomEvent::MemberJoined { name, .. } => {
                        println!("* {name} joined the room");
                    }
                    RoomEvent::MemberLeft { name, .. } => {
                        println!("* {name} left the room");
                    }
                    RoomEvent::ChatReceived { from, content } => {
                        println!("[chat] {from}: {content}");
                    }
                    RoomEvent::RoomCreated { name } => {
                        println!("* Room '{name}' created");
                    }
                    RoomEvent::RoomClosed => {
                        println!("* Room closed");
                    }
                }
            }
            Some(msg) = guest_msg_rx.recv() => {
                match msg {
                    HostMessage::Chat { from, content } => {
                        println!("[chat] {from}: {content}");
                    }
                    HostMessage::MemberJoined(info) => {
                        println!("* {} joined the room", info.name);
                        if let ClientState::InRoomGuest { members, .. } = &mut state {
                            members.push(info);
                        }
                    }
                    HostMessage::MemberLeft { name, .. } => {
                        println!("* {name} left the room");
                        if let ClientState::InRoomGuest { members, .. } = &mut state {
                            members.retain(|m| m.name != name);
                        }
                    }
                    HostMessage::RoomClosed => {
                        println!("* Room has been closed by the host");
                        state = ClientState::MainMenu;
                        print_main_menu_help();
                    }
                    HostMessage::Rejected { reason } => {
                        eprintln!("Rejected: {reason}");
                    }
                    _ => {}
                }
            }
        }
    }

    println!("Goodbye!");
    Ok(())
}

enum CommandAction {
    Quit,
    Error(String),
}

async fn handle_main_menu_command(
    line: &str,
    config: &mut AppConfig,
    identity: &Arc<UserIdentity>,
    host_cmd_tx: &mpsc::Sender<HostCommand>,
    my_endpoint_id: &EndpointId,
    guest_msg_tx: &mpsc::Sender<HostMessage>,
) -> Result<Option<ClientState>, CommandAction> {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim());

    match cmd {
        "rooms" => {
            query_friend_rooms(config).await;
            Ok(None)
        }
        "create" => {
            let name = arg.unwrap_or("My Room");
            host_cmd_tx
                .send(HostCommand::CreateRoom {
                    name: name.to_string(),
                })
                .await
                .map_err(|e| CommandAction::Error(e.to_string()))?;
            Ok(Some(ClientState::InRoomHost))
        }
        "join" => {
            let Some(friend_alias) = arg else {
                return Err(CommandAction::Error("Usage: join <friend>".into()));
            };
            let player_name = config.name.clone();
            match join_friend_room(config, friend_alias, identity, &player_name, guest_msg_tx).await {
                Ok(state) => Ok(Some(state)),
                Err(e) => Err(CommandAction::Error(format!("Failed to join: {e}"))),
            }
        }
        "friends" => {
            let friends: Vec<&FriendEntry> =
                config.friends.iter().filter(|f| !f.tombstone).collect();
            if friends.is_empty() {
                println!("No friends registered.");
            } else {
                println!("Friends:");
                for f in friends {
                    let short = &f.user_public_key.0[..8.min(f.user_public_key.0.len())];
                    println!("  {} (#{short})", f.display_name());
                }
            }
            Ok(None)
        }
        "add-friend" => {
            let Some(args_str) = arg else {
                return Err(CommandAction::Error(
                    "Usage: add-friend <endpoint_id_hex> [petname]".into(),
                ));
            };
            let parts: Vec<&str> = args_str.splitn(2, ' ').collect();
            let endpoint_id_hex = parts[0];
            let petname = parts.get(1).map(|s| s.trim().to_string());

            let bytes = hex::decode(endpoint_id_hex)
                .map_err(|e| CommandAction::Error(format!("Invalid hex: {e}")))?;
            if bytes.len() != 32 {
                return Err(CommandAction::Error(
                    "EndpointId must be 32 bytes (64 hex chars)".into(),
                ));
            }
            let mut bytes_arr = [0u8; 32];
            bytes_arr.copy_from_slice(&bytes);
            let eid = EndpointId::from_bytes(&bytes_arr)
                .map_err(|e| CommandAction::Error(format!("Invalid key: {e}")))?;
            let addr = EndpointAddr::from(eid);

            let node = NetworkNode::bind(None)
                .await
                .map_err(|e| CommandAction::Error(format!("Bind error: {e}")))?;
            let result = playroom::query_room::<()>(&node, addr)
                .await
                .map_err(|e| CommandAction::Error(format!("Query error: {e}")))?;
            node.close().await;

            let key_hex = UserPublicKeyHex::from_bytes(&result.host_self_info.user_public_key);
            let display = petname.clone().unwrap_or_else(|| result.host_self_info.name.clone());
            config.upsert_friend(key_hex, petname);
            let _ = config.update_friend_self_info(result.host_self_info);
            config
                .save()
                .map_err(|e| CommandAction::Error(format!("Failed to save config: {e}")))?;
            println!("Friend '{display}' added.");
            Ok(None)
        }
        "id" => {
            println!("Your user key: {}", hex::encode(identity.public_key()));
            println!("Your EndpointId: {}", hex::encode(my_endpoint_id.as_bytes()));
            Ok(None)
        }
        "quit" | "exit" => Err(CommandAction::Quit),
        "help" => {
            print_main_menu_help();
            Ok(None)
        }
        _ => {
            eprintln!("Unknown command: {cmd}. Type 'help' for available commands.");
            Ok(None)
        }
    }
}

async fn handle_room_host_command(
    line: &str,
    host_cmd_tx: &mpsc::Sender<HostCommand>,
) -> Result<Option<ClientState>, CommandAction> {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim());

    match cmd {
        "say" => {
            let Some(content) = arg else {
                return Err(CommandAction::Error("Usage: say <message>".into()));
            };
            host_cmd_tx
                .send(HostCommand::Chat {
                    content: content.to_string(),
                })
                .await
                .map_err(|e| CommandAction::Error(e.to_string()))?;
            Ok(None)
        }
        "leave" => {
            host_cmd_tx
                .send(HostCommand::CloseRoom)
                .await
                .map_err(|e| CommandAction::Error(e.to_string()))?;
            Ok(Some(ClientState::MainMenu))
        }
        "help" => {
            print_room_help();
            Ok(None)
        }
        _ => {
            eprintln!("Unknown command: {cmd}. Type 'help' for available commands.");
            Ok(None)
        }
    }
}

async fn handle_room_guest_command(
    line: &str,
    sender: &mut FramedSender<ClientMessage>,
    members: &[MemberInfo],
) -> Result<Option<ClientState>, CommandAction> {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim());

    match cmd {
        "say" => {
            let Some(content) = arg else {
                return Err(CommandAction::Error("Usage: say <message>".into()));
            };
            sender
                .send(&ClientMessage::<()>::Chat {
                    content: content.to_string(),
                })
                .await
                .map_err(|e| CommandAction::Error(format!("Send error: {e}")))?;
            Ok(None)
        }
        "leave" => {
            let _ = sender.send(&ClientMessage::<()>::Leave).await;
            Ok(Some(ClientState::MainMenu))
        }
        "members" => {
            println!("Room members:");
            for m in members {
                let id_short = &hex::encode(&m.endpoint_id)[..16.min(m.endpoint_id.len() * 2)];
                let key_short = &hex::encode(m.user_public_key)[..8];
                println!("  {} (#{key_short}, dev {id_short}...)", m.name);
            }
            Ok(None)
        }
        "help" => {
            print_room_help();
            Ok(None)
        }
        _ => {
            eprintln!("Unknown command: {cmd}. Type 'help' for available commands.");
            Ok(None)
        }
    }
}

/// フレンドに対して並列で QueryRoom を送りルーム一覧を取得する
async fn query_friend_rooms(config: &AppConfig) {
    let friends: Vec<FriendEntry> = config
        .friends
        .iter()
        .filter(|f| !f.tombstone)
        .cloned()
        .collect();
    if friends.is_empty() {
        println!("No friends registered. Use 'add-friend' to add friends.");
        return;
    }

    println!("Querying friends for rooms...");
    let mut handles = Vec::new();
    for friend in friends {
        let display = friend.display_name();
        handles.push(tokio::spawn(async move {
            let result = query_friend_room(&friend).await;
            (display, result)
        }));
    }

    let mut found_any = false;
    for handle in handles {
        if let Ok((friend_name, Ok(Some(summary)))) = handle.await {
            println!(
                "  [{friend_name}] {} ({} members: {})",
                summary.name,
                summary.members.len(),
                summary.members.join(", ")
            );
            found_any = true;
        }
    }

    if !found_any {
        println!("  No rooms found.");
    }
}

async fn query_friend_room(friend: &FriendEntry) -> Result<Option<RoomSummary>> {
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

async fn join_friend_room(
    config: &mut AppConfig,
    friend_alias: &str,
    identity: &Arc<UserIdentity>,
    player_name: &str,
    guest_msg_tx: &mpsc::Sender<HostMessage>,
) -> Result<ClientState> {
    let friend = find_friend_by_alias(config, friend_alias)
        .ok_or_else(|| anyhow::anyhow!("Friend '{}' not found", friend_alias))?
        .clone();

    let addr = friend
        .cached_self_info
        .as_ref()
        .and_then(|info| info.endpoint_addrs.first().cloned())
        .ok_or_else(|| {
            anyhow::anyhow!("no known endpoint address; try 'rooms' first to refresh")
        })?;

    println!("Connecting to {}...", friend.display_name());
    let node = NetworkNode::bind(None).await?;
    let joined = playroom::join_room::<()>(&node, addr, identity, player_name.to_string()).await?;

    println!("Joined room '{}'!", joined.room_name);
    println!("Members:");
    for m in &joined.members {
        let id_short = &hex::encode(&m.endpoint_id)[..16.min(m.endpoint_id.len() * 2)];
        let key_short = &hex::encode(m.user_public_key)[..8];
        println!("  {} (#{key_short}, dev {id_short}...)", m.name);
    }

    // ホスト情報を更新して永続化
    let _ = config.update_friend_self_info(joined.host_self_info);
    let _ = config.save();

    let tx = guest_msg_tx.clone();
    tokio::spawn(guest_recv_loop(joined.receiver, tx));

    Ok(ClientState::InRoomGuest {
        sender: joined.sender,
        room_name: joined.room_name,
        members: joined.members,
    })
}

fn find_friend_by_alias<'a>(config: &'a AppConfig, alias: &str) -> Option<&'a FriendEntry> {
    if let Some(f) = config
        .friends
        .iter()
        .find(|f| !f.tombstone && f.petname.as_deref() == Some(alias))
    {
        return Some(f);
    }
    if let Some(f) = config.friends.iter().find(|f| {
        !f.tombstone
            && f.cached_self_info
                .as_ref()
                .map(|i| i.name == alias)
                .unwrap_or(false)
    }) {
        return Some(f);
    }
    config
        .friends
        .iter()
        .find(|f| !f.tombstone && f.user_public_key.0.starts_with(alias))
}

async fn guest_recv_loop(
    mut receiver: FramedReceiver<HostMessage>,
    tx: mpsc::Sender<HostMessage>,
) {
    loop {
        match receiver.recv().await {
            Ok(msg) => {
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
            Err(_) => {
                let _ = tx.send(HostMessage::RoomClosed).await;
                break;
            }
        }
    }
}

fn print_main_menu_help() {
    println!("Commands:");
    println!("  rooms              - List friends' rooms");
    println!("  create <name>      - Create and host a room");
    println!("  join <friend>      - Join a friend's room (petname or key hex prefix)");
    println!("  friends            - List friends");
    println!("  add-friend <endpoint_id_hex> [petname] - Add a friend by bootstrap EndpointId");
    println!("  id                 - Show your user key and EndpointId");
    println!("  quit               - Exit");
}

fn print_room_help() {
    println!("Room commands:");
    println!("  say <message>  - Send a chat message");
    println!("  leave          - Leave the room");
    println!("  members        - List room members");
}
