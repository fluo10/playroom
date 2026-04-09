use iroh::{Endpoint, EndpointAddr, EndpointId, endpoint::presets};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{ALPN, NetworkError, session::PeerSession};

/// iroh Endpoint のラッパー。ゲーム固有の概念を持たない汎用ノード。
pub struct NetworkNode {
    endpoint: Endpoint,
}

impl NetworkNode {
    /// ノードを起動し、接続の受け入れを開始する。
    /// `alpn` が `None` の場合はデフォルトの `ALPN` を使用する。
    pub async fn bind(alpn: Option<&[u8]>) -> Result<Self, NetworkError> {
        let alpn = alpn.unwrap_or(ALPN).to_vec();
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![alpn])
            .bind()
            .await
            .map_err(|e| NetworkError::Iroh(e.into()))?;
        debug!(id = %endpoint.id().fmt_short(), "NetworkNode bound");
        Ok(Self { endpoint })
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// ピアに接続し、双方向ストリームを開く。
    pub async fn connect<S, R>(
        &self,
        addr: EndpointAddr,
    ) -> Result<PeerSession<S, R>, NetworkError>
    where
        S: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let conn = self
            .endpoint
            .connect(addr, ALPN)
            .await
            .map_err(|e| NetworkError::Iroh(e.into()))?;

        let peer_id = conn.remote_id();
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| NetworkError::Iroh(e.into()))?;

        Ok(PeerSession::new(send, recv, peer_id))
    }

    /// 次の接続を受け入れ、双方向ストリームを返す。
    pub async fn accept<S, R>(&self) -> Result<PeerSession<S, R>, NetworkError>
    where
        S: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or(NetworkError::Closed)?;

        let accepting = incoming
            .accept()
            .map_err(|e| NetworkError::Iroh(e.into()))?;

        let conn = accepting
            .await
            .map_err(|e| NetworkError::Iroh(e.into()))?;

        let peer_id = conn.remote_id();
        let (send, recv) = conn
            .accept_bi()
            .await
            .map_err(|e| NetworkError::Iroh(e.into()))?;

        Ok(PeerSession::new(send, recv, peer_id))
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}
