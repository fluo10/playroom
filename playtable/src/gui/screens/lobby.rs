use bevy::prelude::*;
use super::super::state::AppScreen;

pub struct LobbyScreenPlugin;

impl Plugin for LobbyScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppScreen::Lobby), setup_lobby_screen)
            .add_systems(OnExit(AppScreen::Lobby), cleanup_lobby_screen);
    }
}

#[derive(Component)]
struct LobbyScreenMarker;

fn setup_lobby_screen(mut commands: Commands) {
    commands.spawn((
        Text::new("Lobby"),
        LobbyScreenMarker,
    ));
}

fn cleanup_lobby_screen(
    mut commands: Commands,
    query: Query<Entity, With<LobbyScreenMarker>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}
