use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
static HTTP_LIVE: OnceLock<reqwest::Client> = OnceLock::new();

pub fn http_client() -> &'static reqwest::Client {
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .build()
            .expect("http client")
    })
}

/// Shorter timeouts for user-triggered live price capture on Calculate.
pub fn http_client_live() -> &'static reqwest::Client {
    HTTP_LIVE.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(8)
            .build()
            .expect("http client live")
    })
}

/// Extract On-Demand USD price from an AWS Price List regional offer index.
pub fn aws_on_demand_price(index: &Value, attr_key: &str, attr_value: &str) -> Result<Option<String>> {
    let products = index
        .get("products")
        .and_then(|p| p.as_object())
        .context("missing products")?;
    let on_demand = index
        .get("terms")
        .and_then(|t| t.get("OnDemand"))
        .and_then(|o| o.as_object())
        .context("missing OnDemand terms")?;

    for (sku, product) in products {
        let attrs = match product.get("attributes").and_then(|a| a.as_object()) {
            Some(a) => a,
            None => continue,
        };
        if attrs.get(attr_key).and_then(|v| v.as_str()) != Some(attr_value) {
            continue;
        }
        let term = match on_demand.get(sku).and_then(|t| t.as_object()) {
            Some(t) => t,
            None => continue,
        };
        for offer in term.values() {
            if let Some(dims) = offer.get("priceDimensions").and_then(|d| d.as_object()) {
                for dim in dims.values() {
                    if let Some(usd) = dim
                        .get("pricePerUnit")
                        .and_then(|p| p.get("USD"))
                        .and_then(|u| u.as_str())
                    {
                        return Ok(Some(usd.to_string()));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// First OnDemand USD price and its attribute pair from an AWS regional offer index.
pub fn aws_first_on_demand(index: &Value) -> Option<(String, String, String)> {
    let products = index.get("products")?.as_object()?;
    let on_demand = index.get("terms")?.get("OnDemand")?.as_object()?;

    for (sku, product) in products {
        let attrs = product.get("attributes")?.as_object()?;
        let term = on_demand.get(sku)?.as_object()?;
        for offer in term.values() {
            if let Some(dims) = offer.get("priceDimensions").and_then(|d| d.as_object()) {
                for dim in dims.values() {
                    if let Some(usd) = dim
                        .get("pricePerUnit")
                        .and_then(|p| p.get("USD"))
                        .and_then(|u| u.as_str())
                    {
                        let (attr_key, attr_value) = attrs
                            .iter()
                            .find(|(k, _)| *k != "location" && *k != "locationType")
                            .map(|(k, v)| (k.to_string(), v.as_str().unwrap_or("").to_string()))?;
                        return Some((attr_key, attr_value, usd.to_string()));
                    }
                }
            }
        }
    }
    None
}

/// Distinct attribute values (e.g. EC2 instance types) with On-Demand pricing in an AWS offer index.
pub fn aws_list_attr_values(index: &Value, attr_key: &str) -> Vec<String> {
    use std::collections::BTreeSet;
    let Some(products) = index.get("products").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let Some(on_demand) = index
        .get("terms")
        .and_then(|t| t.get("OnDemand"))
        .and_then(|o| o.as_object())
    else {
        return Vec::new();
    };

    let mut values = BTreeSet::new();
    for (sku, product) in products {
        if !on_demand.contains_key(sku) {
            continue;
        }
        let Some(attrs) = product.get("attributes").and_then(|a| a.as_object()) else {
            continue;
        };
        if let Some(v) = attrs.get(attr_key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                values.insert(v.to_string());
            }
        }
    }
    values.into_iter().collect()
}

pub async fn fetch_json(url: &str) -> Result<Value> {
    fetch_json_retry_inner(url, 0, false).await
}

pub async fn fetch_json_retry(url: &str, retries: u32) -> Result<Value> {
    fetch_json_retry_inner(url, retries, false).await
}

/// Live pricing on Calculate — shorter timeout, fewer retries.
pub async fn fetch_json_retry_live(url: &str) -> Result<Value> {
    fetch_json_retry_inner(url, 1, true).await
}

async fn fetch_json_retry_inner(url: &str, retries: u32, live: bool) -> Result<Value> {
    let mut last_err = None;
    for attempt in 0..=retries {
        match fetch_json_once(url, live).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt < retries {
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("fetch failed for {url}")))
}

async fn fetch_json_once(url: &str, live: bool) -> Result<Value> {
    let client = if live {
        http_client_live()
    } else {
        http_client()
    };
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        anyhow::bail!("HTTP {status} for {url}: {snippet}");
    }
    resp.json().await.context("parse JSON response")
}

/// Standard commercial AWS region codes (excludes Local Zones, Wavelength, GovCloud, China).
pub fn is_standard_aws_region(region: &str) -> bool {
    if region.starts_with("us-gov") || region.starts_with("cn-") {
        return false;
    }
    region.matches('-').count() == 2
}

pub fn filter_standard_aws_regions(regions: Vec<String>) -> Vec<String> {
    regions
        .into_iter()
        .filter(|r| is_standard_aws_region(r))
        .collect()
}

/// Azure commercial regions + `global`; excludes edge/carrier sites (e.g. attdetroit1).
pub fn is_standard_azure_region(region: &str) -> bool {
    let r = region.to_lowercase();
    if r == "global" {
        return true;
    }
    if r.starts_with("att")
        || r.starts_with("verizon")
        || r.contains("edge")
        || r.starts_with("intercontinental")
    {
        return false;
    }
    (4..=20).contains(&r.len()) && r.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Filter Azure retail API regions before writing to SQLite (no static substitution).
pub fn filter_azure_service_regions(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let raw: Vec<String> = raw.into_iter().collect();
    let had_global = raw.iter().any(|r| r.eq_ignore_ascii_case("global"));
    let mut regions: Vec<String> = raw
        .into_iter()
        .filter(|r| is_standard_azure_region(r))
        .collect();
    if had_global && !regions.iter().any(|r| r.eq_ignore_ascii_case("global")) {
        regions.push("global".into());
    }
    regions.sort();
    regions.dedup();
    regions
}

/// GCP commercial regions (excludes zones); used for catalog + UI sub-region lists.
pub fn is_standard_gcp_region(region: &str) -> bool {
    let r = region.to_lowercase();
    if r == "global" {
        return true;
    }
    let parts: Vec<&str> = r.split('-').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }
    // Drop zone suffixes like us-central1-a
    if parts.len() == 3 && parts[2].len() <= 2 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Filter GCP SKU/API regions before writing to SQLite (no static substitution).
pub fn filter_gcp_service_regions(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let raw: Vec<String> = raw.into_iter().collect();
    let had_global = raw.iter().any(|r| r.eq_ignore_ascii_case("global"));
    let mut regions: Vec<String> = raw
        .into_iter()
        .filter(|r| is_standard_gcp_region(r))
        .collect();
    if had_global && !regions.iter().any(|r| r.eq_ignore_ascii_case("global")) {
        regions.insert(0, "global".into());
    }
    regions.sort();
    regions.dedup();
    regions
}

#[cfg(test)]
mod tests {
    use crate::db::PriceTarget;
    use std::collections::HashMap;

    fn batch_price_targets(targets: Vec<PriceTarget>, anchor: &str) -> Vec<PriceTarget> {
        let mut by_key: HashMap<(String, String), Vec<PriceTarget>> = HashMap::new();
        for t in targets {
            by_key
                .entry((t.service_key.clone(), t.sku.clone()))
                .or_default()
                .push(t);
        }
        by_key
            .into_values()
            .filter_map(|group| {
                group
                    .iter()
                    .find(|t| t.region == anchor)
                    .cloned()
                    .or_else(|| {
                        group
                            .iter()
                            .find(|t| super::is_standard_aws_region(&t.region))
                            .cloned()
                    })
                    .or_else(|| group.first().cloned())
            })
            .collect()
    }

    fn target(service: &str, sku: &str, region: &str) -> PriceTarget {
        PriceTarget {
            service_key: service.into(),
            sku: sku.into(),
            region: region.into(),
            unit: "unit".into(),
            offer_code: None,
            attr_key: None,
            attr_value: None,
            billing_service_id: None,
            sku_description_hint: None,
        }
    }

    #[test]
    fn is_standard_aws_region_filters_local_zones() {
        assert!(super::is_standard_aws_region("us-east-1"));
        assert!(!super::is_standard_aws_region("us-east-1-bos-1"));
        assert!(!super::is_standard_aws_region("ap-northeast-1-wl1-nrt1"));
        assert!(!super::is_standard_aws_region("us-gov-east-1"));
    }

    #[test]
    fn is_standard_azure_region_filters_edge_pops() {
        assert!(super::is_standard_azure_region("eastus"));
        assert!(super::is_standard_azure_region("global"));
        assert!(!super::is_standard_azure_region("attdetroit1"));
        assert!(!super::is_standard_azure_region("verizon-east-us"));
    }

    #[test]
    fn filter_azure_service_regions_drops_edge_pops() {
        let out = super::filter_azure_service_regions(["attdetroit1".into(), "eastus".into()]);
        assert!(out.contains(&"eastus".to_string()));
        assert!(!out.contains(&"attdetroit1".to_string()));
    }

    #[test]
    fn filter_azure_service_regions_preserves_global() {
        let out = super::filter_azure_service_regions(["attdetroit1".into(), "global".into()]);
        assert!(out.contains(&"global".to_string()));
    }

    #[test]
    fn filter_gcp_service_regions_filters_junk() {
        let out = super::filter_gcp_service_regions(["bad".into(), "us-central1".into()]);
        assert!(out.contains(&"us-central1".to_string()));
        assert!(!out.contains(&"bad".to_string()));
    }

    #[test]
    fn batch_price_targets_prefers_anchor() {
        let targets = vec![
            target("AmazonEC2", "t3.micro", "eu-west-1"),
            target("AmazonEC2", "t3.micro", "us-east-1"),
        ];
        let out = batch_price_targets(targets, "us-east-1");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].region, "us-east-1");
    }

    #[test]
    fn batch_price_targets_falls_back_when_anchor_missing() {
        let targets = vec![target("AmazonEC2", "t3.micro", "eu-west-1")];
        let out = batch_price_targets(targets, "us-east-1");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].region, "eu-west-1");
    }
}
