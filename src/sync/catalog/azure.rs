use super::specs;
use crate::db::ProviderServiceIngest;
use crate::db::Database;
use crate::sync::pricing_common::fetch_json;
use anyhow::Result;
use chrono::Utc;
use std::collections::{HashMap, HashSet};

struct AzureServiceAgg {
    sku: String,
    regions: HashSet<String>,
}

pub async fn sync(db: &Database) -> Result<usize> {
    sync_pages(db, 0).await
}

/// Paginate Azure retail prices. `max_pages` 0 = full global catalog (production default).
#[doc(hidden)]
pub async fn sync_pages(db: &Database, max_pages: usize) -> Result<usize> {
    let now = Utc::now();
    let mut by_service: HashMap<String, AzureServiceAgg> = HashMap::new();
    let mut url = Some("https://prices.azure.com/api/retail/prices".to_string());
    let mut pages = 0usize;

    while let Some(page_url) = url.take() {
        pages += 1;
        if max_pages > 0 && pages > max_pages {
            tracing::warn!(max_pages, pages, "azure catalog test scan page limit reached");
            break;
        }
        if azure_retail_skip_at_limit(&page_url) {
            tracing::info!(
                pages,
                services = by_service.len(),
                "azure catalog reached Retail API pagination cap (skip=1000000); snapshot complete"
            );
            break;
        }
        let json = match crate::sync::pricing_common::fetch_json_retry(&page_url, 3).await {
            Ok(j) => j,
            Err(e) => {
                if azure_retail_skip_at_limit(&page_url) {
                    tracing::info!(
                        pages,
                        services = by_service.len(),
                        "azure catalog reached Retail API pagination cap; snapshot complete"
                    );
                    break;
                }
                tracing::error!(pages, error = %e, "azure catalog page failed");
                if by_service.is_empty() {
                    return Err(e);
                }
                tracing::warn!(
                    pages,
                    services = by_service.len(),
                    "azure catalog saving partial snapshot after page failure"
                );
                break;
            }
        };
        if let Some(items) = json.get("Items").and_then(|i| i.as_array()) {
            for item in items {
                let Some(service_name) = item.get("serviceName").and_then(|v| v.as_str()) else {
                    continue;
                };
                let sku = item
                    .get("armSkuName")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Standard")
                    .to_string();
                let region = item
                    .get("armRegionName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("eastus")
                    .to_string();
                let entry = by_service
                    .entry(service_name.to_string())
                    .or_insert(AzureServiceAgg {
                        sku: sku.clone(),
                        regions: HashSet::new(),
                    });
                entry.regions.insert(region);
            }
        }
        url = json
            .get("NextPageLink")
            .and_then(|n| n.as_str())
            .map(str::to_string);
        if pages.is_multiple_of(50) {
            tracing::info!(pages, services = by_service.len(), "azure catalog scan progress");
        }
    }

    let distinct_regions: usize = by_service
        .values()
        .flat_map(|agg| agg.regions.iter())
        .collect::<HashSet<_>>()
        .len();

    let mut count = 0usize;
    for (service_name, agg) in by_service {
        let raw_regions: Vec<String> = agg.regions.into_iter().collect();
        let regions =
            crate::sync::pricing_common::filter_azure_service_regions(raw_regions);
        let category_id = specs::infer_category(&service_name);
        let unit = specs::infer_unit(&service_name);
        let service_key = slugify(&service_name);
        let row = ProviderServiceIngest {
            catalog_id: format!("azure:{service_key}"),
            provider: "azure".into(),
            service_key: service_key.clone(),
            display_name: service_name.clone(),
            category_id: category_id.into(),
            unit: unit.into(),
            default_sku: agg.sku.clone(),
            offer_code: None,
            attr_key: Some("serviceName".into()),
            attr_value: Some(service_name.clone()),
            billing_service_id: None,
            sku_description_hint: Some(agg.sku),
            regions,
        };
        db.upsert_provider_service(&row, now)?;
        count += 1;
    }
    if max_pages > 0 {
        tracing::warn!(
            max_pages,
            pages,
            services = count,
            distinct_regions,
            "azure catalog partial scan (test page limit only)"
        );
    } else {
        tracing::info!(
            pages,
            services = count,
            distinct_regions,
            "azure catalog sync complete"
        );
    }
    Ok(count)
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

/// Azure Retail Prices API stops accepting `$skip` at 1_000_000 (~1000 pages).
pub(crate) fn azure_retail_skip_at_limit(url: &str) -> bool {
    url.split('$').any(|part| {
        part.strip_prefix("skip=")
            .and_then(|v| v.split('&').next())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|skip| skip >= 1_000_000)
    })
}

/// Live retail price for a known Azure service + sku + region.
pub async fn fetch_retail_price(
    service_name: &str,
    sku_name: &str,
    region: &str,
    live: bool,
) -> Result<String> {
    use anyhow::Context;
    use urlencoding::encode;

    let sku_name = if sku_name.is_empty() {
        "Standard"
    } else {
        sku_name
    };
    let service_name = service_name.replace('\'', "''");
    let sku_name = sku_name.replace('\'', "''");
    let region = region.replace('\'', "''");
    let filter = format!(
        "serviceName eq '{service_name}' and armSkuName eq '{sku_name}' and armRegionName eq '{region}' and priceType eq 'Consumption'"
    );
    let url = format!(
        "https://prices.azure.com/api/retail/prices?$filter={}",
        encode(&filter)
    );
    let json = if live {
        crate::sync::pricing_common::fetch_json_retry_live(&url).await?
    } else {
        fetch_json(&url).await?
    };
    let price = json
        .get("Items")
        .and_then(|i| i.as_array())
        .and_then(|a| a.first())
        .and_then(|item| item.get("retailPrice"))
        .and_then(|p| p.as_f64())
        .context("no retail price in Azure response")?;
    Ok(format!("{price}"))
}

/// Distinct ARM SKU names for an Azure service in a region (VM sizes, tiers, etc.).
pub async fn list_retail_skus(service_name: &str, region: &str) -> Result<Vec<String>> {
    use std::collections::BTreeSet;
    use urlencoding::encode;

    let filter = format!(
        "serviceName eq '{}' and armRegionName eq '{}' and priceType eq 'Consumption'",
        service_name.replace('\'', "''"),
        region.replace('\'', "''"),
    );
    let mut url = Some(format!(
        "https://prices.azure.com/api/retail/prices?$filter={}",
        encode(&filter)
    ));
    let mut skus = BTreeSet::new();
    let mut pages = 0usize;

    while let Some(page_url) = url.take() {
        pages += 1;
        if pages > 20 {
            break;
        }
        let json = fetch_json(&page_url).await?;
        if let Some(items) = json.get("Items").and_then(|i| i.as_array()) {
            for item in items {
                if let Some(sku) = item.get("armSkuName").and_then(|v| v.as_str()) {
                    if !sku.is_empty() {
                        skus.insert(sku.to_string());
                    }
                }
            }
        }
        url = json
            .get("NextPageLink")
            .and_then(|n| n.as_str())
            .map(str::to_string);
    }

    Ok(skus.into_iter().collect())
}
