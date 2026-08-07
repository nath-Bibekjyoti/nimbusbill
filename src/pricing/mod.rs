mod aws;
mod azure;
mod gcp;
mod tokens;

use crate::db::Database;
use crate::models::{CloudProvider, CostRow, LineItem, ResourceSpec, TokenUsageSpec};
use crate::sync;
use anyhow::{Context, Result};
use rust_decimal::Decimal;

pub use tokens::estimate_token_cost;

pub fn estimate_resource_from_cache(
    db: &Database,
    provider: CloudProvider,
    spec: &ResourceSpec,
    provider_key: &str,
) -> Result<LineItem> {
    let sku = spec.sku.as_deref().unwrap_or(&spec.service);
    let unit_price = match db.lookup_price(provider_key, &spec.service, sku, &spec.region)? {
        Some((price, _currency)) => price.parse::<Decimal>().context("parse cached price")?,
        None => Decimal::ZERO,
    };

    let monthly = unit_price * spec.quantity;
    let category = spec
        .category
        .clone()
        .unwrap_or_else(|| spec.service.clone());
    let display = spec
        .display_name
        .clone()
        .unwrap_or_else(|| sku.to_string());
    let region = spec
        .region_label
        .as_deref()
        .unwrap_or(&spec.region);
    let qty_note = quantity_note(spec);

    Ok(LineItem {
        provider,
        category,
        description: format!("{display} — {sku}{qty_note} in {region}"),
        unit_price,
        quantity: spec.quantity,
        unit: spec.unit.clone(),
        monthly_cost: monthly,
    })
}

fn quantity_note(spec: &ResourceSpec) -> String {
    if let Some(n) = spec.instance_count {
        if n > Decimal::ONE {
            if spec.unit == "hours" {
                if let Some(h) = spec.hours {
                    return format!(" × {n} resources × {h} h/mo");
                }
            }
            return format!(" × {n} resources");
        }
    }
    if spec.unit == "hours" {
        if let (Some(n), Some(h)) = (spec.instance_count, spec.hours) {
            if n > Decimal::ONE || h != spec.quantity {
                return format!(" × {n} resources × {h} h/mo");
            }
        }
    }
    String::new()
}

pub async fn estimate_resource(
    db: &Database,
    provider: CloudProvider,
    spec: &ResourceSpec,
    live: bool,
) -> Result<LineItem> {
    let provider_key = provider.as_str();
    let sku = spec.sku.as_deref().unwrap_or(&spec.service);

    if live {
        match sync::fetch_live_price(db, provider, &spec.service, sku, &spec.region).await {
            Ok(price) => return line_item(provider, spec, sku, &price, true),
            Err(e) => tracing::warn!(
                provider = provider_key,
                service = %spec.service,
                sku = %sku,
                region = %spec.region,
                error = %e,
                "live price fetch failed, falling back to cache"
            ),
        }
    }

    Ok(estimate_resource_from_cache(db, provider, spec, provider_key)?)
}

fn line_item(
    provider: CloudProvider,
    spec: &ResourceSpec,
    sku: &str,
    price: &str,
    live: bool,
) -> Result<LineItem> {
    let unit_price: Decimal = price.parse().context("parse price")?;
    let monthly = unit_price * spec.quantity;
    let category = spec
        .category
        .clone()
        .unwrap_or_else(|| spec.service.clone());
    let display = spec
        .display_name
        .clone()
        .unwrap_or_else(|| sku.to_string());
    let region = spec
        .region_label
        .as_deref()
        .unwrap_or(&spec.region);
    let qty_note = quantity_note(spec);
    let suffix = if live { " (live)" } else { "" };
    Ok(LineItem {
        provider,
        category,
        description: format!("{display} — {sku}{qty_note} in {region}{suffix}"),
        unit_price,
        quantity: spec.quantity,
        unit: spec.unit.clone(),
        monthly_cost: monthly,
    })
}

pub trait PricingProvider {
    fn provider(&self) -> CloudProvider;
    fn estimate_resource(&self, db: &Database, spec: &ResourceSpec) -> Result<LineItem>;
}

pub fn provider_engine(provider: CloudProvider) -> Box<dyn PricingProvider + Send + Sync> {
    match provider {
        CloudProvider::Aws => Box::new(aws::AwsPricing),
        CloudProvider::Azure => Box::new(azure::AzurePricing),
        CloudProvider::Gcp => Box::new(gcp::GcpPricing),
    }
}

pub async fn estimate_resources_live(
    db: &Database,
    provider: CloudProvider,
    resources: &[ResourceSpec],
    live: bool,
) -> Result<Vec<LineItem>> {
    if resources.is_empty() {
        return Ok(vec![]);
    }
    if !live || resources.len() == 1 {
        let mut items = Vec::with_capacity(resources.len());
        for r in resources {
            items.push(estimate_resource(db, provider, r, live).await?);
        }
        return Ok(items);
    }

    let mut set = tokio::task::JoinSet::new();
    for r in resources {
        let db = db.clone();
        let spec = r.clone();
        set.spawn(async move { estimate_resource(&db, provider, &spec, true).await });
    }
    let mut items = Vec::with_capacity(resources.len());
    while let Some(res) = set.join_next().await {
        items.push(res??);
    }
    Ok(items)
}

pub fn estimate_resources(
    db: &Database,
    provider: CloudProvider,
    resources: &[ResourceSpec],
) -> Result<Vec<LineItem>> {
    let engine = provider_engine(provider);
    resources
        .iter()
        .map(|r| engine.estimate_resource(db, r))
        .collect()
}

pub async fn estimate_all_token_rows(
    db: &Database,
    usage: &[TokenUsageSpec],
    live: bool,
) -> Result<Vec<CostRow>> {
    if live {
        sync::refresh_token_prices(db, usage).await.ok();
    }

    let mut rows = Vec::new();
    for u in usage {
        let monthly = estimate_token_cost(db, u)?;
        let label = u
            .display_name
            .clone()
            .unwrap_or_else(|| u.model.clone());
        let rate_note = db
            .lookup_token_price(
                u.cloud().map(|p| p.as_str()).unwrap_or(u.provider.as_str()),
                &u.model,
            )
            .ok()
            .flatten()
            .map(|(inp, out)| format!("${inp}/M in · ${out}/M out"))
            .unwrap_or_default();
        let total_tokens = u.input_tokens_per_month + u.output_tokens_per_month;
        let unit_price = if total_tokens > 0 {
            monthly / (Decimal::from(total_tokens) / Decimal::from(1_000_000u64))
        } else {
            Decimal::ZERO
        };
        rows.push(CostRow {
            category: "LLM".into(),
            service: label,
            description: format!(
                "{} in / {} out tokens per month{}",
                u.input_tokens_per_month,
                u.output_tokens_per_month,
                if rate_note.is_empty() {
                    String::new()
                } else {
                    format!(" ({rate_note})")
                }
            ),
            unit_price,
            quantity: Decimal::from(total_tokens),
            unit: "tokens/month".into(),
            usage_display: format!(
                "{} in · {} out",
                u.input_tokens_per_month, u.output_tokens_per_month
            ),
            costs: crate::models::PeriodBreakdown::from_monthly(monthly),
        });
    }
    Ok(rows)
}

pub fn estimate_all_tokens(db: &Database, usage: &[TokenUsageSpec]) -> Result<Decimal> {
    usage
        .iter()
        .map(|u| estimate_token_cost(db, u))
        .try_fold(Decimal::ZERO, |acc, c| Ok(acc + c?))
}
