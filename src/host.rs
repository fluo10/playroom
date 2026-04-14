use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::config::{self, AppConfig};
use crate::framing::{FramedReceiver, FramedSender};
use crate::identity::{self, UserIdentity, UserPublicKey};
use crate::protocol::{
    ClientMessage, FriendSelfInfo, HostMessage, MemberInfo, RoomSummary, SyncPayload,
    PROTOCOL_VERSION,
};
use crate::{EndpointId, NetworkNode, PeerSession};

/// UI 層に公開するイベント
#[derive(Debug, Clone)]
pub enum RoomEvent {
    MemberJoined {
        user_id: String,
        user_public_key: UserPublicKey,
    },
    MemberLeft {
        user_id: String,
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
    PairingWindowOpened {
        otp: String,
    },
    PairingWindowClosed,
    PairingCompleted {
        device_label: String,
        device_endpoint_id: EndpointId,
    },
    SyncCompleted {
        peer_endpoint_id: EndpointId,
    },
}

/// ルーム内メンバーの状態（ホスト側が管理）
struct MemberState<T: Serialize> {
    user_id: String,
    user_public_key: UserPublicKey,
    sender: FramedSender<HostMessage<T>>,
}

/// ルームの状態
struct RoomState<T: Serialize> {
    room_name: String,
    members: HashMap<EndpointId, MemberState<T>>,
}

/// ペアリング受け入れウィンドウ
struct PairingWindow {
    otp: String,
    expires_at: Instant,
}

/// 認証済みの参加要求
struct AuthenticatedJoin<T> {
    endpoint_id: EndpointId,
    user_id: String,
    user_public_key: UserPublicKey,
    sender: FramedSender<HostMessage<T>>,
}

/// accept loop からメインループへのイベント
enum InternalEvent<T> {
    QueryRoom {
        sender: FramedSender<HostMessage<T>>,
    },
    NewJoin(AuthenticatedJoin<T>),
    PairRequest {
        sender: FramedSender<HostMessage<T>>,
        endpoint_id: EndpointId,
        otp: String,
        device_label: String,
    },
    SyncRequest {
        sender: FramedSender<HostMessage<T>>,
        endpoint_id: EndpointId,
        payload: SyncPayload,
    },
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
    /// ペアリングを受け入れるウィンドウを開く（OTP と TTL を指定）
    OpenPairingWindow { otp: String, ttl: Duration },
    /// ペアリングウィンドウを閉じる
    ClosePairingWindow,
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
    host_user_id: String,
    identity: Arc<UserIdentity>,
    config: Arc<Mutex<AppConfig>>,
    host_addr: crate::EndpointAddr,
    self_info_version: u64,
    room: Option<RoomState<T>>,
    pairing_window: Option<PairingWindow>,
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
        host_user_id: String,
        identity: Arc<UserIdentity>,
        config: Arc<Mutex<AppConfig>>,
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
            host_user_id,
            identity,
            config,
            host_addr: endpoint_addr.clone(),
            self_info_version: config::now_unix_ms(),
            room: None,
            pairing_window: None,
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

    /// 新規接続のハンドリング。最初のメッセージで種類を分岐する。
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
                user_id,
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

                if !identity::verify_signature(&user_public_key, &nonce, &resp) {
                    let _ = sender
                        .send(&HostMessage::Rejected {
                            reason: "signature verification failed".into(),
                        })
                        .await;
                    return;
                }

                let auth = AuthenticatedJoin {
                    endpoint_id,
                    user_id: user_id.clone(),
                    user_public_key,
                    sender,
                };
                if tx.send(InternalEvent::NewJoin(auth)).await.is_err() {
                    return;
                }

