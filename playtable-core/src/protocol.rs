use serde::{Deserialize, Serialize};
use crate::games::{GameAction, GameEvent, GameKind, LegalAction};
use crate::lobby::LobbySnapshot;
use crate::player::{PlayerId, PlayerInfo};

/// クライアント（GUI / MCP）からサーバーへ送るメッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    // ロビー
    JoinLobby { name: String },
    LeaveLobby,
    SetReady(bool),
    SelectGame(GameKind),

    // ゲーム中
    GameAction(GameAction),
}

/// サーバーからクライアントへ送るメッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    // ロビー — 初回のみ全スナップショット、以降は差分
    LobbyState(LobbySnapshot),
    PlayerJoined(PlayerInfo),
    PlayerLeft(PlayerId),
    GameStarting {
        game: GameKind,
        players: Vec<PlayerInfo>,
    },

    // ゲーム中
    /// 全プレイヤーへブロードキャストする差分イベント
    GameEvent(GameEvent),
    /// 手番プレイヤーへのみユニキャスト。合法手リスト付き
    YourTurn { actions: Vec<LegalAction> },
    GameOver(GameResult),

    // エラー
    Rejected { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<PlayerId>,
    pub scores: Vec<(PlayerId, i64)>,
}
