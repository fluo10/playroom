use std::sync::Arc;

use anyhow::Result;
use playroom::{AppConfig, RoomEvent, RoomHost, UserIdentity};
use rmcp::transport;
use rmcp::ServiceExt;
use tokio::sync::{mpsc, Mutex};

mod handler;

pub async fn run(name: String) -> Result<()> {
    let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
    let identity = Arc::new(UserIdentity::load_or_generate()?);

    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);
    let (host, host_handle) = RoomHost::<()>::start(
        name.clone(),
        identity.clone(),
        config.clone(),
        room_event_tx,
    )
    .await?;

    tokio::spawn(async move {
        if let Err(e) = host.run().await {
            tracing::error!("Host error: {e}");
        }
    });

    let mcp_handler = Arc::new(handler::PlaytableMcpHandler::new(
        name,
        config,
        identity,
        host_handle,
    ));

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
