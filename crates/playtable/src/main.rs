use anyhow::Result;
use clap::Parser;

mod cli;
mod gui;
mod mcp;

#[derive(Parser)]
#[command(name = "playtable", about = "Multiplayer tabletop games for humans and AI")]
struct Cli {
    /// MCP サーバーモードで起動（AI エージェント向け）
    #[arg(long)]
    mcp: bool,

    /// CLI（readline 式）モードで起動
    #[arg(long, name = "cli")]
    cli_mode: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Cli::parse();

    if args.mcp {
        tokio::runtime::Runtime::new()?.block_on(mcp::run())?;
    } else if args.cli_mode {
        tokio::runtime::Runtime::new()?.block_on(cli::run())?;
    } else {
        gui::run();
    }

    Ok(())
}
