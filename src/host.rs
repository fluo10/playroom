use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::framing::{FramedReceiver, FramedSender};
use crate::protocol::{ClientMessage, HostMessage, MemberInfo};
use crate::{EndpointId, NetworkNode, PeerSession};

/// UI 層に公開するイベント
#[derive(Debug, Clone)]
pub enum RoomEvent {
    /// メンバーが参加した
    MemberJoined { name: String },
    /// メンバーが退出した
    MemberLeft { name: String },
    /// チャットメッセージを受信した
    ChatReceived { from: String, content: String },
    /// ルームが作成された
    RoomCreated { name: String },
    /// ルームが解散された
    RoomClosed,
}

/// ルーム内メンバーの状態（ホスト側が管理）
struct MemberState<T: Serialize> {
    name: String,
    sender: FramedSender<HostMessage<T>>,
}

/// ルームの状態
struct RoomState<T: Serialize> {
    room_name: String,
    members: HashMap<EndpointId, MemberState<T>>,
}

/// accept loop からメインループへのイベント
enum InternalEvent<T> {
    NewConnection {
        endpoint_id: EndpointId,
        sender: FramedSender<HostMessage<T>>,
        first_message: ClientMessage<T>,
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
    /// ホスト自身のチャット
    Chat { content: String },
}

/// ホストを起動する際に返されるハンドル
pub struct RoomHostHandle {
    /// コマンド送信用
    pub commands: mpsc::Sender<HostCommand>,
    /// ホストの EndpointId
    pub endpoint_id: EndpointId,
    /// ホストの EndpointAddr
    pub endpoint_addr: crate::EndpointAddr,
}

/// P2P ルームホスト。1 ノードにつき 1 ルームをホストする。
pub struct RoomHost<T = ()>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + Send + 'static,
{
    host_name: String,
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
    /// `RoomHostHandle` を返すので、UI 層はそれを使ってコマンドを送信する。
    pub async fn start(
        host_name: String,
        event_tx: mpsc::Sender<RoomEvent>,
    ) -> anyhow::Result<(Self, RoomHostHandle)> {
        let node = NetworkNode::bind(None).await?;
        let endpoint_id = node.id();
        let endpoint_addr = node.addr();
        let (internal_tx, internal_rx) = mpsc::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        // accept loop を別タスクで起動
        let accept_tx = internal_tx.clone();
        tokio::spawn(Self::accept_loop(node, accept_tx));

        let host = Self {
            host_name,
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

    /// ルームを作成する。ホストは自動的にメンバーとなる。
    fn create_room(&mut self, name: String) {
        info!(room = %name, "Creating room");
        self.room = Some(RoomState {
            room_name: name.clone(),
            members: HashMap::new(),
        });
        let _ = self.event_tx.try_send(RoomEvent::RoomCreated { name });
    }

    /// ルームを解散する。全メンバーに RoomClosed を送信。
    async fn close_room(&mut self) {
        if let Some(mut room) = self.room.take() {
            info!(room = %room.room_name, "Closing room");
            for (_, member) in room.members.iter_mut() {
                let _ = member.sender.send(&HostMessage::RoomClosed).await;
            }
            let _ = self.event_tx.try_send(RoomEvent::RoomClosed);
        }
    }

    /// accept loop（別タスクで実行）
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

    /// メインループ。内部イベント + コマンドを処理する。
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

    /// 新規接続のハンドリング（最初のメッセージを読んでからイベントを送信）
    async fn handle_new_connection(
        session: PeerSession<HostMessage<T>, ClientMessage<T>>,
        tx: mpsc::Sender<InternalEvent<T>>,
    ) {
        let (sender, mut receiver, endpoint_id) = session.split();

        // 最初のメッセージを読む
        match receiver.recv().await {
            Ok(first_message) => {
                if tx
                    .send(InternalEvent::NewConnection {
                        endpoint_id,
                        sender,
                        first_message,
                    })
                    .await
                    .is_err()
                {
                    return;
                }

                // 以降のメッセージを転送するタスク
                Self::client_recv_loop(endpoint_id, receiver, tx).await;
            }
            Err(e) => {
                warn!(peer = %endpoint_id.fmt_short(), "Failed to read first message: {e}");
            }
        }
    }

    /// クライアントの受信ループ
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
            InternalEvent::NewConnection {
                endpoint_id,
                mut sender,
                first_message,
            } => {
                match first_message {
                    ClientMessage::QueryRoom => {
                        // ルーム情報を返して終了
                        let msg = if let Some(room) = &self.room {
                            let members: Vec<String> =
                                room.members.values().map(|m| m.name.clone()).collect();
                            HostMessage::RoomInfo {
                                name: room.room_name.clone(),
                                members,
                                host_name: self.host_name.clone(),
                            }
                        } else {
                            HostMessage::NotHosting
                        };
                        let _ = sender.send(&msg).await;
                        // 接続は自然に切れる（sender drop）
                    }
                    ClientMessage::Join { name } => {
                        self.handle_join(endpoint_id, name, sender).await;
                    }
                    _ => {
                        let _ = sender
                            .send(&HostMessage::Rejected {
                                reason: "Expected QueryRoom or Join as first message".to_string(),
                            })
                            .await;
                    }
                }
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

    async fn handle_join(
        &mut self,
        endpoint_id: EndpointId,
        name: String,
        mut sender: FramedSender<HostMessage<T>>,
    ) {
        let Some(room) = &mut self.room else {
            let _ = sender
                .send(&HostMessage::Rejected {
                    reason: "No room is hosted".to_string(),
                })
                .await;
            return;
        };

        info!(peer = %endpoint_id.fmt_short(), name = %name, "Player joining room");

        // 既存メンバー一覧を Welcome で送信
        let mut members: Vec<MemberInfo> = room
            .members
            .iter()
            .map(|(eid, m)| MemberInfo {
                name: m.name.clone(),
                endpoint_id: eid.as_bytes().to_vec(),
            })
            .collect();
        // ホスト自身もメンバー一覧に含める
        members.push(MemberInfo {
            name: self.host_name.clone(),
            endpoint_id: self.node_id.as_bytes().to_vec(),
        });

        let _ = sender
            .send(&HostMessage::Welcome {
                room_name: room.room_name.clone(),
                members,
            })
            .await;

        // 既存メンバーに通知
        let new_member_info = MemberInfo {
            name: name.clone(),
            endpoint_id: endpoint_id.as_bytes().to_vec(),
        };
        for member in room.members.values_mut() {
            let _ = member
                .sender
                .send(&HostMessage::MemberJoined(new_member_info.clone()))
                .await;
        }

        // メンバー追加
        room.members.insert(
            endpoint_id,
            MemberState {
                name: name.clone(),
                sender,
            },
        );

        let _ = self
            .event_tx
            .try_send(RoomEvent::MemberJoined { name });
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

                // 全メンバーに broadcast（送信者以外）
                let chat_msg = HostMessage::Chat {
                    from: from.clone(),
                    content: content.clone(),
                };
                for (eid, member) in room.members.iter_mut() {
                    if *eid != endpoint_id {
                        let _ = member.sender.send(&chat_msg).await;
                    }
                }

                // ホスト（UI）にも通知
                let _ = self
                    .event_tx
                    .try_send(RoomEvent::ChatReceived { from, content });
            }
            ClientMessage::Leave => {
                self.handle_disconnect(endpoint_id).await;
            }
            ClientMessage::InRoom(_) => {
                // 将来のゲーム別処理用。今回は無視。
            }
            _ => {
                // QueryRoom, Join は最初のメッセージでのみ有効
            }
        }
    }

    async fn handle_disconnect(&mut self, endpoint_id: EndpointId) {
        let Some(room) = &mut self.room else { return };
        if let Some(member) = room.members.remove(&endpoint_id) {
            info!(peer = %endpoint_id.fmt_short(), name = %member.name, "Player left room");

            // 他メンバーに通知
            let left_msg = HostMessage::MemberLeft {
                name: member.name.clone(),
            };
            for m in room.members.values_mut() {
                let _ = m.sender.send(&left_msg).await;
            }

            let _ = self
                .event_tx
                .try_send(RoomEvent::MemberLeft { name: member.name });
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
