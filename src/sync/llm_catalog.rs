use crate::db::Database;
use crate::models::TokenUsageSpec;
use crate::sync::catalog::azure::azure_retail_skip_at_limit;
use crate::sync::pricing_common::fetch_json_retry;
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use urlencoding::encode;

const AWS_PRICING_HOST: &str = "https://pricing.us-east-1.amazonaws.com";
const BEDROCK_OFFER: &str = "AmazonBedrockFoundationModels";
const BEDROCK_REGION: &str = "us-east-1";
const AZURE_FOUNDRY_FILTER: &str = "serviceName eq 'Foundry Models'";

/// Fallback when cloud APIs are unreachable (startup + offline).
const BASELINE: &[(&str, &str, &str, &str, &str)] = &[
    ("aws", "bedrock-claude-sonnet", "Claude Sonnet (Bedrock)", "3.00", "15.00"),
    ("aws", "bedrock-llama3-70b", "Llama 3 70B (Bedrock)", "2.65", "3.50"),
    ("azure", "gpt-4o", "GPT-4o (Azure OpenAI)", "2.50", "10.00"),
    ("azure", "gpt-4o-mini", "GPT-4o Mini (Azure OpenAI)", "0.15", "0.60"),
    ("gcp", "gemini-1.5-pro", "Gemini 1.5 Pro (Vertex)", "1.25", "5.00"),
    ("gcp", "gemini-1.5-flash", "Gemini 1.5 Flash (Vertex)", "0.075", "0.30"),
];

#[derive(Default)]
struct RateAgg {
    label: String,
    input: Option<(i32, f64)>,
    output: Option<(i32, f64)>,
    regions: HashSet<String>,
    global: bool,
}

/// Quick seed so the UI has models before the first async sync finishes.
pub fn seed_baseline(db: &Database) -> Result<usize> {
    upsert_models(db, BASELINE)
}

pub async fn sync_all(db: &Database) -> Result<usize> {
    let mut total = 0usize;
    match sync_aws_bedrock(db, None).await {
        Ok(n) => {
            tracing::info!(provider = "aws", models = n, "llm catalog sync complete");
            total += n;
        }
        Err(e) => tracing::warn!(provider = "aws", error = %e, "llm catalog sync failed"),
    }
    match sync_azure_foundry(db, None).await {
        Ok(n) => {
            tracing::info!(provider = "azure", models = n, "llm catalog sync complete");
            total += n;
        }
        Err(e) => tracing::warn!(provider = "azure", error = %e, "llm catalog sync failed"),
    }
    total += sync_gcp_vertex(db, None).await.unwrap_or(0);
    if total == 0 {
        if let Ok(n) = seed_baseline(db) {
            if n > 0 {
                tracing::warn!(
                    models = n,
                    "llm live sync unreachable; using baseline rates until Sync catalog succeeds"
                );
                return Ok(n);
            }
        }
        tracing::warn!("llm catalog live sync returned nothing; check network/proxy to AWS/Azure pricing APIs");
        return Ok(0);
    }
    Ok(total)
}

/// Refresh token rates only for models referenced in an estimate (live pricing path).
pub async fn sync_selected(db: &Database, usage: &[TokenUsageSpec]) -> Result<usize> {
    use std::collections::HashSet;
    if usage.is_empty() {
        return Ok(0);
    }

    let mut aws: HashSet<String> = HashSet::new();
    let mut azure: HashSet<String> = HashSet::new();
    let mut gcp: HashSet<String> = HashSet::new();
    for u in usage {
        let provider = u.cloud().map(|p| p.as_str()).unwrap_or(u.provider.as_str());
        match provider {
            "aws" => {
                aws.insert(u.model.clone());
            }
            "azure" => {
                azure.insert(u.model.clone());
            }
            "gcp" => {
                gcp.insert(u.model.clone());
            }
            _ => {}
        }
    }

    let mut total = 0usize;
    if !aws.is_empty() {
        let ids: Vec<String> = aws.into_iter().collect();
        match sync_aws_bedrock(db, Some(&ids)).await {
            Ok(n) => {
                tracing::info!(provider = "aws", models = n, "selective llm price sync");
                total += n;
            }
            Err(e) => tracing::warn!(provider = "aws", error = %e, "selective llm sync failed"),
        }
    }
    if !azure.is_empty() {
        let ids: Vec<String> = azure.into_iter().collect();
        match sync_azure_foundry(db, Some(&ids)).await {
            Ok(n) => {
                tracing::info!(provider = "azure", models = n, "selective llm price sync");
                total += n;
            }
            Err(e) => tracing::warn!(provider = "azure", error = %e, "selective llm sync failed"),
        }
    }
    if !gcp.is_empty() {
        let ids: Vec<String> = gcp.into_iter().collect();
        total += sync_gcp_vertex(db, Some(&ids)).await.unwrap_or(0);
    }
    Ok(total)
}

