use anyhow::Result;
use clap::Parser;

mod gui;
mod mcp;

#[derive(Parser)]
#[command(name = "playtable", about = "Multiplayer tabletop games for humans and AI")]
struct Cli {
    /// MCP サーバーモードで起動（AI エージェント向け）
    #[arg(long)]
    mcp: bool,

    /// 接続先サーバーの EndpointAddr（JSON 形式）
    #[arg(long)]
    server: Option<String>,

    /// プレイヤー名
    #[arg(short, long, default_value = "Player")]
    name: String,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if cli.mcp {
        let server = cli
            .server
            .expect("--server is required in --mcp mode");
        tokio::runtime::Runtime::new()?.block_on(mcp::run(server, cli.name))?;
    } else {
        gui::run();
    }

    Ok(())
}
