use anyhow::Result;

mod bridge;
mod tools;

pub async fn run(server_addr: String, name: String) -> Result<()> {
    tracing::info!(server = %server_addr, name = %name, "Starting MCP mode");

    let _bridge = bridge::PlaytableBridge::connect(&server_addr, &name).await?;

    // TODO: rmcp MCP サーバーを起動して stdio で AI クライアントと通信

    Ok(())
}
