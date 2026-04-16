use serde::{Deserialize, Serialize};

/// プレイヤーの一意識別子。iroh の NodeId に対応するが、
/// playtable-core は iroh に依存しないので u64 で表現する。
/// playroom / playtable-server で EndpointId ↔ PlayerId の変換を行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "player:{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub ready: bool,
}
