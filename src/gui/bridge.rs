//! Tokio 非同期 AppService と Bevy の同期 system の橋渡し。
//!
//! - AppService のメソッド呼び出し → `rt.spawn` による fire-and-forget
//! - 結果（成功メッセージ / エラー） → std mpsc で Bevy 側に送り、ServiceStatus に反映
//! - AppEvent → std mpsc で Bevy 側に送り、`AppEventReceived` メッセージに変換

use std::sync::{mpsc as std_mpsc, Arc, Mutex as StdMutex};

use bevy::prelude::*;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use super::state::{AppScreen, ServiceStatus};
use crate::service::{AppEvent, AppService};
use crate::{HostMessage, RoomEvent};

/// Bevy 側から AppService を呼ぶためのハンドル。
#[derive(Resource, Clone)]
pub struct ServiceHandle {
    pub service: Arc<AppService>,
    pub runtime: Arc<Runtime>,
    pub result_tx: std_mpsc::Sender<ServiceOutcome>,
    result_rx: Arc<StdMutex<std_mpsc::Receiver<ServiceOutcome>>>,
}

impl ServiceHandle {
    pub fn new(runtime: Arc<Runtime>, service: Arc<AppService>) -> Self {
        let (tx, rx) = std_mpsc::channel();
        Self {
            service,
            runtime,
            result_tx: tx,
            result_rx: Arc::new(StdMutex::new(rx)),
        }
    }

    /// 非同期アクションを spawn し、結果（文字列）を result チャネルに流す。
    pub fn dispatch<F>(&self, label: &'static str, fut: F)
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let tx = self.result_tx.clone();
        self.runtime.spawn(async move {
            let outcome = match fut.await {
                Ok(msg) => ServiceOutcome {
                    label,
                    ok: true,
                    message: msg,
                },
                Err(e) => ServiceOutcome {
                    label,
                    ok: false,
                    message: e,
                },
            };
            let _ = tx.send(outcome);
        });
    }
}

/// AppService の非同期呼び出しの結果（成功/失敗）を Bevy 側に運ぶペイロード。
#[derive(Debug, Clone)]
pub struct ServiceOutcome {
    pub label: &'static str,
    pub ok: bool,
    pub message: String,
}

/// AppEvent を tokio → std mpsc に流す中継。Bevy 側は `IncomingAppEvents` から polling。
#[derive(Resource)]
pub struct IncomingAppEvents(pub Arc<StdMutex<std_mpsc::Receiver<AppEvent>>>);

pub fn spawn_event_forwarder(
    handle: &ServiceHandle,
    mut event_rx: mpsc::Receiver<AppEvent>,
) -> IncomingAppEvents {
    let (std_tx, std_rx) = std_mpsc::channel::<AppEvent>();
    handle.runtime.spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if std_tx.send(ev).is_err() {
                break;
            }
        }
    });
    IncomingAppEvents(Arc::new(StdMutex::new(std_rx)))
}

/// AppEvent を毎フレーム poll し、StateChanged / Room / Guest メッセージを捌く。
pub fn poll_app_events(
    incoming: Res<IncomingAppEvents>,
    mut next: ResMut<NextState<AppScreen>>,
    mut status: ResMut<ServiceStatus>,
) {
    let Ok(rx) = incoming.0.lock() else { return };
    while let Ok(event) = rx.try_recv() {
        match event {
            AppEvent::StateChanged(state) => {
                next.set(AppScreen::from(state));
            }
            AppEvent::Room(ev) => {
                if let Some(text) = format_room_event(&ev) {
                    status.text = text;
                }
            }
            AppEvent::Guest(msg) => {
                if let Some(text) = format_host_message(&msg) {
                    status.text = text;
                }
            }
        }
    }
}

pub fn poll_service_results(handle: Res<ServiceHandle>, mut status: ResMut<ServiceStatus>) {
    let Ok(rx) = handle.result_rx.lock() else { return };
    while let Ok(outcome) = rx.try_recv() {
        let prefix = if outcome.ok { "[ok]" } else { "[err]" };
        status.text = format!("{prefix} {}: {}", outcome.label, outcome.message);
    }
}

fn format_room_event(event: &RoomEvent) -> Option<String> {
    Some(match event {
        RoomEvent::MemberJoined { user_id, .. } => format!("* {user_id} joined"),
        RoomEvent::MemberLeft { user_id, .. } => format!("* {user_id} left"),
        RoomEvent::ChatReceived { from, content } => format!("[chat] {from}: {content}"),
        RoomEvent::RoomCreated { name } => format!("* Room '{name}' created"),
        RoomEvent::RoomClosed => "* Room closed".to_string(),
        RoomEvent::PairingWindowOpened => "* Pairing window opened".to_string(),
        RoomEvent::PairingWindowClosed => "* Pairing window closed".to_string(),
        RoomEvent::PairingCompleted { device_label, .. } => {
            format!("* Paired with new device: {device_label}")
        }
        RoomEvent::SyncCompleted { .. } => "* Sync completed".to_string(),
    })
}

fn format_host_message(msg: &HostMessage) -> Option<String> {
    Some(match msg {
        HostMessage::Chat { from, content } => format!("[chat] {from}: {content}"),
        HostMessage::MemberJoined(info) => format!("* {} joined", info.user_id),
        HostMessage::MemberLeft { user_id, .. } => format!("* {user_id} left"),
        HostMessage::RoomClosed => "* Host closed the room".to_string(),
        HostMessage::Rejected { reason } => format!("Rejected: {reason}"),
        _ => return None,
    })
}
