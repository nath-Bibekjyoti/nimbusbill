use crate::catalog::resolve_catalog_resource;
use crate::db::Database;
use crate::models::{
    CloudProvider, CostRow, CostTable, EstimateRequest, PeriodBreakdown, ProviderUiEstimate,
    RequirementSet, ResourceSpec, UiEstimateResponse,
};
use crate::pricing::{estimate_all_token_rows, estimate_resources_live};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

pub fn run(db: &Database, requirement: RequirementSet) -> Result<crate::models::EstimateResult> {
    let request = EstimateRequest {
        name: requirement.name.clone(),
        providers: CloudProvider::all().to_vec(),
        live_pricing: false,
        resources: requirement
            .resources
            .iter()
            .map(|r| crate::models::UiResourceInput {
                catalog_id: r.catalog_id.clone().unwrap_or_else(|| r.service.clone()),
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
    let ui = run_ui(db, &request)?;
    legacy_from_ui(&ui, requirement)
}

pub async fn run_ui_async(db: &Database, request: &EstimateRequest) -> Result<UiEstimateResponse> {
    let providers = providers_for_request(request);
    if providers.is_empty() {
        bail!("select at least one cloud provider");
    }

    let mut provider_estimates = Vec::new();

    for &provider in &providers {
        let resources = build_resources_for_provider(db, request, provider)?;
        let line_items =
            estimate_resources_live(db, provider, &resources, request.live_pricing).await?;

        let infra_rows: Vec<CostRow> = line_items
            .iter()
            .zip(resources.iter())
            .map(|(item, spec)| CostRow {
                category: item.category.clone(),
                service: item
                    .description
                    .split(" — ")
                    .next()
                    .unwrap_or(&item.description)
                    .to_string(),
                description: item.description.clone(),
                unit_price: item.unit_price,
                quantity: item.quantity,
                unit: item.unit.clone(),
                usage_display: format_usage_display(spec),
                costs: PeriodBreakdown::from_monthly(item.monthly_cost),
            })
            .collect();

        let infra_totals = infra_rows
            .iter()
            .map(|r| r.costs.clone())
            .fold(PeriodBreakdown::zero(), |a, b| a.add(&b));

        let cloud_usage: Vec<_> = request
            .token_usage
            .iter()
            .filter(|u| u.cloud() == Some(provider))
            .cloned()
            .collect();
        let token_rows =
            estimate_all_token_rows(db, &cloud_usage, request.live_pricing).await?;
        let token_totals = token_rows
            .iter()
            .map(|r| r.costs.clone())
            .fold(PeriodBreakdown::zero(), |a, b| a.add(&b));

        let combined = infra_totals.add(&token_totals);

        provider_estimates.push(ProviderUiEstimate {
            provider,
            infrastructure: CostTable {
                rows: infra_rows,
                totals: infra_totals,
            },
            tokens: CostTable {
                rows: token_rows,
                totals: token_totals,
            },
            combined,
        });
    }

    let response = UiEstimateResponse {
        id: Uuid::new_v4(),
        name: request.name.clone(),
        live_pricing: request.live_pricing,
        providers: provider_estimates,
        created_at: Utc::now(),
    };

    let payload = serde_json::to_string(&response)?;
    db.save_estimate(&response.id.to_string(), &payload)?;

    Ok(response)
}

/// Infra checkboxes + any cloud used in token_usage (so Azure LLM shows when only AWS infra is selected).
fn providers_for_request(request: &EstimateRequest) -> Vec<CloudProvider> {
    let mut providers = request.providers.clone();
    for usage in &request.token_usage {
        if let Some(p) = usage.cloud() {
            if !providers.contains(&p) {
                providers.push(p);
            }
        }
    }
    providers
}

pub fn run_ui(db: &Database, request: &EstimateRequest) -> Result<UiEstimateResponse> {
    tokio::runtime::Handle::try_current()
        .map(|h| h.block_on(run_ui_async(db, request)))
        .unwrap_or_else(|_| {
            tokio::runtime::Runtime::new()
                .expect("tokio runtime")
                .block_on(run_ui_async(db, request))
        })
}

fn build_resources_for_provider(
    db: &Database,
    request: &EstimateRequest,
    provider: CloudProvider,
) -> Result<Vec<ResourceSpec>> {
    let cat = db.load_catalog()?;
    let mut resources = Vec::new();

    for input in request.resources.iter().filter(|r| r.provider == provider) {
        let (service, default_sku, unit, display_name, category) =
            resolve_catalog_resource(db, &input.catalog_id, input.provider)
                .map(|(s, sk, u)| {
                    let meta = cat
                        .categories
                        .iter()
                        .flat_map(|c| c.services.iter())
                        .find(|s| s.id == input.catalog_id);
                    let unit = reconcile_unit(&s, &u);
                    (
                        s,
                        sk,
                        unit,
                        meta.map(|m| m.name.clone())
                            .unwrap_or_else(|| input.catalog_id.clone()),
                        meta.map(|m| m.category.label().to_string())
                            .unwrap_or_else(|| "General".into()),
                    )
                })
                .with_context(|| format!("unknown catalog entry: {}", input.catalog_id))?;

        let sku = effective_sku(input.sku.as_deref(), &default_sku);
        let quantity = effective_quantity(input, &unit);
        let region_label = match &input.sub_region {
            Some(sub) if !sub.is_empty() => format!("{} · {}", sub, input.region),
            _ => input.region.clone(),
        };

        resources.push(ResourceSpec {
            service,
            sku: Some(sku),
            region: input.region.clone(),
            quantity,
            unit,
            provider: Some(provider.as_str().to_string()),
            tags: vec![],
            catalog_id: Some(input.catalog_id.clone()),
            display_name: Some(display_name),
            category: Some(category),
            sub_region: input.sub_region.clone(),
            region_label: Some(region_label),
            instance_count: input.instance_count,
            hours: input.hours,
        });
    }
    Ok(resources)
}

fn reconcile_unit(service_key: &str, stored_unit: &str) -> String {
    let inferred = crate::sync::infer_unit(service_key);
    if stored_unit == "hours" && inferred != "hours" {
        inferred.to_string()
    } else {
        stored_unit.to_string()
    }
}

fn effective_sku(input_sku: Option<&str>, default_sku: &str) -> String {
    match input_sku {
        Some(s) if !s.is_empty() && s != "default" => s.to_string(),
        _ => default_sku.to_string(),
    }
}

fn format_usage_display(spec: &ResourceSpec) -> String {
    use rust_decimal::Decimal;
    if spec.unit == "hours" {
        let n = spec.instance_count.unwrap_or(Decimal::ONE);
        let h = spec.hours.unwrap_or(spec.quantity);
        if n > Decimal::ONE {
            format!("{n} × {h} h")
        } else {
            format!("{h} h")
        }
    } else if let Some(n) = spec.instance_count {
        if n > Decimal::ONE {
            let per = spec.quantity / n;
            format!("{n} × {per} {}", spec.unit)
        } else {
            format!("{} {}", spec.quantity, spec.unit)
        }
    } else {
        format!("{} {}", spec.quantity, spec.unit)
    }
}

fn effective_quantity(input: &crate::models::UiResourceInput, unit: &str) -> rust_decimal::Decimal {
    use rust_decimal::Decimal;
    if unit == "hours" {
        let instances = input.instance_count.unwrap_or(Decimal::ONE);
        let hours = input.hours.unwrap_or(input.quantity);
        return instances * hours;
    }
    if let Some(instances) = input.instance_count {
        return instances * input.quantity;
    }
    input.quantity
}

fn legacy_from_ui(
    ui: &UiEstimateResponse,
    requirement: RequirementSet,
) -> Result<crate::models::EstimateResult> {
    use crate::models::{BillingPeriod, LineItem, PeriodCost, ProviderEstimate};

    let estimates = ui
        .providers
        .iter()
        .map(|p| {
            let line_items: Vec<LineItem> = p
                .infrastructure
                .rows
                .iter()
                .map(|r| LineItem {
                    provider: p.provider,
                    category: r.category.clone(),
                    description: r.description.clone(),
                    unit_price: r.unit_price,
                    quantity: r.quantity,
                    unit: r.unit.clone(),
                    monthly_cost: r.costs.monthly,
                })
                .collect();

            let periods = BillingPeriod::all()
                .iter()
                .map(|&period| {
                    let (infra, tokens, total) = match period {
                        BillingPeriod::Daily => (
                            p.infrastructure.totals.daily,
                            p.tokens.totals.daily,
                            p.combined.daily,
                        ),
                        BillingPeriod::Monthly => (
                            p.infrastructure.totals.monthly,
                            p.tokens.totals.monthly,
                            p.combined.monthly,
                        ),
                        BillingPeriod::Quarterly => (
                            p.infrastructure.totals.quarterly,
                            p.tokens.totals.quarterly,
                            p.combined.quarterly,
                        ),
                        BillingPeriod::HalfYearly => (
                            p.infrastructure.totals.half_yearly,
                            p.tokens.totals.half_yearly,
                            p.combined.half_yearly,
                        ),
                        BillingPeriod::Yearly => (
                            p.infrastructure.totals.yearly,
                            p.tokens.totals.yearly,
                            p.combined.yearly,
                        ),
                    };
                    PeriodCost {
                        period,
                        infrastructure: infra,
                        tokens,
                        total,
                    }
                })
                .collect();

            ProviderEstimate {
                provider: p.provider,
                line_items,
                periods,
            }
        })
        .collect();

    Ok(crate::models::EstimateResult {
        id: ui.id,
        requirement,
        estimates,
        created_at: ui.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UiResourceInput;
    use rust_decimal::Decimal;
    use tempfile::NamedTempFile;

    #[test]
    fn effective_sku_maps_placeholder() {
        assert_eq!(
            effective_sku(Some("default"), "AmazonDynamoDB"),
            "AmazonDynamoDB"
        );
        assert_eq!(effective_sku(Some("t3.medium"), "AmazonEC2"), "t3.medium");
        assert_eq!(effective_sku(None, "AmazonSNS"), "AmazonSNS");
    }

    #[test]
    fn providers_for_request_includes_token_clouds() {
        use crate::models::TokenUsageSpec;
        let req = EstimateRequest {
            name: "t".into(),
            providers: vec![CloudProvider::Aws],
            live_pricing: false,
            resources: vec![],
            token_usage: vec![TokenUsageSpec {
                model: "gpt-4o-mini".into(),
                provider: "azure".into(),
                input_tokens_per_month: 1_000_000,
                output_tokens_per_month: 250_000,
                cloud_provider: Some(CloudProvider::Azure),
                display_name: Some("GPT-4o Mini".into()),
            }],
        };
        let ps = providers_for_request(&req);
        assert!(ps.contains(&CloudProvider::Aws));
        assert!(ps.contains(&CloudProvider::Azure));
    }

    #[tokio::test]
    async fn ui_estimate_falls_back_to_cached_service_price() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        crate::sync::seed_baseline(&db).unwrap();
        db.upsert_provider_service(
            &crate::db::ProviderServiceIngest {
                catalog_id: "aws:AmazonDynamoDB".into(),
                provider: "aws".into(),
                service_key: "AmazonDynamoDB".into(),
                display_name: "DynamoDB".into(),
                category_id: "database".into(),
                unit: "million-invocations".into(),
                default_sku: "AmazonDynamoDB".into(),
                offer_code: Some("AmazonDynamoDB".into()),
                attr_key: None,
                attr_value: None,
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["eu-west-1".into()],
            },
            chrono::Utc::now(),
        )
        .unwrap();
        db.upsert_price(
            "aws",
            "AmazonDynamoDB",
            "WriteCapacityUnit-Hrs",
            "us-east-1",
            "million-invocations",
            "0.00065",
            "USD",
            chrono::Utc::now(),
        )
        .unwrap();

        let request = EstimateRequest {
            name: "test".into(),
            providers: vec![CloudProvider::Aws],
            live_pricing: false,
            resources: vec![UiResourceInput {
                catalog_id: "aws:AmazonDynamoDB".into(),
                provider: CloudProvider::Aws,
                region: "eu-west-1".into(),
                sub_region: Some("Europe".into()),
                quantity: Decimal::ONE,
                sku: Some("default".into()),
                instance_count: Some(Decimal::ONE),
                hours: None,
            }],
            token_usage: vec![],
        };

        let result = run_ui_async(&db, &request).await.unwrap();
        assert!(
            result.providers[0].infrastructure.totals.monthly > Decimal::ZERO,
            "expected non-zero price from service-level cache fallback"
        );
    }

    #[tokio::test]
    async fn ui_estimate_with_selected_provider() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        crate::sync::seed_baseline(&db).unwrap();
        // Seed a minimal provider catalog for offline estimate
        db.upsert_provider_service(
            &crate::db::ProviderServiceIngest {
                catalog_id: "aws:AmazonEC2".into(),
                provider: "aws".into(),
                service_key: "AmazonEC2".into(),
                display_name: "Amazon EC2".into(),
                category_id: "compute".into(),
                unit: "hours".into(),
                default_sku: "t3.medium".into(),
                offer_code: Some("AmazonEC2".into()),
                attr_key: Some("instanceType".into()),
                attr_value: Some("t3.medium".into()),
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["us-east-1".into()],
            },
            chrono::Utc::now(),
        )
        .unwrap();
        db.upsert_price(
            "aws",
            "AmazonEC2",
            "t3.medium",
            "us-east-1",
            "hours",
            "0.0416",
            "USD",
            chrono::Utc::now(),
        )
        .unwrap();

        let request = EstimateRequest {
            name: "test".into(),
            providers: vec![CloudProvider::Aws],
            live_pricing: false,
            resources: vec![UiResourceInput {
                catalog_id: "aws:AmazonEC2".into(),
                provider: CloudProvider::Aws,
                region: "us-east-1".into(),
                sub_region: None,
                quantity: Decimal::from(730u32),
                sku: Some("t3.medium".into()),
                instance_count: Some(Decimal::ONE),
                hours: Some(Decimal::from(730u32)),
            }],
            token_usage: vec![],
        };

        let result = run_ui_async(&db, &request).await.unwrap();
        assert_eq!(result.providers.len(), 1);
        assert!(result.providers[0].infrastructure.totals.monthly > Decimal::ZERO);
    }
}
