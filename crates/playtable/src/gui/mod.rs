use bevy::prelude::*;

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Playtable".into(),
                resolution: (900u32, 720u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(playroom::gui::PlayroomLobbyPlugin)
        .run();
}
