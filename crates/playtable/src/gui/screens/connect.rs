use bevy::prelude::*;
use super::super::state::AppScreen;

pub struct ConnectScreenPlugin;

impl Plugin for ConnectScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppScreen::Connect), setup_connect_screen)
            .add_systems(OnExit(AppScreen::Connect), cleanup_connect_screen);
    }
}

#[derive(Component)]
struct ConnectScreenMarker;

fn setup_connect_screen(mut commands: Commands) {
    commands.spawn((
        Text::new("Playtable\n\nEnter server address to connect"),
        ConnectScreenMarker,
    ));
}

fn cleanup_connect_screen(
    mut commands: Commands,
    query: Query<Entity, With<ConnectScreenMarker>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}
