use anyhow::{Context, Result};
use playtable_core::{ClientMessage, ServerMessage};
use playroom::{EndpointAddr, NetworkNode, PeerSession};

/// ゲームサーバーへの接続と MCP ツール層を橋渡しする
pub struct PlaytableBridge {
    session: PeerSession<ClientMessage, ServerMessage>,
}

impl PlaytableBridge {
    pub async fn connect(server_addr: &str, name: &str) -> Result<Self> {
        // EndpointAddr は JSON でシリアライズされた文字列として受け渡す
        let addr: EndpointAddr = serde_json::from_str(server_addr)
            .context("Invalid server EndpointAddr (expected JSON)")?;

        let node = NetworkNode::bind(None).await?;
        let mut session: PeerSession<ClientMessage, ServerMessage> =
            node.connect(addr).await?;

        // ロビーに参加
        session
            .send(&ClientMessage::JoinLobby {
                name: name.to_string(),
            })
            .await?;

        Ok(Self { session })
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        self.session.send(msg).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<ServerMessage> {
        Ok(self.session.recv().await?)
    }
}
