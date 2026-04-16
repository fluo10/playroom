use std::sync::Arc;

use anyhow::Result;
use playroom::AppConfig;
use rmcp::transport;
use rmcp::ServiceExt;
use tokio::sync::Mutex;

use crate::service::AppService;

mod handler;

pub async fn run() -> Result<()> {
    let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
    let (service, event_rx) = AppService::new(config)
        .await
        .map_err(|e| anyhow::anyhow!("service init: {e}"))?;

    let mcp_handler = Arc::new(handler::PlaytableMcpHandler::new(service, event_rx));
    mcp_handler.spawn_forwarder().await;

    let service = mcp_handler
        .serve(transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;

    service.waiting().await?;

    Ok(())
}
