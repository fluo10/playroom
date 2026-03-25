mod endpoint;
mod error;
mod framing;
mod session;

pub use endpoint::NetworkNode;
pub use error::NetworkError;
pub use session::PeerSession;

// iroh の識別子型を再エクスポート
pub use iroh::{EndpointAddr, EndpointId};

/// このアプリケーションの ALPN 識別子
pub const ALPN: &[u8] = b"playtable/1";
