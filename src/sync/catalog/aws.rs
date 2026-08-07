use super::specs;
use crate::db::ProviderServiceIngest;
use crate::db::Database;
use crate::db::PriceTarget;
use crate::sync::parallel;
use crate::sync::pricing_common::{
    aws_first_on_demand, aws_on_demand_price, fetch_json_retry, fetch_json_retry_live,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const AWS_PRICING_HOST: &str = "https://pricing.us-east-1.amazonaws.com";

pub async fn sync(db: &Database, concurrency: usize) -> Result<usize> {
    let now = Utc::now();
    let index_url = format!("{AWS_PRICING_HOST}/offers/v1.0/aws/index.json");
    let index = fetch_json_retry(&index_url, 3).await?;
    let offers = index
        .get("offers")
        .and_then(|o| o.as_object())
        .context("missing AWS offers index")?;

    let items: Vec<(String, Value)> = offers
        .iter()
        .map(|(code, offer)| (code.clone(), offer.clone()))
        .collect();

    let fallback_regions = Arc::new(load_fallback_regions(&db, &items).await?);

    let count = Arc::new(AtomicUsize::new(0));
    let tally = Arc::clone(&count);
    let db = db.clone();

    parallel::for_each(items, concurrency, move |(offer_code, offer)| {
        let db = db.clone();
        let count = Arc::clone(&tally);
        let fallback_regions = Arc::clone(&fallback_regions);
        async move {
            ingest_offer(&db, &offer_code, &offer, now, &count, &fallback_regions).await
        }
    })
    .await?;

    Ok(count.load(Ordering::Relaxed))
}

async fn load_fallback_regions(db: &Database, items: &[(String, Value)]) -> Result<Vec<String>> {
    if let Some((_, offer)) = items.iter().find(|(code, _)| code == "AmazonEC2") {
        if let Ok(regions) = fetch_regions(offer).await {
            let regions = crate::sync::pricing_common::filter_standard_aws_regions(regions);
            if !regions.is_empty() {
                tracing::info!(regions = regions.len(), "aws catalog using AmazonEC2 region index");
                return Ok(regions);
            }
        }
    }
    let cached = db.provider_region_union("aws")?;
    if !cached.is_empty() {
        tracing::warn!(
            count = cached.len(),
            "aws catalog using region union already stored in database"
        );
        return Ok(cached);
    }
    anyhow::bail!("no AWS regions from pricing API or database cache")
}

async fn ingest_offer(
    db: &Database,
    offer_code: &str,
    _offer: &Value,
    now: DateTime<Utc>,
    count: &AtomicUsize,
    fallback_regions: &[String],
) -> Result<()> {
    if specs::is_llm_token_service(offer_code) {
        return Ok(());
    }
    let display_name = specs::humanize_service_key(offer_code);
    let category_id = specs::infer_category(&display_name);
    let unit = specs::infer_unit(&display_name);
    // ponytail: one shared region list + offer_code placeholder SKU; live /api/catalog/skus fills configs
    let regions = fallback_regions.to_vec();
    let attr_key = specs::aws_attr_key(offer_code).map(str::to_string);
    let default_sku = specs::aws_default_sku(offer_code);
    let attr_value = None;

    let row = ProviderServiceIngest {
        catalog_id: format!("aws:{offer_code}"),
        provider: "aws".into(),
        service_key: offer_code.to_string(),
        display_name,
        category_id: category_id.into(),
        unit: unit.into(),
        default_sku,
        offer_code: Some(offer_code.to_string()),
        attr_key,
        attr_value,
        billing_service_id: None,
        sku_description_hint: None,
        regions,
    };
    db.upsert_provider_service(&row, now)?;
    count.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

async fn fetch_regions(offer: &Value) -> Result<Vec<String>> {
    let path = offer
        .get("currentRegionIndexUrl")
        .and_then(|u| u.as_str())
        .context("missing currentRegionIndexUrl")?;
    let url = format!("{AWS_PRICING_HOST}{path}");
    let json = fetch_json_retry(&url, 2).await?;
    let mut regions: Vec<String> = json
        .get("regions")
        .and_then(|r| r.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    regions.sort();
    if regions.is_empty() {
        anyhow::bail!("no regions in AWS region index");
    }
    Ok(regions)
}

async fn fetch_offer_index(offer_code: &str, region: &str, live: bool) -> Result<Value> {
    let url = format!(
        "{AWS_PRICING_HOST}/offers/v1.0/aws/{offer_code}/current/{region}/index.json"
    );
    if live {
        fetch_json_retry_live(&url).await
    } else {
        fetch_json_retry(&url, 2).await
    }
}

/// Used by live price fetch when attr pair known.
pub async fn fetch_price_for_offer(
    offer_code: &str,
    region: &str,
    attr_key: Option<&str>,
    attr_value: Option<&str>,
    live: bool,
) -> Result<String> {
    let index = fetch_offer_index(offer_code, region, live).await?;
    if let (Some(k), Some(v)) = (attr_key, attr_value) {
        if let Some(p) = aws_on_demand_price(&index, k, v)? {
            return Ok(p);
        }
    }
    aws_first_on_demand(&index)
        .map(|(_, _, p)| p)
        .context("no on-demand price in AWS offer index")
}

/// Resolve a unit price and the SKU label to cache for a sync target.
pub async fn fetch_price_for_target(target: &PriceTarget, live: bool) -> Result<(String, String)> {
    let offer = target
        .offer_code
        .as_deref()
        .unwrap_or(&target.service_key);

    // Bedrock token pricing is published via us-east-1 pricebook only — not per-region infra SKUs.
    if specs::is_llm_token_service(offer) {
        anyhow::bail!(
            "{offer} is token-priced — add models under LLM / Token Usage, not Infrastructure Services"
        );
    }

    if let Some(attr_key) = target.attr_key.as_deref().or_else(|| {
        specs::aws_attr_key(
            target
                .offer_code
                .as_deref()
                .unwrap_or(&target.service_key),
        )
    }) {
        if let Ok(price) = fetch_price_for_offer(
            offer,
            &target.region,
            Some(attr_key),
            Some(&target.sku),
            live,
        )
        .await
        {
            return Ok((price, target.sku.clone()));
        }
    }

    let index = fetch_offer_index(offer, &target.region, live).await?;
    if let Some((_, attr_value, price)) = aws_first_on_demand(&index) {
        return Ok((price, attr_value));
    }
    anyhow::bail!("no on-demand price for {} in {}", offer, target.region)
}

/// List configurable SKUs (e.g. instance types) from an AWS regional offer index.
pub async fn list_offer_skus(
    offer_code: &str,
    region: &str,
    attr_key: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "{AWS_PRICING_HOST}/offers/v1.0/aws/{offer_code}/current/{region}/index.json"
    );
    let index = fetch_json_retry(&url, 2).await?;
    Ok(crate::sync::pricing_common::aws_list_attr_values(&index, attr_key))
}
