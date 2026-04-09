use bevy::prelude::*;
use playtable_core::{ClientMessage, ServerMessage};
use std::sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}};
use tokio::runtime::Runtime;

/// Bevy → ネットワークスレッドへの送信チャネル
#[derive(Resource)]
pub struct NetSender(pub Sender<ClientMessage>);

/// ネットワークスレッド → Bevy へのメッセージキュー
///
/// `Receiver` は Sync でないため Mutex で包む
#[derive(Resource)]
pub struct IncomingMessages(pub Arc<Mutex<Receiver<ServerMessage>>>);

/// tokio ランタイムのハンドル
#[derive(Resource)]
pub struct TokioRuntime(pub Runtime);

/// Bevy の Message として流す受信イベント
#[derive(Debug, Clone, Message)]
pub struct ServerMessageReceived(pub ServerMessage);

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        let rt = Runtime::new().expect("failed to create tokio runtime");
        let (to_net_tx, _to_net_rx) = mpsc::channel::<ClientMessage>();
        let (_from_net_tx, from_net_rx) = mpsc::channel::<ServerMessage>();

        app.insert_resource(TokioRuntime(rt))
            .insert_resource(NetSender(to_net_tx))
            .insert_resource(IncomingMessages(Arc::new(Mutex::new(from_net_rx))))
            .add_message::<ServerMessageReceived>()
            .add_systems(Update, poll_network_messages);
    }
}

fn poll_network_messages(
    incoming: Res<IncomingMessages>,
    mut writer: MessageWriter<ServerMessageReceived>,
) {
    if let Ok(rx) = incoming.0.lock() {
        while let Ok(msg) = rx.try_recv() {
            writer.write(ServerMessageReceived(msg));
        }
    }
}