                Self::client_recv_loop(endpoint_id, receiver, tx).await;
            }
            ClientMessage::PairRequest { otp, device_label } => {
                let _ = tx
                    .send(InternalEvent::PairRequest {
                        sender,
                        endpoint_id,
                        otp,
                        device_label,
                    })
                    .await;
            }
            ClientMessage::SyncRequest(payload) => {
                let _ = tx
                    .send(InternalEvent::SyncRequest {
                        sender,
                        endpoint_id,
                        payload,
                    })
                    .await;
            }
            _ => {
                let _ = sender
                    .send(&HostMessage::Rejected {
                        reason: "expected QueryRoom, JoinRequest, PairRequest, or SyncRequest as first message".into(),
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
                    members: r.members.values().map(|m| m.user_id.clone()).collect(),
                });
                let msg: HostMessage<T> = HostMessage::QueryRoomResponse {
                    host_self_info,
                    room,
                };
                let _ = sender.send(&msg).await;
            }
            InternalEvent::NewJoin(auth) => {
                self.handle_join(auth).await;
            }
            InternalEvent::PairRequest {
                sender,
                endpoint_id,
                otp,
                device_label,
            } => {
                self.handle_pair_request(sender, endpoint_id, otp, device_label)
                    .await;
            }
            InternalEvent::SyncRequest {
                sender,
                endpoint_id,
                payload,
            } => {
                self.handle_sync_request(sender, endpoint_id, payload).await;
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

    fn build_host_self_info(&self) -> FriendSelfInfo {
        FriendSelfInfo::sign_new(
            &self.identity,
            self.host_user_id.clone(),
            vec![self.host_addr.clone()],
            self.self_info_version,
        )
    }

    async fn handle_join(&mut self, auth: AuthenticatedJoin<T>) {
        let AuthenticatedJoin {
            endpoint_id,
            user_id,
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
            user_id = %user_id,
            user_key = %identity::short_key(&user_public_key),
            "Player joining room"
        );

        let mut members: Vec<MemberInfo> = room
            .members
            .iter()
            .map(|(eid, m)| MemberInfo {
                user_id: m.user_id.clone(),
                user_public_key: m.user_public_key,
                endpoint_id: eid.as_bytes().to_vec(),
            })
            .collect();
        members.push(MemberInfo {
            user_id: self.host_user_id.clone(),
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

        let new_member_info = MemberInfo {
            user_id: user_id.clone(),
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
                user_id: user_id.clone(),
                user_public_key,
                sender,
            },
        );

        let _ = self.event_tx.try_send(RoomEvent::MemberJoined {
            user_id,
            user_public_key,
        });
    }

    async fn handle_pair_request(
        &mut self,
        mut sender: FramedSender<HostMessage<T>>,
        endpoint_id: EndpointId,
        otp: String,
        device_label: String,
    ) {
        // OTP 検証
        let window = match &self.pairing_window {
            Some(w) if w.expires_at > Instant::now() => w,
            Some(_) => {
                self.pairing_window = None;
                let _ = sender
                    .send(&HostMessage::Rejected {
                        reason: "pairing window expired".into(),
                    })
                    .await;
                let _ = self.event_tx.try_send(RoomEvent::PairingWindowClosed);
                return;
            }
            None => {
                let _ = sender
                    .send(&HostMessage::Rejected {
                        reason: "no pairing window open".into(),
                    })
                    .await;
                return;
            }
        };

        if window.otp != otp {
            let _ = sender
                .send(&HostMessage::Rejected {
                    reason: "invalid OTP".into(),
                })
                .await;
            return;
        }

        // 成功：config のスナップショットと秘密鍵を送信、新デバイスを my_devices に追加
        let (user_id, friends, my_devices) = {
            let mut cfg = self.config.lock().await;
            // 自デバイス側にも新規デバイスを即時登録しておく（ペアリング相互性）
            cfg.upsert_my_device(hex::encode(endpoint_id.as_bytes()), device_label.clone());
            let snap = (
                cfg.user_id.clone(),
                cfg.friends.clone(),
                cfg.my_devices.clone(),
            );
            let _ = cfg.save();
            snap
        };

        let pair_accepted = HostMessage::<T>::PairAccepted {
            user_secret_key: self.identity.secret_bytes(),
            user_id,
            friends,
            my_devices,
        };
        let _ = sender.send(&pair_accepted).await;

        // ウィンドウを閉じる
        self.pairing_window = None;
        let _ = self.event_tx.try_send(RoomEvent::PairingWindowClosed);
        let _ = self.event_tx.try_send(RoomEvent::PairingCompleted {
            device_label,
            device_endpoint_id: endpoint_id,
        });
    }

    async fn handle_sync_request(
        &mut self,
        mut sender: FramedSender<HostMessage<T>>,
        endpoint_id: EndpointId,
        payload: SyncPayload,
    ) {
        let own_key = self.identity.public_key();
        if !payload.verify_with(&own_key) {
            let _ = sender
                .send(&HostMessage::Rejected {
                    reason: "sync signature verification failed".into(),
                })
                .await;
            return;
        }

        // 相手のペイロードを自分の config にマージして、自分の新しい状態で応答する
        let response_payload = {
            let mut cfg = self.config.lock().await;
            let peer_snapshot = AppConfig {
                user_id: cfg.user_id.clone(),
                my_devices: payload.my_devices,
                friends: payload.friends,
            };
            cfg.merge_from_peer(&peer_snapshot);
            let _ = cfg.save();
            SyncPayload::sign_new(
                &self.identity,
                cfg.my_devices.clone(),
                cfg.friends.clone(),
            )
        };

        let _ = sender
            .send(&HostMessage::SyncResponse(response_payload))
            .await;

        let _ = self
            .event_tx
            .try_send(RoomEvent::SyncCompleted { peer_endpoint_id: endpoint_id });
    }

    async fn handle_client_message(&mut self, endpoint_id: EndpointId, message: ClientMessage<T>) {
        match message {
            ClientMessage::Chat { content } => {
                let Some(room) = &mut self.room else { return };
                let from = room
                    .members
                    .get(&endpoint_id)
                    .map(|m| m.user_id.clone())
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
            ClientMessage::InRoom(_) => {}
            _ => {
                // QueryRoom/JoinRequest/JoinResponse/PairRequest/SyncRequest は first-message のみ
            }
        }
    }

    async fn handle_disconnect(&mut self, endpoint_id: EndpointId) {
        let Some(room) = &mut self.room else { return };
        if let Some(member) = room.members.remove(&endpoint_id) {
            info!(peer = %endpoint_id.fmt_short(), user_id = %member.user_id, "Player left room");

            let left_msg = HostMessage::MemberLeft {
                user_id: member.user_id.clone(),
                user_public_key: member.user_public_key,
            };
            for m in room.members.values_mut() {
                let _ = m.sender.send(&left_msg).await;
            }

            let _ = self.event_tx.try_send(RoomEvent::MemberLeft {
                user_id: member.user_id,
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
                    from: self.host_user_id.clone(),
                    content,
                };
                for member in room.members.values_mut() {
                    let _ = member.sender.send(&chat_msg).await;
                }
            }
            HostCommand::OpenPairingWindow { otp, ttl } => {
                self.pairing_window = Some(PairingWindow {
                    otp: otp.clone(),
                    expires_at: Instant::now() + ttl,
                });
                let _ = self
                    .event_tx
                    .try_send(RoomEvent::PairingWindowOpened { otp });
            }
            HostCommand::ClosePairingWindow => {
                if self.pairing_window.take().is_some() {
                    let _ = self.event_tx.try_send(RoomEvent::PairingWindowClosed);
                }
            }
        }
    }
}
