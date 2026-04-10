use anyhow::Result;
use crate::{ClientMessage, PlayerInfo, ServerMessage};
use crate::player::PlayerId;
use playroom::{EndpointAddr, NetworkNode, PeerSession};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// クライアントからサーバーへのメッセージ（PlayerId 付き）
struct ClientEvent {
    player_id: PlayerId,
    message: ClientMessage,
}

pub struct GameServer {
    node: NetworkNode,
    event_tx: mpsc::Sender<ClientEvent>,
    event_rx: mpsc::Receiver<ClientEvent>,
}

impl GameServer {
    pub async fn start() -> Result<Self> {
        let node = NetworkNode::bind(None).await?;
        let (event_tx, event_rx) = mpsc::channel(256);
        Ok(Self {
            node,
            event_tx,
            event_rx,
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.node.addr()
    }

    pub async fn run(mut self) -> Result<()> {
        // 接続受け入れループ（別タスク）
        let node = self.node;
        let event_tx = self.event_tx;

        let accept_handle = tokio::spawn(async move {
            loop {
                match node
                    .accept::<ServerMessage, ClientMessage>()
                    .await
                {
                    Ok(session) => {
                        let peer_id = session.peer_id();
                        // EndpointId の上位 8 バイトを PlayerId として使用
                        let player_id = PlayerId(endpoint_id_to_u64(peer_id));
                        info!(%player_id, "New client connected");
                        let tx = event_tx.clone();
                        tokio::spawn(client_task(session, player_id, tx));
                    }
                    Err(e) => {
                        warn!("Accept error: {e}");
                        break;
                    }
                }
            }
        });

        // メインイベントループ
        let mut players: HashMap<PlayerId, PlayerInfo> = HashMap::new();

        while let Some(event) = self.event_rx.recv().await {
            handle_event(event, &mut players);
        }

        accept_handle.abort();
        Ok(())
    }
}

async fn client_task(
    mut session: PeerSession<ServerMessage, ClientMessage>,
    player_id: PlayerId,
    tx: mpsc::Sender<ClientEvent>,
) {
    loop {
        match session.recv().await {
            Ok(msg) => {
                if tx
                    .send(ClientEvent {
                        player_id,
                        message: msg,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    info!(%player_id, "Client disconnected");
}

fn handle_event(event: ClientEvent, players: &mut HashMap<PlayerId, PlayerInfo>) {
    match event.message {
        ClientMessage::JoinLobby { name } => {
            info!(player = %event.player_id, name, "Player joined lobby");
            players.insert(
                event.player_id,
                PlayerInfo {
                    id: event.player_id,
                    name,
                    ready: false,
                },
            );
        }
        ClientMessage::LeaveLobby => {
            players.remove(&event.player_id);
        }
        ClientMessage::SetReady(ready) => {
            if let Some(p) = players.get_mut(&event.player_id) {
                p.ready = ready;
            }
        }
        msg => {
            info!(player = %event.player_id, "Unhandled message: {:?}", msg);
        }
    }
}

fn endpoint_id_to_u64(id: playroom::EndpointId) -> u64 {
    let bytes = id.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap())
}
