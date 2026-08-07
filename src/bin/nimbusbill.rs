//! NimbusBill CLI entry point (sync, serve, estimate, search, status).

use anyhow::Result;
use clap::Parser;
use nimbusbill::cli::{self, Cli};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("nimbusbill=info".parse()?))
        .init();

    let cli = Cli::parse();
    cli::run(cli).await
}
