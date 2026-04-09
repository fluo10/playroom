use bevy::prelude::*;

mod network;
mod screens;
mod state;

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Playtable".into(),
                resolution: (800u32, 600u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(network::NetworkPlugin)
        .add_plugins(screens::ScreensPlugin)
        .run();
}
