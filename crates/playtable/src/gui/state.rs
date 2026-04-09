use bevy::prelude::*;
use playtable_core::{PlayerInfo, lobby::LobbySnapshot};
use playtable_core::games::{GameEvent, GameKind};

/// ゲームの現在フェーズ
#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppScreen {
    #[default]
    Connect,
    Lobby,
    InGame,
}

/// サーバーから受け取った最新のロビー状態
#[derive(Resource, Default)]
pub struct LobbyState {
    pub players: Vec<PlayerInfo>,
    pub selected_game: Option<GameKind>,
}

/// 接続設定
#[derive(Resource, Default)]
pub struct ConnectionConfig {
    pub server_addr: String,
    pub player_name: String,
}
