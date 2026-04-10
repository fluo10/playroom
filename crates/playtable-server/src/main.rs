use anyhow::Result;
use clap::Parser;
use playtable_core::GameServer;
use tracing::info;

#[derive(Parser)]
#[command(name = "playtable-server", about = "Playtable headless game server")]
struct Cli {
    /// バインドするポート（省略時は OS が割り当て）
    #[arg(short, long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let _cli = Cli::parse();

    let server = GameServer::start().await?;

    info!("Server started.");
    info!("Share this address with other players:");
    let addr_json = serde_json::to_string(&server.addr())?;
    println!("{addr_json}");

    server.run().await?;

    Ok(())
}
