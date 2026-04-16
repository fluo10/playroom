use std::sync::Arc;

use anyhow::Result;
use playroom::{AppConfig, HostMessage, RoomEvent};
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::Mutex;

use crate::service::{AppEvent, AppService, AppState};

pub async fn run() -> Result<()> {
    let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    let (service, mut events) = AppService::new(config.clone())
        .await
        .map_err(|e| anyhow::anyhow!("service init: {e}"))?;

    // 未初期化なら対話的にブートストラップ（service 経由）
    if service.state().await == AppState::Uninitialized {
        bootstrap_interactive(&service, &mut lines).await?;
    }

    println!("=== Playroom CLI ===");
    {
        let cfg = config.lock().await;
        println!("user_id: {}", cfg.user_id);
    }
    println!();
    print_help_for(service.state().await);

    loop {
        print_prompt(service.state().await);

        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim().to_string();
                if line.is_empty() { continue; }
                match handle_command(&service, &line).await {
                    CommandOutcome::Continue => {}
                    CommandOutcome::Quit => break,
                }
            }
            Some(event) = events.recv() => {
                print_event(&event);
            }
        }
    }

    println!("Goodbye!");
    Ok(())
}

enum CommandOutcome {
    Continue,
    Quit,
}

fn print_prompt(state: AppState) {
    match state {
        AppState::Uninitialized => eprint!("[init] > "),
        AppState::MainMenu => eprint!("> "),
        AppState::InRoomHost => eprint!("[host] > "),
        AppState::InRoomGuest => eprint!("[guest] > "),
    }
}

fn print_event(event: &AppEvent) {
    match event {
        AppEvent::Room(ev) => match ev {
            RoomEvent::MemberJoined { user_id, .. } => println!("* {user_id} joined the room"),
            RoomEvent::MemberLeft { user_id, .. } => println!("* {user_id} left the room"),
            RoomEvent::ChatReceived { from, content } => println!("[chat] {from}: {content}"),
            RoomEvent::RoomCreated { name } => println!("* Room '{name}' created"),
            RoomEvent::RoomClosed => println!("* Room closed"),
            RoomEvent::PairingWindowOpened => {}
            RoomEvent::PairingWindowClosed => println!("* Pairing window closed"),
            RoomEvent::PairingCompleted { device_label, .. } => {
                println!("* Paired with new device: {device_label}")
            }
            RoomEvent::SyncCompleted { .. } => println!("* Sync completed"),
        },
        AppEvent::Guest(msg) => match msg {
            HostMessage::Chat { from, content } => println!("[chat] {from}: {content}"),
            HostMessage::MemberJoined(info) => println!("* {} joined the room", info.user_id),
            HostMessage::MemberLeft { user_id, .. } => println!("* {user_id} left the room"),
            HostMessage::RoomClosed => println!("* Room has been closed by the host"),
            HostMessage::Rejected { reason } => eprintln!("Rejected: {reason}"),
            _ => {}
        },
        AppEvent::StateChanged(state) => {
            print_help_for(*state);
        }
    }
}

async fn handle_command(service: &Arc<AppService>, line: &str) -> CommandOutcome {
    let state = service.state().await;
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim());

    if matches!(cmd, "quit" | "exit") {
        return CommandOutcome::Quit;
    }
    if cmd == "help" {
        print_help_for(state);
        return CommandOutcome::Continue;
    }

    let result: Result<(), String> = match (state, cmd) {
        (AppState::MainMenu, "rooms") => service
            .list_rooms()
            .await
            .map(|listing| {
                if listing.rooms.is_empty() {
                    println!("  No rooms found.");
                } else {
                    println!("Available rooms:");
                    for r in &listing.rooms {
                        println!(
                            "  [{}] {} ({} members: {})",
                            r.friend_display,
                            r.room_name,
                            r.members.len(),
                            r.members.join(", ")
                        );
                    }
                }
            })
            .map_err(|e| e.to_string()),
        (AppState::MainMenu, "create") => service
            .create_room(arg.unwrap_or("My Room"))
            .await
            .map(|r| println!("Room '{}' created.", r.name))
            .map_err(|e| e.to_string()),
        (AppState::MainMenu, "join") => {
            let Some(friend) = arg else {
                eprintln!("Usage: join <friend>");
                return CommandOutcome::Continue;
            };
            service
                .join_room(friend)
                .await
                .map(|j| {
                    println!("Joined room '{}'!", j.room_name);
                    for m in &j.members {
                        let name =
                            playroom::identity::with_key_suffix(&m.user_id, &m.user_public_key);
                        let id_short = playroom::identity::short_bytes(&m.endpoint_id)
                            .unwrap_or_else(|| "invalid".into());
                        println!("  {name} (dev #{id_short})");
                    }
                })
                .map_err(|e| e.to_string())
        }
        (AppState::MainMenu, "friends") => service
            .list_friends()
            .await
            .map(|listing| {
                if listing.friends.is_empty() {
                    println!("No friends registered.");
                } else {
                    println!("Friends:");
                    for f in &listing.friends {
                        println!(
                            "  {}",
                            playroom::identity::with_key_suffix(&f.user_id, &f.user_public_key)
                        );
                    }
                }
            })
            .map_err(|e| e.to_string()),
        (AppState::MainMenu, "add-friend") => {
            let Some(eid) = arg else {
                eprintln!("Usage: add-friend <endpoint_id_hex>");
                return CommandOutcome::Continue;
            };
            service
                .add_friend(eid)
                .await
                .map(|info| println!("Friend '{}' added.", info.user_id))
                .map_err(|e| e.to_string())
        }
        (AppState::MainMenu, "devices") => service
            .list_devices()
            .await
            .map(|listing| {
                if listing.devices.is_empty() {
                    println!("No devices registered.");
                } else {
                    println!("Your devices:");
                    for d in &listing.devices {
                        println!("  {} (#{})", d.label, d.endpoint_id_short);
                    }
                }
            })
            .map_err(|e| e.to_string()),
        (AppState::MainMenu, "pair-init") => service
            .pair_init()
            .await
            .map(|r| {
                println!("Pairing window open for {}s.", r.ttl_secs);
                println!("On the new device, paste:");
                println!("  {}", r.invite_code);
            })
            .map_err(|e| e.to_string()),
        (AppState::MainMenu, "sync") => {
            let Some(eid) = arg else {
                eprintln!("Usage: sync <endpoint_id_hex>");
                return CommandOutcome::Continue;
            };
            service
                .sync_device(eid)
                .await
                .map(|r| {
                    if r.changed {
                        println!("Sync completed with changes.");
                    } else {
                        println!("Sync completed (no changes).");
                    }
                })
                .map_err(|e| e.to_string())
        }

        (AppState::InRoomHost | AppState::InRoomGuest, "say") => {
            let Some(content) = arg else {
                eprintln!("Usage: say <message>");
                return CommandOutcome::Continue;
            };
            service
                .send_message(content)
                .await
                .map(|_| {})
                .map_err(|e| e.to_string())
        }
        (AppState::InRoomHost | AppState::InRoomGuest, "leave") => service
            .leave_room()
            .await
            .map(|_| {})
            .map_err(|e| e.to_string()),

        _ => {
            eprintln!("Unknown command: {cmd}. Type 'help' for available commands.");
            return CommandOutcome::Continue;
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }
    CommandOutcome::Continue
}