async fn sync_gcp_vertex(db: &Database, only: Option<&[String]>) -> Result<usize> {
    use std::collections::HashSet;
    let want: Option<HashSet<&str>> = only.map(|ids| ids.iter().map(|s| s.as_str()).collect());
    use crate::sync::gcp_billing;
    let services = match gcp_billing::list_services().await {
        Ok(s) => s,
        Err(e) => {
            tracing::info!(error = %e, "gcp llm catalog skipped — set GCP_PRICING_API_KEY");
            return Ok(0);
        }
    };
    let mut by_model: HashMap<String, RateAgg> = HashMap::new();
    for service in services {
        let svc_name = service
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let lower = svc_name.to_ascii_lowercase();
        if !lower.contains("vertex") && !lower.contains("gemini") {
            continue;
        }
        let billing_id = match service.get("serviceId").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        let skus = match gcp_billing::list_skus(billing_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(service = %svc_name, error = %e, "gcp llm sku fetch failed");
                continue;
            }
        };
        for sku in &skus {
            ingest_gcp_sku(sku, svc_name, &mut by_model, want.as_ref());
        }
    }
    let out = if let Some(want) = want {
        by_model
            .into_iter()
            .filter(|(id, _)| want.contains(id.as_str()))
            .collect()
    } else {
        by_model
    };
    write_aggs(db, "gcp", &out)
}

fn ingest_gcp_sku(
    sku: &Value,
    service_name: &str,
    by_model: &mut HashMap<String, RateAgg>,
    only: Option<&HashSet<&str>>,
) {
    let desc = sku
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lower = desc.to_ascii_lowercase();
    if !lower.contains("token") {
        return;
    }
    let direction = if lower.contains("input") {
        BedrockDirection::Input
    } else if lower.contains("output") {
        BedrockDirection::Output
    } else {
        return;
    };
    let price = match gcp_billing_sku_mtok(sku) {
        Some(p) => p,
        None => return,
    };
    let family = desc
        .split('-')
        .next()
        .unwrap_or(desc)
        .trim()
        .to_string();
    let id = format!("gcp-{}", slugify(&family));
    if let Some(want) = only {
        if !want.contains(id.as_str()) {
            return;
        }
    }
    let label = format!("{family} ({service_name})");
    let entry = by_model.entry(id.clone()).or_default();
    if entry.label.is_empty() {
        entry.label = label;
    }
    bump_rate(
        match direction {
            BedrockDirection::Input => &mut entry.input,
            BedrockDirection::Output => &mut entry.output,
        },
        0,
        price,
    );
}

fn gcp_billing_sku_mtok(sku: &Value) -> Option<f64> {
    let expr = sku.get("pricingInfo")?.as_array()?.first()?;
    let pe = expr.get("pricingExpression")?;
    let unit = pe.get("usageUnit")?.as_str()?.to_ascii_lowercase();
    let tier = pe.get("tieredRates")?.as_array()?.first()?;
    let unit_price = tier.get("unitPrice")?;
    let units: f64 = unit_price
        .get("units")
        .and_then(|u| u.as_str())
        .unwrap_or("0")
        .parse()
        .ok()?;
    let nanos = unit_price.get("nanos").and_then(|n| n.as_i64()).unwrap_or(0) as f64 / 1e9;
    let price = units + nanos;
    if unit.contains("token") && unit.contains("1m") {
        Some(price)
    } else if unit.contains("token") && unit.contains("1k") {
        Some(price * 1000.0)
    } else {
        None
    }
}

