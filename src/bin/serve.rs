//! Standalone Axum web server — open UI in a browser at http://127.0.0.1:8080
//!
//! Prefer: `nimbusbill serve` (same behavior).

use anyhow::Result;
use nimbusbill::api::{self, resolve_static_dir};
use nimbusbill::paths::{default_db_path, ensure_db_parent};
use nimbusbill::{Database, SyncConfig};
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("nimbusbill=info".parse()?))
        .init();

    let addr: SocketAddr = std::env::var("NIMUSBILL_ADDR")
        .or_else(|_| std::env::var("COSR_ADDR"))
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;

    let db_path = default_db_path();
    ensure_db_parent(&db_path)?;
    let db = Database::open(&db_path)?;

    tracing::info!(db = %db_path.display(), "starting web server on http://{addr}");
    api::serve(addr, db, SyncConfig::default(), resolve_static_dir()).await
}
