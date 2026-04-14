use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::framing::{FramedReceiver, FramedSender};
use crate::identity::{self, UserIdentity, UserPublicKey};
use crate::protocol::{
    ClientMessage, FriendSelfInfo, HostMessage, MemberInfo, RoomSummary, PROTOCOL_VERSION,
};
use crate::{EndpointId, NetworkNode, PeerSession};

/// UI 層に公開するイベント
#[derive(Debug, Clone)]
pub enum RoomEvent {
    MemberJoined {
        name: String,
        user_public_key: UserPublicKey,
    },
    MemberLeft {
        name: String,
        user_public_key: UserPublicKey,
    },
    ChatReceived {
        from: String,
        content: String,
    },
    RoomCreated {
        name: String,
    },
    RoomClosed,
}

/// ルーム内メンバーの状態（ホスト側が管理）
struct MemberState<T: Serialize> {
    name: String,
    user_public_key: UserPublicKey,
    sender: FramedSender<HostMessage<T>>,
}

/// ルームの状態
struct RoomState<T: Serialize> {
    room_name: String,
    members: HashMap<EndpointId, MemberState<T>>,
}

/// 認証済みの参加要求
struct AuthenticatedJoin<T> {
    endpoint_id: EndpointId,
    name: String,
    user_public_key: UserPublicKey,
    sender: FramedSender<HostMessage<T>>,
}

/// accept loop からメインループへのイベント
enum InternalEvent<T> {
    /// ルーム情報の問い合わせ（認証不要。応答後切断）
    QueryRoom {
        sender: FramedSender<HostMessage<T>>,
    },
    /// 認証済みの新規参加
    NewJoin(AuthenticatedJoin<T>),
    ClientMessage {
        endpoint_id: EndpointId,
        message: ClientMessage<T>,
    },
    ClientDisconnected {
        endpoint_id: EndpointId,
    },
}

/// UI 層からホストへのコマンド
#[derive(Debug)]
pub enum HostCommand {
    CreateRoom { name: String },
    CloseRoom,
    Chat { content: String },
}

/// ホストを起動する際に返されるハンドル
pub struct RoomHostHandle {
    pub commands: mpsc::Sender<HostCommand>,
    pub endpoint_id: EndpointId,
    pub endpoint_addr: crate::EndpointAddr,
}

/// P2P ルームホスト。1 ノードにつき 1 ルームをホストする。
pub struct RoomHost<T = ()>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + Send + 'static,
{
    host_name: String,
    identity: Arc<UserIdentity>,
    host_addr: crate::EndpointAddr,
    self_info_version: u64,
    room: Option<RoomState<T>>,
    internal_rx: mpsc::Receiver<InternalEvent<T>>,
    event_tx: mpsc::Sender<RoomEvent>,
    cmd_rx: mpsc::Receiver<HostCommand>,
    node_id: EndpointId,
}

impl<T> RoomHost<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + Send + Sync + std::fmt::Debug + 'static,
{
    /// ノードを起動し、accept loop を開始する。
    pub async fn start(
        host_name: String,
        identity: Arc<UserIdentity>,
        event_tx: mpsc::Sender<RoomEvent>,
    ) -> anyhow::Result<(Self, RoomHostHandle)> {
        let node = NetworkNode::bind(None).await?;
        let endpoint_id = node.id();
        let endpoint_addr = node.addr();
        let (internal_tx, internal_rx) = mpsc::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        let accept_tx = internal_tx.clone();
        tokio::spawn(Self::accept_loop(node, accept_tx));

        let host = Self {
            host_name,
            identity,
            host_addr: endpoint_addr.clone(),
            self_info_version: crate::config::now_unix_ms(),
            room: None,
            internal_rx,
            event_tx,
            cmd_rx,
            node_id: endpoint_id,
        };
        let handle = RoomHostHandle {
            commands: cmd_tx,
            endpoint_id,
            endpoint_addr,
        };
        Ok((host, handle))
    }

    fn create_room(&mut self, name: String) {
        info!(room = %name, "Creating room");
        self.room = Some(RoomState {
            room_name: name.clone(),
            members: HashMap::new(),
        });
        let _ = self.event_tx.try_send(RoomEvent::RoomCreated { name });
    }

    async fn close_room(&mut self) {
        if let Some(mut room) = self.room.take() {
            info!(room = %room.room_name, "Closing room");
            for (_, member) in room.members.iter_mut() {
                let _ = member.sender.send(&HostMessage::RoomClosed).await;
            }
            let _ = self.event_tx.try_send(RoomEvent::RoomClosed);
        }
    }

