use crate::sync::pricing_common::{fetch_json, fetch_json_retry_live};
use anyhow::{Context, Result};
use serde_json::Value;

const GCP_BILLING_BASE: &str = "https://cloudbilling.googleapis.com/v1";

pub fn api_key() -> Option<String> {
    std::env::var("GCP_PRICING_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub async fn list_services() -> Result<Vec<Value>> {
    let key = api_key().context("GCP_PRICING_API_KEY not set")?;
    let mut url = Some(format!("{GCP_BILLING_BASE}/services?key={key}"));
    let mut services = Vec::new();

    while let Some(page_url) = url.take() {
        let json = fetch_json(&page_url).await?;
        if let Some(page) = json.get("services").and_then(|s| s.as_array()) {
            services.extend(page.iter().cloned());
        }
        url = json
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|token| format!("{GCP_BILLING_BASE}/services?key={key}&pageToken={token}"));
    }
    Ok(services)
}

pub async fn list_skus(billing_service_id: &str) -> Result<Vec<Value>> {
    list_skus_inner(billing_service_id, false).await
}

pub async fn list_skus_live(billing_service_id: &str) -> Result<Vec<Value>> {
    list_skus_inner(billing_service_id, true).await
}

async fn list_skus_inner(billing_service_id: &str, live: bool) -> Result<Vec<Value>> {
    let key = api_key().context("GCP_PRICING_API_KEY not set")?;
    let mut url = Some(format!(
        "{GCP_BILLING_BASE}/services/{billing_service_id}/skus?key={key}&currencyCode=USD"
    ));
    let mut skus = Vec::new();

    while let Some(page_url) = url.take() {
        let json = if live {
            fetch_json_retry_live(&page_url).await?
        } else {
            fetch_json(&page_url).await?
        };
        if let Some(page) = json.get("skus").and_then(|s| s.as_array()) {
            skus.extend(page.iter().cloned());
        }
        url = json
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|token| {
                format!(
                    "{GCP_BILLING_BASE}/services/{billing_service_id}/skus?key={key}&currencyCode=USD&pageToken={token}"
                )
            });
    }
    Ok(skus)
}

pub fn find_sku<'a>(skus: &'a [Value], description_contains: &str) -> Option<&'a Value> {
    skus.iter().find(|sku| {
        sku.get("skuId")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id == description_contains)
            || sku
                .get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|n| n == description_contains)
            || sku
                .get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.contains(description_contains))
    })
}

pub fn sku_regions(sku: &Value) -> Vec<String> {
    sku.get("serviceRegions")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Union of all regions advertised across a billing service's SKUs.
pub fn collect_service_regions(skus: &[Value]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut regions = BTreeSet::new();
    for sku in skus {
        for region in sku_regions(sku) {
            regions.insert(region);
        }
    }
    regions.into_iter().collect()
}

pub fn sku_price(sku: &Value) -> Option<String> {
    let unit_price = sku
        .get("pricingInfo")?
        .as_array()?
        .first()?
        .get("pricingExpression")?
        .get("tieredRates")?
        .as_array()?
        .first()?
        .get("unitPrice")?;
    let units: f64 = unit_price
        .get("units")
        .and_then(|u| u.as_str())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    let nanos = unit_price
        .get("nanos")
        .and_then(|n| n.as_i64())
        .unwrap_or(0) as f64
        / 1e9;
    Some(format!("{}", units + nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_service_regions_unions_all_skus() {
        let skus = vec![
            json!({ "serviceRegions": ["us-central1", "europe-west1"] }),
            json!({ "serviceRegions": ["asia-east1"] }),
        ];
        let regions = collect_service_regions(&skus);
        assert_eq!(regions.len(), 3);
        assert!(regions.contains(&"us-central1".to_string()));
        assert!(regions.contains(&"europe-west1".to_string()));
        assert!(regions.contains(&"asia-east1".to_string()));
    }
}
