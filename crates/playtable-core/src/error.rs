use thiserror::Error;

#[derive(Debug, Error)]
pub enum GameError {
    #[error("not your turn")]
    NotYourTurn,
    #[error("illegal action: {0}")]
    IllegalAction(String),
    #[error("game not in progress")]
    NotInProgress,
    #[error("game already in progress")]
    AlreadyInProgress,
    #[error("player not found")]
    PlayerNotFound,
}
