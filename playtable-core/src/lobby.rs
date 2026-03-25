use serde::{Deserialize, Serialize};
use crate::player::PlayerInfo;
use crate::games::GameKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbySnapshot {
    pub players: Vec<PlayerInfo>,
    pub selected_game: Option<GameKind>,
}
