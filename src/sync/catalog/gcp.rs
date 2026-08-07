use super::specs;
use crate::db::ProviderServiceIngest;
use crate::db::Database;
use crate::sync::gcp_billing;
use crate::sync::parallel;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub async fn sync(db: &Database, concurrency: usize) -> Result<usize> {
    let services = match gcp_billing::list_services().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "gcp catalog sync skipped — set GCP_PRICING_API_KEY");
            return Ok(0);
        }
    };
    let now = Utc::now();
    let count = Arc::new(AtomicUsize::new(0));
    let tally = Arc::clone(&count);
    let db = db.clone();

    parallel::for_each(services, concurrency, move |service| {
        let db = db.clone();
        let count = Arc::clone(&tally);
        async move { ingest_service(&db, &service, now, &count).await }
    })
    .await?;

    Ok(count.load(Ordering::Relaxed))
}

async fn ingest_service(
    db: &Database,
    service: &Value,
    now: DateTime<Utc>,
    count: &AtomicUsize,
) -> Result<()> {
    let billing_id = service
        .get("serviceId")
        .and_then(|v| v.as_str())
        .context("gcp service missing serviceId")?;
    let display_name = service
        .get("displayName")
        .and_then(|v| v.as_str())
        .unwrap_or(billing_id)
        .to_string();
    let category_id = specs::infer_category(&display_name);
    let unit = specs::infer_unit(&display_name);
    let service_key = slugify(&display_name);
    let catalog_id = format!("gcp:{service_key}");
    let regions = match gcp_billing::list_skus(billing_id).await {
        Ok(skus) => {
            let raw = gcp_billing::collect_service_regions(&skus);
            crate::sync::pricing_common::filter_gcp_service_regions(raw)
        }
        Err(e) => {
            tracing::debug!(
                service = %display_name,
                error = %e,
                "gcp sku region fetch failed; keeping cached regions if any"
            );
            db.catalog_regions(&catalog_id).unwrap_or_default()
        }
    };
    let default_sku = "default".into();
    let hint = display_name.clone();

    let row = ProviderServiceIngest {
        catalog_id,
        provider: "gcp".into(),
        service_key,
        display_name,
        category_id: category_id.into(),
        unit: unit.into(),
        default_sku,
        offer_code: None,
        attr_key: None,
        attr_value: None,
        billing_service_id: Some(billing_id.to_string()),
        sku_description_hint: Some(hint),
        regions,
    };
    db.upsert_provider_service(&row, now)?;
    count.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// List GCP SKUs for a billing service in a region (`value` = sku id, `label` = description).
pub async fn list_service_skus(
    billing_service_id: &str,
    region: &str,
) -> Result<Vec<(String, String)>> {
    let skus = gcp_billing::list_skus(billing_service_id).await?;
    let mut out = Vec::new();
    for sku in skus {
        let regions = gcp_billing::sku_regions(&sku);
        if !regions.is_empty() && !regions.iter().any(|r| r == region) {
            continue;
        }
        let id = sku
            .get("skuId")
            .and_then(|v| v.as_str())
            .or_else(|| sku.get("name").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let label = sku
            .get("description")
            .and_then(|d| d.as_str())
            .filter(|d| !d.is_empty())
            .unwrap_or(&id)
            .to_string();
        out.push((id, label));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
