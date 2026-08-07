mod aws;
mod azure;
mod catalog;
mod gcp;
mod gcp_billing;
mod parallel;
mod pricing_common;
mod llm_catalog;
mod tokens;
pub use catalog::aws::list_offer_skus;
pub use catalog::azure::list_retail_skus;
pub use catalog::gcp::list_service_skus;
pub use catalog::specs::infer_unit;
pub use catalog::specs::{aws_attr_key, aws_default_sku, is_llm_token_service};
pub use llm_catalog::seed_baseline;
use crate::db::Database;
use crate::models::{CloudProvider, SyncConfig, TokenUsageSpec};
use anyhow::Result;
use chrono::Utc;
use std::time::Duration;
use tokio::time;

/// Skip catalog re-scan when `catalog_meta.last_updated` is newer than this.
pub const CATALOG_STALE_SECS: u64 = 6 * 3600;

pub async fn run_once(db: &Database, config: &SyncConfig) -> Result<()> {
    run_sync(db, config, false).await
}

pub async fn run_once_force(db: &Database, config: &SyncConfig) -> Result<()> {
    run_sync(db, config, true).await
}

async fn run_sync(db: &Database, config: &SyncConfig, force: bool) -> Result<()> {
    let workers = parallel::concurrency_limit(config.concurrency);

    if force || catalog_needs_sync(db)? {
        match catalog::sync_all(db, workers).await {
            Ok((aws_n, azure_n, gcp_n)) => {
                let total = aws_n + azure_n + gcp_n;
                let cached = db.provider_catalog_count().unwrap_or(0);
                if total > 0 {
                    let now = Utc::now();
                    db.set_catalog_last_updated(now)?;
                    let mut parts = Vec::new();
                    for (label, provider) in [
                        ("aws", CloudProvider::Aws),
                        ("azure", CloudProvider::Azure),
                        ("gcp", CloudProvider::Gcp),
                    ] {
                        if let Ok((services, regions, pairs)) =
                            db.catalog_coverage(provider.as_str())
                        {
                            parts.push(format!(
                                "{label}={services}svc/{regions}reg/{pairs}pairs"
                            ));
                        }
                    }
                    db.record_sync(
                        "catalog",
                        "ok",
                        Some(&format!(
                            "aws={aws_n} azure={azure_n} gcp={gcp_n} entries · {} · last_updated={}",
                            parts.join(" "),
                            now.to_rfc3339()
                        )),
                    )?;
                } else if cached > 0 {
                    db.record_sync(
                        "catalog",
                        "ok",
                        Some(&format!(
                            "refresh failed (network/proxy); using cached catalog ({cached} services)"
                        )),
                    )?;
                    tracing::warn!(
                        cached,
                        "catalog refresh produced no updates; using cached catalog"
                    );
                } else {
                    db.record_sync(
                        "catalog",
                        "error",
                        Some("no catalog data and cloud APIs unreachable"),
                    )?;
                }
            }
            Err(e) => {
                db.record_sync("catalog", "error", Some(&e.to_string()))?;
                tracing::error!(error = %e, "catalog sync failed");
            }
        }
    } else if let Ok(Some(last_updated)) = db.catalog_last_updated() {
        tracing::info!(
            last_updated = %last_updated,
            stale_secs = CATALOG_STALE_SECS,
            "catalog sync skipped (fresh)"
        );
    } else {
        tracing::info!(stale_secs = CATALOG_STALE_SECS, "catalog sync skipped (fresh)");
    }

    // ponytail: prices are on-demand per estimate line (live calculate or cache miss), not bulk-synced here
    for &provider in &config.providers {
        db.record_sync(
            provider.as_str(),
            "ok",
            Some("metadata only — prices fetched per service/region on calculate"),
        )?;
    }

    if force || llm_catalog_needs_sync(db)? {
        if force {
            tracing::info!("sync catalog: refreshing LLM models");
        } else {
            tracing::info!("llm catalog stale or empty; refreshing");
        }
        match tokens::sync_all(db).await {
            Ok(n) => {
                db.record_sync("llm", "ok", Some(&format!("{n} LLM models (catalog)")))?;
                tracing::info!(models = n, "llm catalog sync complete");
            }
            Err(e) => {
                db.record_sync("llm", "error", Some(&e.to_string()))?;
                tracing::error!(error = %e, "llm catalog sync failed");
            }
        }
    } else {
        tracing::debug!("llm catalog fresh; background sync skipped");
    }
    Ok(())
}

pub async fn run_daemon(db: Database, config: SyncConfig) {
    let mut interval = time::interval(Duration::from_secs(config.interval_secs));
    interval.tick().await; // ponytail: skip immediate duplicate sync after startup run_once
    loop {
        interval.tick().await;
        if let Err(e) = run_once(&db, &config).await {
            tracing::error!(error = %e, "sync cycle failed");
        }
    }
}

fn catalog_needs_sync(db: &Database) -> Result<bool> {
    if db.provider_catalog_count()? == 0 {
        return Ok(true);
    }
    match db.catalog_age_secs()? {
        None => Ok(true),
        Some(age) => Ok(age >= CATALOG_STALE_SECS as i64),
    }
}

fn llm_catalog_needs_sync(db: &Database) -> Result<bool> {
    if db.llm_model_count()? == 0 {
        return Ok(true);
    }
    match db.sync_provider_age_secs("llm")? {
        None => Ok(true),
        Some(age) => Ok(age >= CATALOG_STALE_SECS as i64),
    }
}

/// Fetch a single price live, upsert to cache, return price string.
pub async fn fetch_live_price(
    db: &Database,
    provider: CloudProvider,
    service: &str,
    sku: &str,
    region: &str,
) -> Result<String> {
    let (price, resolved_sku) = match provider {
        CloudProvider::Aws => aws::fetch_live(&db, service, sku, region).await?,
        CloudProvider::Azure => {
            let price = azure::fetch_live(&db, service, sku, region).await?;
            (price, sku.to_string())
        }
        CloudProvider::Gcp => {
            let price = gcp::fetch_live(&db, service, sku, region).await?;
            (price, sku.to_string())
        }
    };
    db.upsert_price(
        provider.as_str(),
        service,
        &resolved_sku,
        region,
        "unit",
        &price,
        "USD",
        Utc::now(),
    )?;
    Ok(price)
}

pub async fn refresh_token_prices(db: &Database, usage: &[TokenUsageSpec]) -> Result<()> {
    if usage.is_empty() {
        return Ok(());
    }
    llm_catalog::sync_selected(db, usage).await?;
    Ok(())
}

/// Seed baseline LLM rates when cloud APIs are unreachable.
pub fn seed_token_prices(db: &Database) -> Result<()> {
    match seed_baseline(db) {
        Ok(n) if n > 0 => tracing::info!(models = n, "llm baseline rates loaded"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "llm baseline seed failed"),
    }
    Ok(())
}