async fn sync_aws_bedrock(db: &Database, only: Option<&[String]>) -> Result<usize> {
    use std::collections::HashSet;
    let want: Option<HashSet<&str>> = only.map(|ids| ids.iter().map(|s| s.as_str()).collect());

    let index = fetch_json_retry(&format!("{AWS_PRICING_HOST}/offers/v1.0/aws/index.json"), 3).await?;
    let offer = index
        .get("offers")
        .and_then(|o| o.get(BEDROCK_OFFER))
        .context("AmazonBedrockFoundationModels missing from AWS pricing index")?;
    let region_index_url = offer
        .get("currentRegionIndexUrl")
        .and_then(|u| u.as_str())
        .context("missing bedrock region index url")?;
    let region_index =
        fetch_json_retry(&format!("{AWS_PRICING_HOST}{region_index_url}"), 3).await?;
    let region_entry = region_index
        .get("regions")
        .and_then(|r| r.get(BEDROCK_REGION))
        .context("bedrock us-east-1 region missing")?;
    let pricebook_url = region_entry
        .get("currentVersionUrl")
        .and_then(|u| u.as_str())
        .context("missing bedrock pricebook url")?;
    let book = fetch_json_retry(&format!("{AWS_PRICING_HOST}{pricebook_url}"), 3).await?;

    let products = book
        .get("products")
        .and_then(|p| p.as_object())
        .context("missing bedrock products")?;
    let on_demand = book
        .get("terms")
        .and_then(|t| t.get("OnDemand"))
        .and_then(|o| o.as_object())
        .context("missing bedrock OnDemand terms")?;

    let mut by_model: HashMap<String, RateAgg> = HashMap::new();
    for product in products.values() {
        let attrs = match product.get("attributes").and_then(|a| a.as_object()) {
            Some(a) => a,
            None => continue,
        };
        let service_name = match attrs.get("servicename").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let usage = match attrs.get("usagetype").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => continue,
        };
        let direction = match bedrock_direction(usage) {
            Some(d) => d,
            None => continue,
        };
        let sku = match product.get("sku").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let price = match bedrock_on_demand_mtok(on_demand, sku) {
            Some(p) => p,
            None => continue,
        };
        let score = bedrock_usage_score(usage);
        let id = bedrock_model_id(service_name);
        if let Some(want) = want.as_ref() {
            if !want.contains(id.as_str()) {
                continue;
            }
        }
        let entry = by_model.entry(id).or_default();
        entry.label = service_name.to_string();
        if usage.contains("_Global") {
            entry.global = true;
            entry.regions.clear();
        } else if !entry.global {
            if let Some(region) = bedrock_usage_region(usage) {
                entry.regions.insert(region);
            }
        }
        match direction {
            BedrockDirection::Input => bump_rate(&mut entry.input, score, price),
            BedrockDirection::Output => bump_rate(&mut entry.output, score, price),
        }
    }

    write_aggs(db, "aws", &by_model)
}

async fn sync_azure_foundry(db: &Database, only: Option<&[String]>) -> Result<usize> {
    use std::collections::HashSet;
    let want: Option<HashSet<String>> = only.map(|ids| ids.iter().cloned().collect());
    let mut by_model: HashMap<String, RateAgg> = HashMap::new();

    if let Some(ids) = only {
        for model_id in ids {
            let slug = model_id.strip_prefix("azure-").unwrap_or(model_id.as_str());
            let filter = format!(
                "{AZURE_FOUNDRY_FILTER} and contains(skuName,'{}')",
                slug.replace('\'', "''")
            );
            azure_retail_pages(&filter, &mut by_model, Some(25)).await?;
        }
    } else {
        azure_retail_pages(AZURE_FOUNDRY_FILTER, &mut by_model, None).await?;
    }

    let out = if let Some(want) = want {
        by_model
            .into_iter()
            .filter(|(id, _)| want.contains(id))
            .collect()
    } else {
        by_model
    };
    write_aggs(db, "azure", &out)
}

async fn azure_retail_pages(
    filter: &str,
    by_model: &mut HashMap<String, RateAgg>,
    max_pages: Option<usize>,
) -> Result<()> {
    let mut url = Some(format!(
        "https://prices.azure.com/api/retail/prices?$filter={}",
        encode(filter)
    ));
    let mut pages = 0usize;

    while let Some(page_url) = url.take() {
        pages += 1;
        if max_pages.is_some_and(|cap| pages > cap) {
            tracing::debug!(pages, filter, "azure llm selective sync page cap");
            break;
        }
        if azure_retail_skip_at_limit(&page_url) {
            tracing::info!(pages, "azure llm catalog reached retail pagination cap");
            break;
        }
        let json = fetch_json_retry(&page_url, 3).await?;
        if let Some(items) = json.get("Items").and_then(|i| i.as_array()) {
            for item in items {
                ingest_azure_item(item, by_model);
            }
        }
        url = json
            .get("NextPageLink")
            .and_then(|n| n.as_str())
            .filter(|u| !azure_retail_skip_at_limit(u))
            .map(str::to_string);
        if max_pages.is_none() && pages.is_multiple_of(50) {
            tracing::info!(pages, models = by_model.len(), "azure llm catalog scan progress");
        }
    }
    Ok(())
}

