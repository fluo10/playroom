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

    /// プレイヤー名（未指定時は設定ファイルの値を使用）
    #[arg(short, long)]
    name: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Cli::parse();

    // 設定ファイルからデフォルト名を取得
    let config = playroom::AppConfig::load_or_default();
    let name = args.name.unwrap_or(config.name);

    if args.mcp {
        tokio::runtime::Runtime::new()?.block_on(mcp::run(name))?;
    } else if args.cli_mode {
        tokio::runtime::Runtime::new()?.block_on(cli::run(name))?;
    } else {
        gui::run();
    }

    Ok(())
}