fn print_help_for(state: AppState) {
    match state {
        AppState::Uninitialized => {
            println!("(uninitialized — waiting for bootstrap)");
        }
        AppState::MainMenu => {
            println!("Commands:");
            println!("  rooms                          - List friends' rooms");
            println!("  create <name>                  - Create and host a room");
            println!("  join <friend>                  - Join a friend's room");
            println!("  friends                        - List friends");
            println!("  add-friend <endpoint_id_hex>   - Add a friend by bootstrap EndpointId");
            println!("  devices                        - List your paired devices");
            println!("  pair-init                      - Generate an invite code for a new device");
            println!("  sync <endpoint_id_hex>         - Sync with another of your own devices");
            println!("  quit                           - Exit");
        }
        AppState::InRoomHost | AppState::InRoomGuest => {
            println!("Room commands:");
            println!("  say <message>  - Send a chat message");
            println!("  leave          - Leave the room");
        }
    }
}

// ── 対話的ブートストラップ（初回起動時）: service.create_user / pair_device を呼ぶ ──

async fn bootstrap_interactive(
    service: &Arc<AppService>,
    lines: &mut Lines<BufReader<Stdin>>,
) -> Result<()> {
    println!("=== Playroom first-launch setup ===");
    println!("[n]ew user (create a new identity)");
    println!("[e]xisting user (adopt an identity from another device via invite)");

    loop {
        eprint!("Choice [n/e]: ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed before setup completed");
        };
        match line.trim().to_lowercase().as_str() {
            "n" | "new" => {
                let user_id = prompt_user_id(lines).await?;
                match service.create_user(&user_id).await {
                    Ok(r) => {
                        println!(
                            "Created user '{}'.\n  user key: {}\n  EndpointId: {}",
                            r.user_id,
                            hex::encode(r.user_public_key),
                            hex::encode(r.endpoint_id),
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        // 続行（ユーザーはリトライ可能）
                    }
                }
            }
            "e" | "existing" => {
                let invite = prompt_invite(lines).await?;
                let label = prompt_label(lines).await?;
                println!("Connecting to existing device...");
                match service.pair_device(&invite, &label).await {
                    Ok(r) => {
                        println!(
                            "Paired successfully as '{}'. Device labeled '{}'.",
                            r.user_id, r.device_label
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                    }
                }
            }
            _ => eprintln!("Please enter 'n' or 'e'."),
        }
    }
}

async fn prompt_user_id(lines: &mut Lines<BufReader<Stdin>>) -> Result<String> {
    loop {
        eprint!("Enter user_id (alphanumeric, 1-32 chars): ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed");
        };
        let id = line.trim().to_string();
        if playroom::identity::is_valid_user_id(&id) {
            return Ok(id);
        }
        eprintln!("Invalid user_id. Must be 1-32 ASCII alphanumeric characters.");
    }
}

async fn prompt_invite(lines: &mut Lines<BufReader<Stdin>>) -> Result<String> {
    loop {
        eprint!("Paste invite code (77 chars): ");
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("stdin closed");
        };
        let t = line.trim();
        if t.parse::<playroom::Invite>().is_ok() {
            return Ok(t.to_string());
        }
        eprintln!("Invalid invite code.");
    }
}

async fn prompt_label(lines: &mut Lines<BufReader<Stdin>>) -> Result<String> {
    eprint!("Device label (e.g. 'Laptop'): ");
    let Some(line) = lines.next_line().await? else {
        anyhow::bail!("stdin closed");
    };
    let t = line.trim();
    Ok(if t.is_empty() {
        "New Device".to_string()
    } else {
        t.to_string()
    })
}
