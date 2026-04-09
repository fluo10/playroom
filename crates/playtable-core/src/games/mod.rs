pub mod coin;
pub mod dice;
pub mod poker;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameKind {
    CoinFlip,
    Dice,
    Poker,
}

/// ゲーム固有アクション（クライアント → サーバー）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameAction {
    CoinFlip(coin::Action),
    Dice(dice::Action),
    Poker(poker::Action),
}

/// ゲーム固有イベント（サーバー → 全クライアント、差分のみ）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameEvent {
    CoinFlip(coin::Event),
    Dice(dice::Event),
    Poker(poker::Event),
}

/// 合法手（YourTurn に同梱）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LegalAction {
    CoinFlip(coin::Action),
    Dice(dice::Action),
    Poker(poker::Action),
}
