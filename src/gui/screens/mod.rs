use bevy::prelude::*;

mod bootstrap;
mod in_room;
mod main_menu;
pub(super) mod util;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_camera)
            .add_plugins(bootstrap::BootstrapScreenPlugin)
            .add_plugins(main_menu::MainMenuScreenPlugin)
            .add_plugins(in_room::InRoomScreenPlugin)
            .add_systems(
                Update,
                (util::update_button_colors, util::update_status_bar),
            );
    }
}

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}