    async fn accept_loop(node: NetworkNode, tx: mpsc::Sender<InternalEvent<T>>) {
        loop {
            match node.accept::<HostMessage<T>, ClientMessage<T>>().await {
                Ok(session) => {
                    let tx = tx.clone();
                    tokio::spawn(Self::handle_new_connection(session, tx));
                }
                Err(e) => {
                    warn!("Accept error: {e}");
                    break;
                }
            }
        }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(event) = self.internal_rx.recv() => {
                    self.handle_internal_event(event).await;
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                else => break,
            }
        }
        Ok(())
    }

    /// 新規接続のハンドリング。
    /// QueryRoom は即応答で終了、JoinRequest はチャレンジ認証を済ませてから内部イベントを送る。
    async fn handle_new_connection(
        session: PeerSession<HostMessage<T>, ClientMessage<T>>,
        tx: mpsc::Sender<InternalEvent<T>>,
    ) {
        let (mut sender, mut receiver, endpoint_id) = session.split();

        let first = match receiver.recv().await {
            Ok(m) => m,
            Err(e) => {
                warn!(peer = %endpoint_id.fmt_short(), "Failed to read first message: {e}");
                return;
            }
        };

        match first {
            ClientMessage::QueryRoom => {
                let _ = tx.send(InternalEvent::QueryRoom { sender }).await;
            }
            ClientMessage::JoinRequest {
                name,
                user_public_key,
                protocol_version,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    let _ = sender
                        .send(&HostMessage::Rejected {
                            reason: format!(
                                "protocol version mismatch: expected {}, got {}",
                                PROTOCOL_VERSION, protocol_version
                            ),
                        })
                        .await;
                    return;
                }

                // チャレンジを生成して送信
                let mut nonce = [0u8; 32];
                if getrandom::fill(&mut nonce).is_err() {
                    let _ = sender
                        .send(&HostMessage::Rejected {
                            reason: "failed to generate nonce".into(),
                        })
                        .await;
                    return;
                }
                if sender.send(&HostMessage::Challenge { nonce }).await.is_err() {
                    return;
                }

                // 署名応答を待つ
                let resp = match receiver.recv().await {
                    Ok(ClientMessage::JoinResponse { signature }) => signature,
                    Ok(_) => {
                        let _ = sender
                            .send(&HostMessage::Rejected {
                                reason: "expected JoinResponse".into(),
                            })
                            .await;
                        return;
                    }
                    Err(_) => return,
                };

                // 署名を検証
                if !identity::verify_signature(&user_public_key, &nonce, &resp) {
                    let _ = sender
                        .send(&HostMessage::Rejected {
                            reason: "signature verification failed".into(),
                        })
                        .await;
                    return;
                }

                // 認証成功：メインループに委譲
                let auth = AuthenticatedJoin {
                    endpoint_id,
                    name: name.clone(),
                    user_public_key,
                    sender,
                };
                if tx.send(InternalEvent::NewJoin(auth)).await.is_err() {
                    return;
                }

                // 以降のメッセージを転送するタスク
                Self::client_recv_loop(endpoint_id, receiver, tx).await;
            }
            _ => {
                let _ = sender
                    .send(&HostMessage::Rejected {
                        reason: "expected QueryRoom or JoinRequest as first message".into(),
                    })
                    .await;
            }
        }
    }

    async fn client_recv_loop(
        endpoint_id: EndpointId,
        mut receiver: FramedReceiver<ClientMessage<T>>,
        tx: mpsc::Sender<InternalEvent<T>>,
    ) {
        loop {
            match receiver.recv().await {
                Ok(msg) => {
                    if tx
                        .send(InternalEvent::ClientMessage {
                            endpoint_id,
                            message: msg,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx
                        .send(InternalEvent::ClientDisconnected { endpoint_id })
                        .await;
                    break;
                }
            }
        }
    }

    async fn handle_internal_event(&mut self, event: InternalEvent<T>) {
        match event {
            InternalEvent::QueryRoom { mut sender } => {
                let host_self_info = self.build_host_self_info();
                let room = self.room.as_ref().map(|r| RoomSummary {
                    name: r.room_name.clone(),
                    members: r.members.values().map(|m| m.name.clone()).collect(),
                });
                let msg: HostMessage<T> = HostMessage::QueryRoomResponse {
                    host_self_info,
                    room,
                };
                let _ = sender.send(&msg).await;
                // sender drop で切断
            }
            InternalEvent::NewJoin(auth) => {
                self.handle_join(auth).await;
            }
            InternalEvent::ClientMessage {
                endpoint_id,
                message,
            } => {
                self.handle_client_message(endpoint_id, message).await;
            }
            InternalEvent::ClientDisconnected { endpoint_id } => {
                self.handle_disconnect(endpoint_id).await;
            }
        }
    }

    /// ホスト自身の FriendSelfInfo を生成する（Welcome に同梱）。
    fn build_host_self_info(&self) -> FriendSelfInfo {
        FriendSelfInfo::sign_new(
            &self.identity,
            self.host_name.clone(),
            vec![self.host_addr.clone()],
            self.self_info_version,
        )
    }

    async fn handle_join(&mut self, auth: AuthenticatedJoin<T>) {
        let AuthenticatedJoin {
            endpoint_id,
            name,
            user_public_key,
            mut sender,
        } = auth;

        let host_self_info = self.build_host_self_info();

        let Some(room) = &mut self.room else {
            let _ = sender
                .send(&HostMessage::Rejected {
                    reason: "No room is hosted".to_string(),
                })
                .await;
            return;
        };

        info!(
            peer = %endpoint_id.fmt_short(),
            name = %name,
            user_key = %identity::short_key(&user_public_key),
            "Player joining room"
        );

        // 既存メンバー一覧を Welcome で送信
        let mut members: Vec<MemberInfo> = room
            .members
            .iter()
            .map(|(eid, m)| MemberInfo {
                name: m.name.clone(),
                user_public_key: m.user_public_key,
                endpoint_id: eid.as_bytes().to_vec(),
            })
            .collect();
        // ホスト自身もメンバー一覧に含める
        members.push(MemberInfo {
            name: self.host_name.clone(),
            user_public_key: self.identity.public_key(),
            endpoint_id: self.node_id.as_bytes().to_vec(),
        });

        let _ = sender
            .send(&HostMessage::Welcome {
                room_name: room.room_name.clone(),
                members,
                host_self_info,
            })
            .await;

        // 既存メンバーに通知
        let new_member_info = MemberInfo {
            name: name.clone(),
            user_public_key,
            endpoint_id: endpoint_id.as_bytes().to_vec(),
        };
        for member in room.members.values_mut() {
            let _ = member
                .sender
                .send(&HostMessage::MemberJoined(new_member_info.clone()))
                .await;
        }

        room.members.insert(
            endpoint_id,
            MemberState {
                name: name.clone(),
                user_public_key,
                sender,
            },
        );

        let _ = self.event_tx.try_send(RoomEvent::MemberJoined {
            name,
            user_public_key,
        });
    }

    async fn handle_client_message(&mut self, endpoint_id: EndpointId, message: ClientMessage<T>) {
        match message {
            ClientMessage::Chat { content } => {
                let Some(room) = &mut self.room else { return };
                let from = room
                    .members
                    .get(&endpoint_id)
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                let chat_msg = HostMessage::Chat {
                    from: from.clone(),
                    content: content.clone(),
                };
                for (eid, member) in room.members.iter_mut() {
                    if *eid != endpoint_id {
                        let _ = member.sender.send(&chat_msg).await;
                    }
                }

                let _ = self
                    .event_tx
                    .try_send(RoomEvent::ChatReceived { from, content });
            }
            ClientMessage::Leave => {
                self.handle_disconnect(endpoint_id).await;
            }
            ClientMessage::InRoom(_) => {
                // 将来のゲーム別処理用
            }
            ClientMessage::QueryRoom => {
                // handle_new_connection 側で処理済み
            }
            _ => {
                // JoinRequest, JoinResponse はルーム外の手続きのみ
            }
        }
    }

    async fn handle_disconnect(&mut self, endpoint_id: EndpointId) {
        let Some(room) = &mut self.room else { return };
        if let Some(member) = room.members.remove(&endpoint_id) {
            info!(peer = %endpoint_id.fmt_short(), name = %member.name, "Player left room");

            let left_msg = HostMessage::MemberLeft {
                name: member.name.clone(),
                user_public_key: member.user_public_key,
            };
            for m in room.members.values_mut() {
                let _ = m.sender.send(&left_msg).await;
            }

            let _ = self.event_tx.try_send(RoomEvent::MemberLeft {
                name: member.name,
                user_public_key: member.user_public_key,
            });
        }
    }

    async fn handle_command(&mut self, cmd: HostCommand) {
        match cmd {
            HostCommand::CreateRoom { name } => {
                self.create_room(name);
            }
            HostCommand::CloseRoom => {
                self.close_room().await;
            }
            HostCommand::Chat { content } => {
                let Some(room) = &mut self.room else { return };
                let chat_msg = HostMessage::Chat {
                    from: self.host_name.clone(),
                    content,
                };
                for member in room.members.values_mut() {
                    let _ = member.sender.send(&chat_msg).await;
                }
            }
        }
    }
}
