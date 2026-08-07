pub(crate) mod aws;
pub(crate) mod azure;
pub(crate) mod gcp;
pub mod specs;

#[cfg(test)]
mod live_tests;

use crate::db::Database;
use anyhow::Result;

pub async fn sync_all(db: &Database, concurrency: usize) -> Result<(usize, usize, usize)> {
    db.ensure_bootstrap()?;
    let (aws_r, azure_r, gcp_r) = tokio::join!(
        aws::sync(db, concurrency),
        azure::sync(db),
        gcp::sync(db, concurrency),
    );
    let aws_n = aws_r.unwrap_or_else(|e| {
        tracing::error!(provider = "aws", error = %e, "catalog sync failed");
        0
    });
    let azure_n = azure_r.unwrap_or_else(|e| {
        tracing::error!(provider = "azure", error = %e, "catalog sync failed");
        0
    });
    let gcp_n = gcp_r.unwrap_or_else(|e| {
        tracing::error!(provider = "gcp", error = %e, "catalog sync failed");
        0
    });
    if aws_n + azure_n + gcp_n == 0 {
        let cached = db.provider_catalog_count().unwrap_or(0);
        if cached > 0 {
            tracing::warn!(
                cached,
                "catalog APIs unreachable; keeping existing cached catalog"
            );
            return Ok((0, 0, 0));
        }
        anyhow::bail!("catalog sync produced no entries for any provider");
    }
    tracing::info!(aws_n, azure_n, gcp_n, concurrency, "catalog sync complete");
    Ok((aws_n, azure_n, gcp_n))
}
