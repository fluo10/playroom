use bevy::prelude::*;
use super::state::AppScreen;

mod connect;
mod lobby;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppScreen>()
            .add_plugins(connect::ConnectScreenPlugin)
            .add_plugins(lobby::LobbyScreenPlugin);
    }
}
