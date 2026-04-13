mod endpoint;
mod error;
mod framing;
mod session;

pub mod config;
pub mod host;
pub mod protocol;

pub use endpoint::NetworkNode;
pub use error::NetworkError;
pub use framing::{FramedReceiver, FramedSender};
pub use session::PeerSession;

// iroh の識別子型を再エクスポート
pub use iroh::{EndpointAddr, EndpointId};

// プロトコル型の再エクスポート
pub use protocol::{ClientMessage, HostMessage, MemberInfo};
pub use host::{HostCommand, RoomEvent, RoomHost, RoomHostHandle};
pub use config::{AppConfig, FriendEntry};

/// このアプリケーションの ALPN 識別子
pub const ALPN: &[u8] = b"playtable/1";
