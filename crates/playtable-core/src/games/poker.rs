use serde::{Deserialize, Serialize};
use crate::player::PlayerId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Bet { amount: u64 },
    Call,
    Check,
    Fold,
    AllIn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    CardDealt {
        player: PlayerId,
        /// 公開カード（他プレイヤーには None）
        #[serde(skip_serializing_if = "Option::is_none")]
        card: Option<Card>,
    },
    PlayerActed {
        player: PlayerId,
        action: Action,
    },
    CommunityCardRevealed {
        card: Card,
    },
    PotAwarded {
        player: PlayerId,
        amount: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub suit: Suit,
    pub rank: Rank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Suit {
    Spades,
    Hearts,
    Diamonds,
    Clubs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rank {
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Jack,
    Queen,
    King,
    Ace,
}
