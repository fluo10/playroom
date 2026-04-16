//! ルーム内画面：チャット送信と退室。

use bevy::prelude::*;
use bevy_simple_text_input::{TextInputSubmitMessage, TextInputValue};

use super::super::bridge::ServiceHandle;
use super::super::state::AppScreen;
use super::util::{self, StatusBarText};

pub struct InRoomScreenPlugin;

impl Plugin for InRoomScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppScreen::InRoom), setup)
            .add_systems(OnExit(AppScreen::InRoom), cleanup)
            .add_systems(
                Update,
                (handle_send, handle_submit, handle_leave).run_if(in_state(AppScreen::InRoom)),
            );
    }
}

#[derive(Component)]
struct InRoomRoot;

#[derive(Component)]
struct ChatInput;

#[derive(Component)]
struct SendButton;

#[derive(Component)]
struct LeaveButton;

fn setup(mut commands: Commands) {
    commands
        .spawn((util::root_node(), BackgroundColor(util::COLOR_BG), InRoomRoot))
        .with_children(|root| {
            root.spawn((Text::new("Playroom — In Room"), TextColor(util::COLOR_TEXT)));
            util::dim_label(root, "Type a message and press Enter (or click Send).");

            util::spawn_text_input(root, "message", ChatInput);

            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(util::COLOR_BG),
            ))
            .with_children(|row| {
                util::spawn_button(row, "Send", SendButton);
                util::spawn_button(row, "Leave", LeaveButton);
            });

            root.spawn((Text::new(""), TextColor(util::COLOR_DIM), StatusBarText));
        });
}

fn cleanup(mut commands: Commands, q: Query<Entity, With<InRoomRoot>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn handle_send(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<SendButton>)>,
    mut inputs: Query<&mut TextInputValue, With<ChatInput>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(mut value) = inputs.iter_mut().next() else { continue };
        let content = value.0.trim().to_string();
        if content.is_empty() {
            continue;
        }
        value.0.clear();
        dispatch_send(&handle, content);
    }
}

fn handle_submit(
    handle: Res<ServiceHandle>,
    mut submits: MessageReader<TextInputSubmitMessage>,
    inputs: Query<Entity, With<ChatInput>>,
) {
    let Some(chat_entity) = inputs.iter().next() else { return };
    for event in submits.read() {
        if event.entity != chat_entity {
            continue;
        }
        let content = event.value.trim().to_string();
        if !content.is_empty() {
            dispatch_send(&handle, content);
        }
    }
}

fn handle_leave(
    handle: Res<ServiceHandle>,
    buttons: Query<&Interaction, (Changed<Interaction>, With<LeaveButton>)>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let svc = handle.service.clone();
        handle.dispatch("leave_room", async move {
            svc.leave_room()
                .await
                .map(|_| "left room".to_string())
                .map_err(|e| e.to_string())
        });
    }
}

fn dispatch_send(handle: &ServiceHandle, content: String) {
    let svc = handle.service.clone();
    handle.dispatch("send_message", async move {
        svc.send_message(&content)
            .await
            .map(|_| "sent".to_string())
            .map_err(|e| e.to_string())
    });
}
