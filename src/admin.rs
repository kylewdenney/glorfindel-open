use std::path::PathBuf;

use clap::Parser;
use glorfindel_admin::server;

#[derive(Parser)]
#[command(name = "glorfindel-admin")]
#[command(about = "Web admin dashboard for Glorfindel — define, spawn, and test agents")]
struct Cli {
    /// Address to bind the HTTP server
    #[arg(long, env = "ADMIN_ADDR", default_value = "0.0.0.0:3000")]
    addr: String,

    /// Directory to persist agent definitions (survives restarts)
    #[arg(long, env = "ADMIN_DATA_DIR", default_value = "./glorfindel-data")]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    server::run(&cli.addr, cli.data_dir).await
}
