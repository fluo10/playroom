use bevy::prelude::*;

use crate::service::AppState;

/// Bevy 側の画面遷移 State。AppState をそのままマップする。
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppScreen {
    #[default]
    Bootstrap,
    MainMenu,
    InRoom,
}

impl From<AppState> for AppScreen {
    fn from(value: AppState) -> Self {
        match value {
            AppState::Uninitialized => AppScreen::Bootstrap,
            AppState::MainMenu => AppScreen::MainMenu,
            AppState::InRoomHost | AppState::InRoomGuest => AppScreen::InRoom,
        }
    }
}

/// 直近の操作結果テキスト（成功/失敗どちらも）。
#[derive(Resource, Default)]
pub struct ServiceStatus {
    pub text: String,
}
