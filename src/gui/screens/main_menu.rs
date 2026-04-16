//! メインメニュー画面：friend/room/device 操作と pair-init/sync。

use bevy::prelude::*;
use bevy_simple_text_input::TextInputValue;

use super::super::bridge::ServiceHandle;
use super::super::state::AppScreen;
use super::util::{self, StatusBarText};
use crate::service::{DeviceListing, FriendListing, RoomListing};

pub struct MainMenuScreenPlugin;

impl Plugin for MainMenuScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppScreen::MainMenu), setup)
            .add_systems(OnExit(AppScreen::MainMenu), cleanup)
            .add_systems(
                Update,
                (
                    handle_list_friends,
                    handle_list_devices,
                    handle_list_rooms,
                    handle_pair_init,
                    handle_add_friend,
                    handle_sync,
                    handle_create_room,
                    handle_join_room,
                )
                    .run_if(in_state(AppScreen::MainMenu)),
            );
    }
}

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component)]
struct ListFriendsButton;
#[derive(Component)]
struct ListDevicesButton;
#[derive(Component)]
struct ListRoomsButton;
#[derive(Component)]
struct PairInitButton;

#[derive(Component)]
struct AddFriendInput;
#[derive(Component)]
struct AddFriendButton;

#[derive(Component)]
struct SyncInput;
#[derive(Component)]
struct SyncButton;

#[derive(Component)]
struct CreateRoomInput;
#[derive(Component)]
struct CreateRoomButton;

#[derive(Component)]
struct JoinRoomInput;
#[derive(Component)]
struct JoinRoomButton;

fn setup(mut commands: Commands) {
    commands
        .spawn((
            util::root_node(),
            BackgroundColor(util::COLOR_BG),
            MainMenuRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Playroom — Main Menu"),
                TextColor(util::COLOR_TEXT),
            ));

            // list / read-only actions
            section(root, |s| {
                util::label(s, "Listings");
                util::spawn_button(s, "List Friends", ListFriendsButton);
                util::spawn_button(s, "List Devices", ListDevicesButton);
                util::spawn_button(s, "List Rooms", ListRoomsButton);
                util::spawn_button(s, "Pair Init (for new device)", PairInitButton);
            });

            // add friend
            section(root, |s| {
                util::label(s, "Add Friend");
                util::dim_label(s, "friend EndpointId (64 hex chars)");
                util::spawn_text_input(s, "endpoint_id", AddFriendInput);
                util::spawn_button(s, "Add Friend", AddFriendButton);
            });

            // sync
            section(root, |s| {
                util::label(s, "Sync with Own Device");
                util::dim_label(s, "other device EndpointId (64 hex chars)");
                util::spawn_text_input(s, "endpoint_id", SyncInput);
                util::spawn_button(s, "Sync", SyncButton);
            });

            // create room
            section(root, |s| {
                util::label(s, "Create Room");
                util::dim_label(s, "room name");
                util::spawn_text_input(s, "room name", CreateRoomInput);
                util::spawn_button(s, "Create Room", CreateRoomButton);
            });

            // join room
            section(root, |s| {
                util::label(s, "Join Friend's Room");
                util::dim_label(s, "friend user_id or key prefix");
                util::spawn_text_input(s, "friend", JoinRoomInput);
                util::spawn_button(s, "Join Room", JoinRoomButton);
            });

            root.spawn((Text::new(""), TextColor(util::COLOR_DIM), StatusBarText));
        });
}

fn section(
    parent: &mut ChildSpawnerCommands,
    build: impl FnOnce(&mut ChildSpawnerCommands),
) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(util::COLOR_PANEL),
            BorderColor::all(util::COLOR_BORDER),
        ))
        .with_children(|s| build(s));
}

fn cleanup(mut commands: Commands, q: Query<Entity, With<MainMenuRoot>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ── button handlers ──

fn handle_list_friends(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<ListFriendsButton>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("list_friends", async move {
            svc.list_friends()
                .await
                .map(format_friends)
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_list_devices(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<ListDevicesButton>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("list_devices", async move {
            svc.list_devices()
                .await
                .map(format_devices)
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_list_rooms(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<ListRoomsButton>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("list_rooms", async move {
            svc.list_rooms()
                .await
                .map(format_rooms)
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_pair_init(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<PairInitButton>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("pair_init", async move {
            svc.pair_init()
                .await
                .map(|r| format!("invite ({}s): {}", r.ttl_secs, r.invite_code))
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_add_friend(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<AddFriendButton>)>,
    inputs: Query<&TextInputValue, With<AddFriendInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(eid) = inputs.iter().next().map(|v| v.0.trim().to_string()) else {
            continue;
        };
        if eid.is_empty() {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("add_friend", async move {
            svc.add_friend(&eid)
                .await
                .map(|f| format!("added friend '{}'", f.user_id))
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_sync(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<SyncButton>)>,
    inputs: Query<&TextInputValue, With<SyncInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(eid) = inputs.iter().next().map(|v| v.0.trim().to_string()) else {
            continue;
        };
        if eid.is_empty() {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("sync_device", async move {
            svc.sync_device(&eid)
                .await
                .map(|r| {
                    if r.changed {
                        "sync completed with changes".to_string()
                    } else {
                        "sync completed (no changes)".to_string()
                    }
                })
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_create_room(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<CreateRoomButton>)>,
    inputs: Query<&TextInputValue, With<CreateRoomInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let name = inputs
            .iter()
            .next()
            .map(|v| v.0.trim().to_string())
            .unwrap_or_default();
        let name = if name.is_empty() {
            "My Room".to_string()
        } else {
            name
        };
        let svc = handle.service.clone();
        handle.dispatch("create_room", async move {
            svc.create_room(&name)
                .await
                .map(|r| format!("created room '{}'", r.name))
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_join_room(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<JoinRoomButton>)>,
    inputs: Query<&TextInputValue, With<JoinRoomInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(friend) = inputs.iter().next().map(|v| v.0.trim().to_string()) else {
            continue;
        };
        if friend.is_empty() {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("join_room", async move {
            svc.join_room(&friend)
                .await
                .map(|j| {
                    format!(
                        "joined room '{}' with {} members",
                        j.room_name,
                        j.members.len()
                    )
                })
                .map_err(|e| e.to_string())
        });
    }
}

// ── formatters ──

fn format_friends(listing: FriendListing) -> String {
    if listing.friends.is_empty() {
        return "no friends".to_string();
    }
    let names: Vec<String> = listing
        .friends
        .iter()
        .map(|f| crate::identity::with_key_suffix(&f.user_id, &f.user_public_key))
        .collect();
    format!("friends: {}", names.join(", "))
}

fn format_devices(listing: DeviceListing) -> String {
    if listing.devices.is_empty() {
        return "no devices".to_string();
    }
    let names: Vec<String> = listing
        .devices
        .iter()
        .map(|d| format!("{} #{}", d.label, d.endpoint_id_short))
        .collect();
    format!("devices: {}", names.join(", "))
}

fn format_rooms(listing: RoomListing) -> String {
    if listing.rooms.is_empty() {
        return "no rooms".to_string();
    }
    let names: Vec<String> = listing
        .rooms
        .iter()
        .map(|r| format!("[{}] {}", r.friend_display, r.room_name))
        .collect();
    format!("rooms: {}", names.join(" | "))
}
