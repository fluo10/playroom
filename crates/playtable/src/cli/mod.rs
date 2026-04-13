use anyhow::Result;
use playroom::{
    AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedReceiver, FramedSender, HostCommand,
    HostMessage, MemberInfo, NetworkNode, PeerSession, RoomEvent, RoomHost,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// クライアントの状態
enum ClientState {
    /// メインメニュー
    MainMenu,
    /// ルーム内（ホスト）
    InRoomHost,
    /// ルーム内（ゲスト）— sender を保持
    InRoomGuest {
        sender: FramedSender<ClientMessage>,
        room_name: String,
        members: Vec<MemberInfo>,
    },
}

pub async fn run(name: String) -> Result<()> {
    let mut config = AppConfig::load_or_default();
    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);

    // RoomHost を起動
    let (host, handle) = RoomHost::<()>::start(name.clone(), room_event_tx).await?;
    let host_cmd_tx = handle.commands.clone();
    let my_endpoint_id = handle.endpoint_id;

    println!("=== Playroom CLI ===");
    println!("Your name: {name}");
    println!("Your EndpointId: {}", hex::encode(my_endpoint_id.as_bytes()));
    println!();
    print_main_menu_help();

    // ホストをバックグラウンドで実行
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
        // プロンプト表示
        match &state {
            ClientState::MainMenu => eprint!("> "),
            ClientState::InRoomHost => eprint!("[host] > "),
            ClientState::InRoomGuest { room_name, .. } => eprint!("[{room_name}] > "),
        }

        tokio::select! {
            // stdin からの入力
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim().to_string();
                if line.is_empty() { continue; }

                match &mut state {
                    ClientState::MainMenu => {
                        match handle_main_menu_command(
                            &line,
                            &mut config,
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
                            Ok(None) => {} // stay in current state
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
            // ホストイベント（自分がホストの場合の通知）
            Some(event) = room_event_rx.recv() => {
                match event {
                    RoomEvent::MemberJoined { name } => {
                        println!("* {name} joined the room");
                    }
                    RoomEvent::MemberLeft { name } => {
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
                        // ホスト自身が解散した場合は handle_command で状態遷移済み
                    }
                }
            }
            // ゲスト受信メッセージ
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
                    HostMessage::MemberLeft { name } => {
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
    host_cmd_tx: &mpsc::Sender<HostCommand>,
    my_endpoint_id: &EndpointId,
    guest_msg_tx: &mpsc::Sender<HostMessage>,
) -> Result<Option<ClientState>, CommandAction> {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim());

    match cmd {
        "rooms" => {
            query_friend_rooms(config, my_endpoint_id).await;
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
            let Some(friend_name) = arg else {
                return Err(CommandAction::Error("Usage: join <friend_name>".into()));
            };
            match join_friend_room(config, friend_name, my_endpoint_id, guest_msg_tx).await {
                Ok(state) => Ok(Some(state)),
                Err(e) => Err(CommandAction::Error(format!("Failed to join: {e}"))),
            }
        }
        "friends" => {
            if config.friends.is_empty() {
                println!("No friends registered.");
            } else {
                println!("Friends:");
                for f in &config.friends {
                    let short_id = &f.endpoint_id[..16.min(f.endpoint_id.len())];
                    println!("  {} ({short_id}...)", f.name);
                }
            }
            Ok(None)
        }
        "add-friend" => {
            let Some(args_str) = arg else {
                return Err(CommandAction::Error(
                    "Usage: add-friend <endpoint_id_hex> <name>".into(),
                ));
            };
            let parts: Vec<&str> = args_str.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return Err(CommandAction::Error(
                    "Usage: add-friend <endpoint_id_hex> <name>".into(),
                ));
            }
            let endpoint_id_hex = parts[0];
            let friend_name = parts[1];

            // hex が有効な EndpointId か検証
            let bytes = hex::decode(endpoint_id_hex)
                .map_err(|e| CommandAction::Error(format!("Invalid hex: {e}")))?;
            if bytes.len() != 32 {
                return Err(CommandAction::Error(
                    "EndpointId must be 32 bytes (64 hex chars)".into(),
                ));
            }

            config.add_friend(friend_name.to_string(), endpoint_id_hex.to_string());
            config
                .save()
                .map_err(|e| CommandAction::Error(format!("Failed to save config: {e}")))?;
            println!("Friend '{}' added.", friend_name);
            Ok(None)
        }
        "id" => {
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
                println!("  {} ({id_short}...)", m.name);
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

/// フレンドのノードに QueryRoom を並列送信してルーム一覧を取得する
async fn query_friend_rooms(config: &AppConfig, _my_endpoint_id: &EndpointId) {
    if config.friends.is_empty() {
        println!("No friends registered. Use 'add-friend' to add friends.");
        return;
    }

    println!("Querying friends for rooms...");
    let mut handles = Vec::new();

    for friend in &config.friends {
        let friend_name = friend.name.clone();
        let endpoint_id_hex = friend.endpoint_id.clone();

        handles.push(tokio::spawn(async move {
            let result = query_single_room(&endpoint_id_hex).await;
            (friend_name, result)
        }));
    }

    let mut found_any = false;
    for handle in handles {
        if let Ok((_friend_name, Ok(Some((room_name, members, host_name))))) = handle.await {
            println!(
                "  [{host_name}] {room_name} ({} members: {})",
                members.len(),
                members.join(", ")
            );
            found_any = true;
        }
    }

    if !found_any {
        println!("  No rooms found.");
    }
}

async fn query_single_room(
    endpoint_id_hex: &str,
) -> Result<Option<(String, Vec<String>, String)>> {
    let bytes = hex::decode(endpoint_id_hex)?;
    let bytes_arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid EndpointId length"))?;
    let endpoint_id =
        EndpointId::from_bytes(&bytes_arr).map_err(|e| anyhow::anyhow!("Invalid key: {e}"))?;
    let addr = EndpointAddr::from(endpoint_id);

    let node = NetworkNode::bind(None).await?;
    let mut session: PeerSession<ClientMessage, HostMessage> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        node.connect(addr),
    )
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

/// フレンドのルームに参加する
async fn join_friend_room(
    config: &AppConfig,
    friend_name: &str,
    _my_endpoint_id: &EndpointId,
    guest_msg_tx: &mpsc::Sender<HostMessage>,
) -> Result<ClientState> {
    let friend = config
        .find_friend(friend_name)
        .ok_or_else(|| anyhow::anyhow!("Friend '{}' not found", friend_name))?;

    let bytes = hex::decode(&friend.endpoint_id)?;
    let bytes_arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid EndpointId length"))?;
    let endpoint_id =
        EndpointId::from_bytes(&bytes_arr).map_err(|e| anyhow::anyhow!("Invalid key: {e}"))?;
    let addr = EndpointAddr::from(endpoint_id);

    println!("Connecting to {friend_name}...");
    let node = NetworkNode::bind(None).await?;
    let mut session: PeerSession<ClientMessage, HostMessage> = node.connect(addr).await?;

    // 参加メッセージを送信（config の名前を使う）
    let player_name = AppConfig::load_or_default().name;
    session
        .send(&ClientMessage::Join {
            name: player_name,
        })
        .await?;

    // Welcome を待つ
    let response = session.recv().await?;
    match response {
        HostMessage::Welcome {
            room_name,
            members,
        } => {
            println!("Joined room '{room_name}'!");
            println!("Members:");
            for m in &members {
                let id_short = &hex::encode(&m.endpoint_id)[..16.min(m.endpoint_id.len() * 2)];
                println!("  {} ({id_short}...)", m.name);
            }

            // session を split して受信ループを起動
            let (sender, receiver, _peer_id) = session.split();
            let tx = guest_msg_tx.clone();
            tokio::spawn(guest_recv_loop(receiver, tx));

            Ok(ClientState::InRoomGuest {
                sender,
                room_name,
                members,
            })
        }
        HostMessage::Rejected { reason } => {
            anyhow::bail!("Rejected: {reason}");
        }
        other => {
            anyhow::bail!("Unexpected response: {other:?}");
        }
    }
}

/// ゲスト側の受信ループ
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
                // 接続が切れた → RoomClosed として通知
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
    println!("  join <friend_name> - Join a friend's room");
    println!("  friends            - List friends");
    println!("  add-friend <id> <name> - Add a friend by EndpointId");
    println!("  id                 - Show your EndpointId");
    println!("  quit               - Exit");
}

fn print_room_help() {
    println!("Room commands:");
    println!("  say <message>  - Send a chat message");
    println!("  leave          - Leave the room");
    println!("  members        - List room members");
}
