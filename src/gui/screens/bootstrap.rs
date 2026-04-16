//! 未初期化状態の画面。「新規ユーザー作成」と「既存ユーザーペアリング」のフォームを縦に並べる。

use bevy::prelude::*;
use bevy_simple_text_input::TextInputValue;

use super::super::bridge::ServiceHandle;
use super::super::state::AppScreen;
use super::util::{self, StatusBarText};

pub struct BootstrapScreenPlugin;

impl Plugin for BootstrapScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppScreen::Bootstrap), setup)
            .add_systems(OnExit(AppScreen::Bootstrap), cleanup)
            .add_systems(
                Update,
                (handle_create_user, handle_pair_device).run_if(in_state(AppScreen::Bootstrap)),
            );
    }
}

#[derive(Component)]
struct BootstrapRoot;

#[derive(Component)]
struct CreateUserIdInput;

#[derive(Component)]
struct CreateUserButton;

#[derive(Component)]
struct PairInviteInput;

#[derive(Component)]
struct PairLabelInput;

#[derive(Component)]
struct PairDeviceButton;

fn setup(mut commands: Commands) {
    commands
        .spawn((
            util::root_node(),
            BackgroundColor(util::COLOR_BG),
            BootstrapRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Playroom — First Launch"),
                TextColor(util::COLOR_TEXT),
            ));
            util::dim_label(root, "Fill in one of the two sections and press the button.");

            // ── Section A: create new user ──
            root.spawn((
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
            .with_children(|section| {
                util::label(section, "Create new user");
                util::dim_label(section, "user_id (1-32 alphanumeric)");
                util::spawn_text_input(section, "user_id", CreateUserIdInput);
                util::spawn_button(section, "Create User", CreateUserButton);
            });

            // ── Section B: pair existing device ──
            root.spawn((
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
            .with_children(|section| {
                util::label(section, "Adopt existing identity");
                util::dim_label(section, "invite code (77 chars from existing device)");
                util::spawn_text_input(section, "invite code", PairInviteInput);
                util::dim_label(section, "device label (e.g. Laptop)");
                util::spawn_text_input(section, "device label", PairLabelInput);
                util::spawn_button(section, "Pair Device", PairDeviceButton);
            });

            // status bar
            root.spawn((
                Text::new(""),
                TextColor(util::COLOR_DIM),
                StatusBarText,
            ));
        });
}

fn cleanup(mut commands: Commands, q: Query<Entity, With<BootstrapRoot>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn handle_create_user(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<CreateUserButton>)>,
    inputs: Query<&TextInputValue, With<CreateUserIdInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(user_id) = inputs.iter().next().map(|v| v.0.clone()) else {
            continue;
        };
        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("create_user", async move {
            svc.create_user(&user_id)
                .await
                .map(|r| format!("created user '{}'", r.user_id))
                .map_err(|e| e.to_string())
        });
    }
}

fn handle_pair_device(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<PairDeviceButton>)>,
    invite_q: Query<&TextInputValue, With<PairInviteInput>>,
    label_q: Query<&TextInputValue, (With<PairLabelInput>, Without<PairInviteInput>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(invite) = invite_q.iter().next().map(|v| v.0.clone()) else {
            continue;
        };
        let device_label = label_q
            .iter()
            .next()
            .map(|v| v.0.clone())
            .unwrap_or_default();
        let invite = invite.trim().to_string();
        if invite.is_empty() {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("pair_device", async move {
            svc.pair_device(&invite, &device_label)
                .await
                .map(|r| format!("paired as '{}' ({})", r.user_id, r.device_label))
                .map_err(|e| e.to_string())
        });
    }
}
