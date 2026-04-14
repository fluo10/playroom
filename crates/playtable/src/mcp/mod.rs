use std::sync::Arc;

use anyhow::Result;
use playroom::{AppConfig, RoomEvent};
use rmcp::transport;
use rmcp::ServiceExt;
use tokio::sync::{mpsc, Mutex};

mod handler;

pub async fn run() -> Result<()> {
    let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);

    let mcp_handler = Arc::new(handler::PlaytableMcpHandler::new(
        config.clone(),
        room_event_tx,
    ));

    // RoomHost からのイベント（起動前は受信しない）を MCP の notification に変換
    let handler_for_events = mcp_handler.clone();
    tokio::spawn(async move {
        while let Some(event) = room_event_rx.recv().await {
            handler_for_events.handle_room_event(event).await;
        }
    });

    let service = mcp_handler
        .serve(transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;

    service.waiting().await?;

    Ok(())
}
