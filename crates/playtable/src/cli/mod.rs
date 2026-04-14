use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use playroom::{
    AppConfig, ClientMessage, EndpointAddr, EndpointId, FramedReceiver, FramedSender, FriendEntry,
    HostCommand, HostMessage, Invite, MemberInfo, NetworkNode, RoomEvent, RoomHost, RoomSummary,
    UserIdentity, UserPublicKeyHex,
};
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::{mpsc, Mutex};

enum ClientState {
    MainMenu,
    InRoomHost,
    InRoomGuest {
        sender: FramedSender<ClientMessage>,
        room_name: String,
        members: Vec<MemberInfo>,
    },
}

pub async fn run() -> Result<()> {
    let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // 初回起動時：user_id が未設定なら新規 or 既存ユーザーの分岐を聞く
    let already_initialized = {
        let cfg = config.lock().await;
        cfg.has_valid_user_id()
    };

    let identity = if already_initialized {
        Arc::new(UserIdentity::load_or_generate()?)
    } else {
        bootstrap_identity(&config, &mut lines).await?
    };

    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);

    let (host, handle) = RoomHost::<()>::start(identity.clone(), config.clone(), room_event_tx)
        .await?;
    let host_cmd_tx = handle.commands.clone();
    let my_endpoint_id = handle.endpoint_id;

    println!("=== Playroom CLI ===");
    {
        let cfg = config.lock().await;
        println!("user_id: {}", cfg.user_id);
    }
    println!("user key: {}", hex::encode(identity.public_key()));
    println!("EndpointId: {}", hex::encode(my_endpoint_id.as_bytes()));
    println!();
    print_main_menu_help();

    tokio::spawn(async move {
        if let Err(e) = host.run().await {
            eprintln!("Host error: {e}");
        }
    });

    let (guest_msg_tx, mut guest_msg_rx) = mpsc::channel::<HostMessage>(64);

    let mut state = ClientState::MainMenu;

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
                            &config,
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
                    RoomEvent::MemberJoined { user_id: name, .. } => {
                        println!("* {name} joined the room");
                    }
                    RoomEvent::MemberLeft { user_id: name, .. } => {
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
                    RoomEvent::PairingWindowOpened => {
                        // 招待コードは pair-init コマンド側で表示済み
                    }
                    RoomEvent::PairingWindowClosed => {
                        println!("* Pairing window closed");
                    }
                    RoomEvent::PairingCompleted { device_label, .. } => {
                        println!("* Paired with new device: {device_label}");
                    }
                    RoomEvent::SyncCompleted { .. } => {
                        println!("* Sync completed");
                    }
                }
            }
            Some(msg) = guest_msg_rx.recv() => {
                match msg {
                    HostMessage::Chat { from, content } => {
                        println!("[chat] {from}: {content}");
                    }
                    HostMessage::MemberJoined(info) => {
                        println!("* {} joined the room", info.user_id);
                        if let ClientState::InRoomGuest { members, .. } = &mut state {
                            members.push(info);
                        }
                    }
                    HostMessage::MemberLeft { user_id: name, .. } => {
                        println!("* {name} left the room");
                        if let ClientState::InRoomGuest { members, .. } = &mut state {
                            members.retain(|m| m.user_id != name);
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

/// 初回起動時、新規ユーザーか既存ユーザーかを聞き、適切に user_id と identity を確立する。
async fn bootstrap_identity(
    config: &Arc<Mutex<AppConfig>>,
    lines: &mut Lines<BufReader<Stdin>>,
) -> Result<Arc<UserIdentity>> {
    println!("=== Playroom first-launch setup ===");
    println!("[n]ew user (create a new identity)");
    println!("[e]xisting user (adopt an identity from another device via invite)");

    loop {
        eprint!("Choice [n/e]: ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed before setup completed");
        };
        match line.trim().to_lowercase().as_str() {
            "n" | "new" => return bootstrap_new_user(config, lines).await,
            "e" | "existing" => return bootstrap_existing_user(config, lines).await,
            _ => eprintln!("Please enter 'n' or 'e'."),
        }
    }
}

async fn bootstrap_new_user(
    config: &Arc<Mutex<AppConfig>>,
    lines: &mut Lines<BufReader<Stdin>>,
) -> Result<Arc<UserIdentity>> {
    let user_id = loop {
        eprint!("Enter user_id (alphanumeric, 1-32 chars): ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed");
        };
        let id = line.trim().to_string();
        if playroom::identity::is_valid_user_id(&id) {
            break id;
        }
        eprintln!("Invalid user_id. Must be 1-32 ASCII alphanumeric characters.");
    };

    let identity = Arc::new(UserIdentity::load_or_generate()?);
    let mut cfg = config.lock().await;
    cfg.user_id = user_id.clone();
    cfg.save()?;
    println!("Created user '{user_id}'.");
    Ok(identity)
}

async fn bootstrap_existing_user(
    config: &Arc<Mutex<AppConfig>>,
    lines: &mut Lines<BufReader<Stdin>>,
) -> Result<Arc<UserIdentity>> {
    let invite = loop {
        eprint!("Paste invite code (77 chars): ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed");
        };
        match line.trim().parse::<Invite>() {
            Ok(i) => break i,
            Err(e) => eprintln!("Invalid invite: {e}"),
        }
    };

    eprint!("Device label (e.g. 'Laptop'): ");
    let Some(line) = lines.next_line().await? else {
        anyhow::bail!("stdin closed");
    };
    let device_label = {
        let t = line.trim();
        if t.is_empty() {
            "New Device".to_string()
        } else {
            t.to_string()
        }
    };

    println!("Connecting to existing device...");
    let node = NetworkNode::bind(None).await?;
    let own_id = node.id();
    let data = playroom::pair_as_new_device(&node, &invite, device_label.clone()).await?;
    node.close().await;

    playroom::persist_paired_data(data, &own_id, device_label.clone())?;

    // 永続化された identity.key + config を読み直す
    let identity = Arc::new(UserIdentity::load_or_generate()?);
    {
        let mut cfg_in = config.lock().await;
        *cfg_in = AppConfig::load_or_default();
    }
    println!("Paired successfully as '{}'.", config.lock().await.user_id);
    Ok(identity)
}

enum CommandAction {
    Quit,
    Error(String),
}

async fn handle_main_menu_command(
    line: &str,
    config: &Arc<Mutex<AppConfig>>,
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
            let player_user_id = {
                let cfg = config.lock().await;
                cfg.user_id.clone()
            };
            match join_friend_room(config, friend_alias, identity, &player_user_id, guest_msg_tx)
                .await
            {
                Ok(state) => Ok(Some(state)),
                Err(e) => Err(CommandAction::Error(format!("Failed to join: {e}"))),
            }
        }
        "friends" => {
            let cfg = config.lock().await;
            let friends: Vec<&FriendEntry> =
                cfg.friends.iter().filter(|f| !f.tombstone).collect();
            if friends.is_empty() {
                println!("No friends registered.");
            } else {
                println!("Friends:");
                let entries: Vec<(String, playroom::UserPublicKey)> = friends
                    .iter()
                    .map(|f| {
                        let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
                        (f.user_id(), key)
                    })
                    .collect();
                for (id, pk) in &entries {
                    println!("  {}", playroom::identity::with_key_suffix(id, pk));
                }
            }
            Ok(None)
        }
        "add-friend" => {
            let Some(endpoint_id_hex) = arg else {
                return Err(CommandAction::Error(
                    "Usage: add-friend <endpoint_id_hex>".into(),
                ));
            };
            let addr = parse_endpoint_addr(endpoint_id_hex).map_err(CommandAction::Error)?;

            let node = NetworkNode::bind(None)
                .await
                .map_err(|e| CommandAction::Error(format!("Bind error: {e}")))?;
            let result = playroom::query_room::<()>(&node, addr)
                .await
                .map_err(|e| CommandAction::Error(format!("Query error: {e}")))?;
            node.close().await;

            let key_hex = UserPublicKeyHex::from_bytes(&result.host_self_info.user_public_key);
            let display = result.host_self_info.user_id.clone();
            let mut cfg = config.lock().await;
            cfg.upsert_friend(key_hex);
            let _ = cfg.update_friend_self_info(result.host_self_info);
            cfg.save()
                .map_err(|e| CommandAction::Error(format!("Failed to save config: {e}")))?;
            println!("Friend '{display}' added.");
            Ok(None)
        }
        "devices" => {
            let cfg = config.lock().await;
            let devices: Vec<_> = cfg.my_devices.iter().filter(|d| !d.tombstone).collect();
            if devices.is_empty() {
                println!("No devices registered. Use 'pair-init' on an existing device and 'pair-device' on a new one.");
            } else {
                println!("Your devices:");
                for d in devices {
                    let short = playroom::identity::short_bytes_from_hex(&d.endpoint_id)
                        .unwrap_or_else(|| "invalid".into());
                    println!("  {} (#{short})", d.label);
                }
            }
            Ok(None)
        }
        "pair-init" => {
            let invite = Invite::generate(*my_endpoint_id.as_bytes());
            host_cmd_tx
                .send(HostCommand::OpenPairingWindow {
                    secret: invite.secret,
                    ttl: Duration::from_secs(120),
                })
                .await
                .map_err(|e| CommandAction::Error(e.to_string()))?;
            println!("Pairing window open for 120s.");
            println!("On the new device (during first-launch 'existing' setup), paste:");
            println!("  {invite}");
            Ok(None)
        }
        "sync" => {
            let Some(endpoint_id_hex) = arg else {
                return Err(CommandAction::Error(
                    "Usage: sync <other_device_endpoint_id_hex>".into(),
                ));
            };
            let addr = parse_endpoint_addr(endpoint_id_hex).map_err(CommandAction::Error)?;

            let node = NetworkNode::bind(None)
                .await
                .map_err(|e| CommandAction::Error(format!("Bind error: {e}")))?;
            let outcome = {
                let mut cfg = config.lock().await;
                let outcome = playroom::sync_with_peer(&node, addr, identity, &mut cfg)
                    .await
                    .map_err(|e| CommandAction::Error(format!("Sync error: {e}")))?;
                if outcome.changed {
                    let _ = cfg.save();
                }
                outcome
            };
            node.close().await;

            if outcome.changed {
                println!("Sync completed with changes.");
            } else {
                println!("Sync completed (no changes).");
            }
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
            let entries: Vec<(String, playroom::UserPublicKey)> = members
                .iter()
                .map(|m| (m.user_id.clone(), m.user_public_key))
                .collect();
            for (m, (_, _)) in members.iter().zip(entries.iter()) {
                let name = playroom::identity::with_key_suffix(&m.user_id, &m.user_public_key);
                let id_short = playroom::identity::short_bytes(&m.endpoint_id)
                    .unwrap_or_else(|| "invalid".into());
                println!("  {name} (dev #{id_short})");
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

async fn query_friend_rooms(config: &Arc<Mutex<AppConfig>>) {
    let friends: Vec<FriendEntry> = {
        let cfg = config.lock().await;
        cfg.friends.iter().filter(|f| !f.tombstone).cloned().collect()
    };
    if friends.is_empty() {
        println!("No friends registered. Use 'add-friend' to add friends.");
        return;
    }

    println!("Querying friends for rooms...");
    let entries: Vec<(String, playroom::UserPublicKey)> = friends
        .iter()
        .map(|f| {
            let key = f.user_public_key.to_bytes().unwrap_or([0u8; 32]);
            (f.user_id(), key)
        })
        .collect();
    let display_names: Vec<String> = entries
        .iter()
        .map(|(id, pk)| playroom::identity::with_key_suffix(id, pk))
        .collect();

    let mut handles = Vec::new();
    for (friend, display) in friends.into_iter().zip(display_names.into_iter()) {
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
    config: &Arc<Mutex<AppConfig>>,
    friend_alias: &str,
    identity: &Arc<UserIdentity>,
    player_user_id: &str,
    guest_msg_tx: &mpsc::Sender<HostMessage>,
) -> Result<ClientState> {
    let friend = {
        let cfg = config.lock().await;
        find_friend_by_alias(&cfg, friend_alias)
            .ok_or_else(|| anyhow::anyhow!("Friend '{}' not found", friend_alias))?
            .clone()
    };

    let addr = friend
        .cached_self_info
        .as_ref()
        .and_then(|info| info.endpoint_addrs.first().cloned())
        .ok_or_else(|| {
            anyhow::anyhow!("no known endpoint address; try 'rooms' first to refresh")
        })?;

    let pk = friend.user_public_key.to_bytes().unwrap_or([0u8; 32]);
    println!(
        "Connecting to {}...",
        playroom::identity::with_key_suffix(&friend.user_id(), &pk)
    );
    let node = NetworkNode::bind(None).await?;
    let joined = playroom::join_room::<()>(&node, addr, identity, player_user_id.to_string()).await?;

    println!("Joined room '{}'!", joined.room_name);
    println!("Members:");
    for m in &joined.members {
        let name = playroom::identity::with_key_suffix(&m.user_id, &m.user_public_key);
        let id_short = playroom::identity::short_bytes(&m.endpoint_id)
            .unwrap_or_else(|| "invalid".into());
        println!("  {name} (dev #{id_short})");
    }

    {
        let mut cfg = config.lock().await;
        let _ = cfg.update_friend_self_info(joined.host_self_info);
        let _ = cfg.save();
    }

    let tx = guest_msg_tx.clone();
    tokio::spawn(guest_recv_loop(joined.receiver, tx));

    Ok(ClientState::InRoomGuest {
        sender: joined.sender,
        room_name: joined.room_name,
        members: joined.members,
    })
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

fn parse_endpoint_addr(endpoint_id_hex: &str) -> Result<EndpointAddr, String> {
    let bytes = hex::decode(endpoint_id_hex).map_err(|e| format!("Invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err("EndpointId must be 32 bytes (64 hex chars)".into());
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    let eid = EndpointId::from_bytes(&arr).map_err(|e| format!("Invalid key: {e}"))?;
    Ok(EndpointAddr::from(eid))
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
    println!("  rooms                          - List friends' rooms");
    println!("  create <name>                  - Create and host a room");
    println!("  join <friend>                  - Join a friend's room");
    println!("  friends                        - List friends");
    println!("  add-friend <endpoint_id_hex>   - Add a friend by bootstrap EndpointId");
    println!("  devices                        - List your paired devices");
    println!("  pair-init                      - Generate an invite code for a new device");
    println!("  sync <endpoint_id_hex>         - Sync with another of your own devices");
    println!("  id                             - Show your user key and EndpointId");
    println!("  quit                           - Exit");
}

fn print_room_help() {
    println!("Room commands:");
    println!("  say <message>  - Send a chat message");
    println!("  leave          - Leave the room");
    println!("  members        - List room members");
}
