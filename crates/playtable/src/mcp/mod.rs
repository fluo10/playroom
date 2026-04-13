use std::sync::Arc;

use anyhow::Result;
use playroom::{AppConfig, RoomEvent, RoomHost};
use rmcp::transport;
use rmcp::ServiceExt;
use tokio::sync::mpsc;

mod handler;

pub async fn run(name: String) -> Result<()> {
    let config = AppConfig::load_or_default();

    // RoomHost を起動
    let (room_event_tx, mut room_event_rx) = mpsc::channel::<RoomEvent>(64);
    let (host, host_handle) = RoomHost::<()>::start(name.clone(), room_event_tx).await?;

    // ホストをバックグラウンドで実行
    tokio::spawn(async move {
        if let Err(e) = host.run().await {
            tracing::error!("Host error: {e}");
        }
    });

    // MCP ハンドラーを作成
    let mcp_handler = Arc::new(handler::PlaytableMcpHandler::new(
        name,
        config,
        host_handle,
    ));

    // バックグラウンドで RoomEvent を MCP notification に変換
    let handler_for_events = mcp_handler.clone();
    tokio::spawn(async move {
        while let Some(event) = room_event_rx.recv().await {
            handler_for_events.handle_room_event(event).await;
        }
    });

    // MCP サーバーを stdio で起動
    let service = mcp_handler
        .serve(transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;

    service.waiting().await?;

    Ok(())
}
