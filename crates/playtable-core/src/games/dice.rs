use serde::{Deserialize, Serialize};
use crate::player::PlayerId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Roll { sides: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Rolled {
        player: PlayerId,
        sides: u8,
        value: u8,
    },
}
