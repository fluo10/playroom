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

    /// ユーザー ID（英数字 1-32 文字）。未指定時は設定ファイルの値を使用。
    #[arg(short, long = "user-id")]
    user_id: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Cli::parse();

    let config = playroom::AppConfig::load_or_default();
    let user_id = args.user_id.unwrap_or(config.user_id);

    if !playroom::identity::is_valid_user_id(&user_id) {
        anyhow::bail!(
            "Invalid user_id: must be 1-32 ASCII alphanumeric characters. Got: {:?}",
            user_id
        );
    }

    if args.mcp {
        tokio::runtime::Runtime::new()?.block_on(mcp::run(user_id))?;
    } else if args.cli_mode {
        tokio::runtime::Runtime::new()?.block_on(cli::run(user_id))?;
    } else {
        gui::run();
    }

    Ok(())
}
