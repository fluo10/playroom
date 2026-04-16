use serde::{Deserialize, Serialize};
use crate::player::PlayerId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Flip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Flipped {
        player: PlayerId,
        result: CoinSide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoinSide {
    Heads,
    Tails,
}

impl std::fmt::Display for CoinSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoinSide::Heads => write!(f, "heads"),
            CoinSide::Tails => write!(f, "tails"),
        }
    }
}
