//! NimbusBill CLI — sync, serve, estimate, search, status.

use crate::export::{ExportFormat, ExportRequest};
use crate::input;
use crate::models::{CloudProvider, SyncConfig};
use crate::paths::{default_db_path, ensure_db_parent};
use crate::{Database, estimate, export, sync};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nimbusbill", about = "NimbusBill — multi-cloud cost estimator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// SQLite database path (default: OS app data dir)
    #[arg(long, global = true, env = "NIMUSBILL_DB")]
    pub db: Option<PathBuf>,

    /// Parallel HTTP workers for sync [default: CPU-based or NIMUSBILL_CONCURRENCY]
    #[arg(long, global = true, env = "NIMUSBILL_CONCURRENCY")]
    pub concurrency: Option<usize>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Refresh catalog + prices from cloud APIs (parallel async)
    Sync {
        #[arg(long, value_delimiter = ',', default_value = "aws,azure,gcp")]
        providers: Vec<String>,
    },
    /// Run the web UI + API server
    Serve {
        #[arg(long, env = "NIMUSBILL_ADDR", default_value = "127.0.0.1:8080")]
        addr: String,
    },
    /// Estimate costs from a YAML/JSON/text workload file
    Estimate {
        file: PathBuf,
        #[arg(long, value_delimiter = ',')]
        providers: Option<Vec<String>>,
        #[arg(long)]
        live: bool,
        #[arg(long)]
        export: Option<ExportKind>,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Search the synced service catalog (FTS)
    Search {
        query: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "25")]
        limit: usize,
    },
    /// Show last sync status per provider
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ExportKind {
    Csv,
    Xlsx,
    Pdf,
    Json,
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Sync { ref providers } => cmd_sync(&cli, providers.clone()).await,
        Commands::Serve { ref addr } => cmd_serve(&cli, addr.clone()).await,
        Commands::Estimate {
            ref file,
            ref providers,
            live,
            export: export_kind,
            ref output,
        } => {
            cmd_estimate(
                &cli,
                file.clone(),
                providers.clone(),
                live,
                export_kind,
                output.clone(),
            )
            .await
        }
        Commands::Search {
            ref query,
            ref provider,
            limit,
        } => cmd_search(&cli, query.clone(), provider.clone(), limit),
        Commands::Status => cmd_status(&cli),
    }
}

fn open_db(cli: &Cli) -> Result<Database> {
    let path = cli.db.clone().unwrap_or_else(default_db_path);
    ensure_db_parent(&path)?;
    Database::open(&path).with_context(|| format!("open database {}", path.display()))
}

fn sync_config(cli: &Cli, providers: Vec<String>) -> Result<SyncConfig> {
    let mut list = Vec::new();
    for p in providers {
        list.push(
            CloudProvider::parse(p.trim())
                .with_context(|| format!("unknown provider: {p}"))?,
        );
    }
    Ok(SyncConfig {
        concurrency: cli.concurrency.unwrap_or(0),
        providers: list,
        ..SyncConfig::default()
    })
}

async fn cmd_sync(cli: &Cli, providers: Vec<String>) -> Result<()> {
    let db = open_db(cli)?;
    let config = sync_config(cli, providers)?;
    sync::run_once_force(&db, &config).await?;
    println!("sync complete");
    Ok(())
}

async fn cmd_serve(cli: &Cli, addr: String) -> Result<()> {
    let db = open_db(cli)?;
    let addr: SocketAddr = addr.parse().context("invalid --addr")?;
    let mut config = SyncConfig::default();
    config.concurrency = cli.concurrency.unwrap_or(0);
    crate::api::serve(addr, db, config, crate::api::resolve_static_dir()).await
}

async fn cmd_estimate(
    cli: &Cli,
    file: PathBuf,
    providers: Option<Vec<String>>,
    live: bool,
    export_kind: Option<ExportKind>,
    output: Option<PathBuf>,
) -> Result<()> {
    let db = open_db(cli)?;
    let requirement = input::parse_file(&file)?;

    let provs: Vec<CloudProvider> = if let Some(list) = providers {
        list.iter()
            .map(|p| CloudProvider::parse(p).context("unknown provider"))
            .collect::<Result<_>>()?
    } else {
        CloudProvider::all().to_vec()
    };

    let request = crate::models::EstimateRequest {
        name: requirement.name.clone(),
        providers: provs,
        live_pricing: live,
        resources: requirement
            .resources
            .iter()
            .map(|r| crate::models::UiResourceInput {
                catalog_id: r
                    .catalog_id
                    .clone()
                    .unwrap_or_else(|| r.service.clone()),
                provider: CloudProvider::Aws,
                region: r.region.clone(),
                sub_region: r.sub_region.clone(),
                quantity: r.quantity,
                sku: r.sku.clone(),
                instance_count: r.instance_count,
                hours: r.hours,
            })
            .collect(),
        token_usage: requirement.token_usage.clone(),
    };

    let ui = estimate::run_ui_async(&db, &request).await?;

    match export_kind {
        None | Some(ExportKind::Json) => {
            let json = serde_json::to_string_pretty(&ui)?;
            write_output(output, json.as_bytes())?;
        }
        Some(kind) => {
            let format = match kind {
                ExportKind::Csv => ExportFormat::Csv,
                ExportKind::Xlsx => ExportFormat::Xlsx,
                ExportKind::Pdf => ExportFormat::Pdf,
                ExportKind::Json => unreachable!(),
            };
            let file = export::export(&ExportRequest {
                format,
                estimate: ui,
            })?;
            let path = output.unwrap_or_else(|| PathBuf::from(&file.filename));
            std::fs::write(&path, &file.bytes)?;
            println!("wrote {}", path.display());
        }
    }

    Ok(())
}

fn cmd_search(cli: &Cli, query: String, provider: Option<String>, limit: usize) -> Result<()> {
    let db = open_db(cli)?;
    let hits = db.search_catalog(&query, provider.as_deref(), limit)?;
    if hits.is_empty() {
        println!("no matches");
        return Ok(());
    }
    for h in hits {
        println!(
            "{} | {} | {} | {} regions | {}",
            h.catalog_id,
            h.provider,
            h.category_label,
            h.regions.len(),
            h.display_name
        );
    }
    Ok(())
}

fn cmd_status(cli: &Cli) -> Result<()> {
    let db = open_db(cli)?;
    let rows = db.latest_sync_status()?;
    if rows.is_empty() {
        println!("no sync history");
        return Ok(());
    }
    for r in rows {
        println!(
            "{}  {}  {}  {}",
            r.provider,
            r.status,
            r.detail.unwrap_or_default(),
            r.synced_at
        );
    }
    Ok(())
}

fn write_output(path: Option<PathBuf>, bytes: &[u8]) -> Result<()> {
    if let Some(p) = path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes)?;
        println!("wrote {}", p.display());
    } else {
        println!("{}", String::from_utf8_lossy(bytes));
    }
    Ok(())
}