fn ingest_azure_item(item: &Value, by_model: &mut HashMap<String, RateAgg>) {
    if item.get("type").and_then(|t| t.as_str()) != Some("Consumption") {
        return;
    }
    let product = item.get("productName").and_then(|v| v.as_str()).unwrap_or("");
    let sku = item
        .get("skuName")
        .or_else(|| item.get("armSkuName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let meter = item.get("meterName").and_then(|v| v.as_str()).unwrap_or("");
    let unit = item.get("unitOfMeasure").and_then(|v| v.as_str()).unwrap_or("");
    let price = match item.get("retailPrice").and_then(|v| v.as_f64()) {
        Some(p) => p,
        None => return,
    };
    let direction = match azure_direction(sku, meter) {
        Some(d) => d,
        None => return,
    };
    if !azure_token_unit(unit) {
        return;
    }
    let per_mtok = azure_to_mtok(price, unit);
    let score = azure_rate_score(sku, meter);
    let (id, label) = azure_model_key(product, sku);
    let entry = by_model.entry(id).or_default();
    entry.label = label;
    match direction {
        BedrockDirection::Input => bump_rate(&mut entry.input, score, per_mtok),
        BedrockDirection::Output => bump_rate(&mut entry.output, score, per_mtok),
    }
}

fn write_aggs(db: &Database, provider: &str, by_model: &HashMap<String, RateAgg>) -> Result<usize> {
    let now = Utc::now();
    let mut count = 0usize;
    for (id, agg) in by_model {
        let input = agg.input.map(|(_, p)| p).unwrap_or(0.0);
        let output = agg.output.map(|(_, p)| p).unwrap_or(0.0);
        if input == 0.0 && output == 0.0 {
            continue;
        }
        let region_list: Vec<String> = agg.regions.iter().cloned().collect();
        db.upsert_llm_model(
            id,
            &agg.label,
            provider,
            if agg.global || region_list.is_empty() {
                None
            } else {
                Some(region_list.as_slice())
            },
        )?;
        db.upsert_token_price(
            provider,
            id,
            &format_rate(input),
            &format_rate(output),
            "USD",
            now,
        )?;
        count += 1;
    }
    Ok(count)
}

fn upsert_models(db: &Database, rows: &[(&str, &str, &str, &str, &str)]) -> Result<usize> {
    let now = Utc::now();
    for (provider, id, label, input, output) in rows {
        db.upsert_llm_model(id, label, provider, None)?;
        db.upsert_token_price(provider, id, input, output, "USD", now)?;
    }
    Ok(rows.len())
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum BedrockDirection {
    Input,
    Output,
}

fn bedrock_direction(usage: &str) -> Option<BedrockDirection> {
    if usage.contains("Batch")
        || usage.contains("Cache")
        || usage.contains("Reserved")
        || usage.contains("TPM")
        || usage.contains("Provisioned")
    {
        return None;
    }
    if usage.contains("InputTokenCount") {
        Some(BedrockDirection::Input)
    } else if usage.contains("OutputTokenCount") {
        Some(BedrockDirection::Output)
    } else {
        None
    }
}

fn bedrock_usage_score(usage: &str) -> i32 {
    let mut score = 0;
    if usage.contains("InputTokenCount-Units") || usage.contains("OutputTokenCount-Units") {
        score += 10;
    }
    if usage.contains("_Global-") {
        score -= 3;
    }
    score
}

fn bedrock_on_demand_mtok(on_demand: &serde_json::Map<String, Value>, sku: &str) -> Option<f64> {
    let term = on_demand.get(sku)?.as_object()?;
    for offer in term.values() {
        let dims = offer.get("priceDimensions")?.as_object()?;
        for dim in dims.values() {
            let unit = dim.get("unit")?.as_str()?;
            if !unit.to_ascii_lowercase().contains("token") {
                continue;
            }
            let usd = dim.get("pricePerUnit")?.get("USD")?.as_str()?;
            return usd.parse().ok();
        }
    }
    None
}

fn bedrock_model_id(service_name: &str) -> String {
    let core = service_name
        .trim_end_matches(" (Amazon Bedrock Edition)")
        .trim();
    format!("bedrock-{}", slugify(core))
}

fn bedrock_usage_region(usage: &str) -> Option<String> {
    if usage.contains("_Global") {
        return None;
    }
    let prefix = usage.split('-').next()?;
    Some(match prefix {
        "USE1" => "us-east-1",
        "USE2" => "us-east-2",
        "USW1" => "us-west-1",
        "USW2" => "us-west-2",
        "EUW1" => "eu-west-1",
        "EUW2" => "eu-west-2",
        "EUW3" => "eu-west-3",
        "EUC1" => "eu-central-1",
        "EUN1" => "eu-north-1",
        "APS1" => "ap-southeast-1",
        "APS2" => "ap-southeast-2",
        "APN1" => "ap-northeast-1",
        "APN2" => "ap-northeast-2",
        "SAE1" => "sa-east-1",
        "CAN1" => "ca-central-1",
        "MES1" => "me-south-1",
        "MEC1" => "me-central-1",
        "AFS1" => "af-south-1",
        "ILC1" => "il-central-1",
        _ => return None,
    }
    .to_string())
}

fn azure_direction(sku: &str, meter: &str) -> Option<BedrockDirection> {
    let s = format!("{sku} {meter}").to_ascii_lowercase();
    if s.contains("batch") || s.contains("ft-trng") || s.contains("training") {
        return None;
    }
    if s.contains(" inp") || s.contains("-inp") || s.contains("input") {
        Some(BedrockDirection::Input)
    } else if s.contains(" outp") || s.contains("-outp") || s.contains("output") {
        Some(BedrockDirection::Output)
    } else {
        None
    }
}

fn azure_token_unit(unit: &str) -> bool {
    matches!(unit, "1M" | "1K")
}

fn azure_to_mtok(price: f64, unit: &str) -> f64 {
    match unit {
        "1M" => price,
        "1K" => price * 1000.0,
        _ => price,
    }
}

fn azure_rate_score(sku: &str, meter: &str) -> i32 {
    let s = format!("{sku} {meter}").to_ascii_lowercase();
    let mut score = 0;
    if s.contains("glbl") || s.contains("global") {
        score += 20;
    }
    if s.contains("regnl") || s.contains("dzone") || s.contains("dzn") {
        score -= 4;
    }
    if s.contains("cchd") || s.contains("cache") {
        score -= 15;
    }
    if s.contains("rt-") {
        score -= 8;
    }
    score
}

fn azure_model_key(product: &str, sku: &str) -> (String, String) {
    let family = sku
        .split_whitespace()
        .next()
        .unwrap_or(sku)
        .split('-')
        .take_while(|part| {
            !part.eq_ignore_ascii_case("inp")
                && !part.eq_ignore_ascii_case("outp")
                && !part.eq_ignore_ascii_case("input")
                && !part.eq_ignore_ascii_case("output")
        })
        .collect::<Vec<_>>()
        .join("-");
    let family = family.trim_end_matches('-');
    let family = if family.is_empty() { sku } else { family };
    let id = format!("azure-{}", slugify(family));
    let product_short = product
        .strip_prefix("Azure ")
        .unwrap_or(product)
        .trim();
    let label = if product_short.is_empty() {
        family.to_string()
    } else {
        format!("{family} ({product_short})")
    };
    (id, label)
}

fn bump_rate(slot: &mut Option<(i32, f64)>, score: i32, price: f64) {
    match slot {
        Some((best, _)) if *best >= score => {}
        _ => *slot = Some((score, price)),
    }
}

fn format_rate(v: f64) -> String {
    format!("{:.4}", v)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_picks_on_demand_input() {
        assert_eq!(
            bedrock_direction("USE1-MP:USE1_InputTokenCount-Units"),
            Some(BedrockDirection::Input)
        );
        assert_eq!(bedrock_direction("USE1-MP:USE1_InputTokenCount_Batch-Units"), None);
    }

    #[test]
    fn azure_model_key_groups_gpt4o() {
        let (id, label) = azure_model_key("Azure OpenAI", "gpt-4o-0806-Inp-glbl");
        assert_eq!(id, "azure-gpt-4o-0806");
        assert!(label.contains("gpt-4o-0806"));
    }

    #[test]
    fn azure_to_mtok_scales_1k() {
        assert!((azure_to_mtok(0.0025, "1K") - 2.5).abs() < 0.001);
    }

    #[test]
    fn bedrock_usage_region_maps_sae1() {
        assert_eq!(
            bedrock_usage_region("SAE1-MP:SAE1_InputTokenCount-Units"),
            Some("sa-east-1".into())
        );
    }

    #[test]
    fn azure_selective_filter_includes_slug() {
        let filter = format!(
            "{AZURE_FOUNDRY_FILTER} and contains(skuName,'{}')",
            "gpt-4o-0806".replace('\'', "''")
        );
        assert!(filter.contains("Foundry Models"));
        assert!(filter.contains("gpt-4o-0806"));
    }
}
