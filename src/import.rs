use crate::catalog::ServiceCategory;
use crate::db::Database;
use crate::input::{ParsedUpload, parse_upload_bytes};
use crate::models::{
    CloudProvider, EstimateRequest, RequirementSet, ResourceSpec, UiImportResource,
    UiImportResponse, UiResourceInput,
};
use anyhow::{Context, Result, bail};
use rust_decimal::Decimal;

pub fn import_bytes(db: &Database, filename: &str, bytes: &[u8]) -> Result<UiImportResponse> {
    let parsed = parse_upload_bytes(filename, bytes)?;
    to_ui_import(db, parsed)
}

pub fn to_ui_import(db: &Database, parsed: ParsedUpload) -> Result<UiImportResponse> {
    match parsed {
        ParsedUpload::Estimate(req) => estimate_to_ui(db, req),
        ParsedUpload::Requirement(set) => requirement_to_ui(db, set),
    }
}

fn estimate_to_ui(db: &Database, req: EstimateRequest) -> Result<UiImportResponse> {
    let catalog = db.load_catalog()?;
    let mut warnings = Vec::new();
    let mut resources = Vec::new();
    for input in &req.resources {
        match ui_input_to_import(db, &catalog, input) {
            Ok(r) => resources.push(r),
            Err(e) => warnings.push(format!("{}: {e}", input.catalog_id)),
        }
    }
    Ok(UiImportResponse {
        name: req.name,
        providers: req.providers.iter().map(|p| p.as_str().to_string()).collect(),
        live_pricing: req.live_pricing,
        resources,
        token_usage: req.token_usage,
        warnings,
    })
}

fn requirement_to_ui(db: &Database, set: RequirementSet) -> Result<UiImportResponse> {
    let catalog = db.load_catalog()?;
    let mut warnings = Vec::new();
    let mut resources = Vec::new();
    let mut providers = std::collections::BTreeSet::new();

    for spec in &set.resources {
        let provider = spec
            .provider
            .as_deref()
            .or_else(|| infer_provider_from_catalog_id(spec.catalog_id.as_deref()))
            .unwrap_or("aws");
        providers.insert(provider.to_string());
        match spec_to_import(db, &catalog, spec, provider) {
            Ok(r) => resources.push(r),
            Err(e) => warnings.push(format!(
                "{}: {e}",
                spec.catalog_id.as_deref().unwrap_or(&spec.service)
            )),
        }
    }

    for t in &set.token_usage {
        if let Some(p) = t.cloud() {
            providers.insert(p.as_str().to_string());
        } else if let Some(p) = CloudProvider::parse(&t.provider) {
            providers.insert(p.as_str().to_string());
        }
    }

    Ok(UiImportResponse {
        name: set.name,
        providers: providers.into_iter().collect(),
        live_pricing: false,
        resources,
        token_usage: set.token_usage,
        warnings,
    })
}

fn ui_input_to_import(
    db: &Database,
    catalog: &crate::catalog::CatalogResponse,
    input: &UiResourceInput,
) -> Result<UiImportResource> {
    let provider = input.provider.as_str();
    let (service_key, default_sku, resolved_unit) = db
        .resolve_provider_service(&input.catalog_id, provider)?
        .with_context(|| format!("unknown catalog entry {}", input.catalog_id))?;
    let sku = input
        .sku
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or(default_sku);
    let (name, category, category_id) = catalog_meta(catalog, &input.catalog_id, &service_key);
    let instance_count = decimal_to_u64(input.instance_count.unwrap_or(Decimal::ONE));
    let hours = input.hours.map(decimal_to_u64);
    let quantity = if resolved_unit == "hours" {
        instance_count * hours.unwrap_or(decimal_to_u64(input.quantity))
    } else {
        decimal_to_u64(input.quantity)
    };
    let key = format!("{}-{}-{}-{}", input.catalog_id, provider, input.region, sku);

    Ok(UiImportResource {
        key,
        catalog_id: input.catalog_id.clone(),
        provider: provider.to_string(),
        region: input.region.clone(),
        sub_region: input.sub_region.clone(),
        sku,
        instance_count,
        hours,
        quantity,
        name,
        category,
        category_id,
        unit: resolved_unit,
    })
}

fn spec_to_import(
    db: &Database,
    catalog: &crate::catalog::CatalogResponse,
    spec: &ResourceSpec,
    provider: &str,
) -> Result<UiImportResource> {
    let catalog_id = resolve_catalog_id(db, provider, spec)?;
    let (service_key, default_sku, resolved_unit) = db
        .resolve_provider_service(&catalog_id, provider)?
        .with_context(|| format!("unknown catalog entry {catalog_id}"))?;
    let sku = spec
        .sku
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or(default_sku);
    let (name, category, category_id) = catalog_meta(catalog, &catalog_id, &service_key);
    let unit = if spec.unit.is_empty() || spec.unit == "units" {
        resolved_unit
    } else {
        spec.unit.clone()
    };
    let instance_count = decimal_to_u64(spec.instance_count.unwrap_or(Decimal::ONE));
    let hours = spec.hours.map(decimal_to_u64);
    let quantity = if unit == "hours" {
        instance_count * hours.unwrap_or(decimal_to_u64(spec.quantity))
    } else {
        decimal_to_u64(spec.quantity)
    };
    let region = spec.region.clone();
    let sub_region = spec.sub_region.clone();
    let key = format!("{catalog_id}-{provider}-{region}-{sku}");

    Ok(UiImportResource {
        key,
        catalog_id,
        provider: provider.to_string(),
        region,
        sub_region,
        sku,
        instance_count,
        hours,
        quantity,
        name,
        category,
        category_id,
        unit,
    })
}

fn resolve_catalog_id(db: &Database, provider: &str, spec: &ResourceSpec) -> Result<String> {
    if let Some(id) = spec.catalog_id.as_ref().filter(|s| !s.is_empty()) {
        if id.contains(':') {
            return Ok(id.clone());
        }
        return Ok(format!("{provider}:{id}"));
    }
    let hits = db.search_catalog(&spec.service, Some(provider), 5)?;
    if let Some(hit) = hits.into_iter().find(|h| h.provider == provider) {
        return Ok(hit.catalog_id);
    }
    bail!(
        "could not resolve service '{}' for {provider} — set catalog_id",
        spec.service
    )
}

fn infer_provider_from_catalog_id(catalog_id: Option<&str>) -> Option<&str> {
    let id = catalog_id?;
    id.split(':').next().filter(|p| matches!(*p, "aws" | "azure" | "gcp"))
}

fn catalog_meta(
    catalog: &crate::catalog::CatalogResponse,
    catalog_id: &str,
    service_key: &str,
) -> (String, String, String) {
    for cat in &catalog.categories {
        for svc in &cat.services {
            if svc.id == catalog_id {
                return (svc.name.clone(), cat.label.clone(), cat.id.clone());
            }
        }
    }
    (
        service_key.to_string(),
        "General".into(),
        ServiceCategory::Compute.id().to_string(),
    )
}

fn decimal_to_u64(d: Decimal) -> u64 {
    use rust_decimal::prelude::*;
    d.to_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::NamedTempFile;

    #[test]
    fn import_json_requirement_resolves_catalog_id() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
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
                attr_key: None,
                attr_value: None,
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["us-east-1".into()],
            },
            Utc::now(),
        )
        .unwrap();

        let json = r#"{
            "name": "test",
            "resources": [{
                "provider": "aws",
                "catalog_id": "aws:AmazonEC2",
                "service": "AmazonEC2",
                "region": "us-east-1",
                "quantity": "730",
                "unit": "hours",
                "sku": "t3.medium"
            }]
        }"#;
        let out = import_bytes(&db, "workload.json", json.as_bytes()).unwrap();
        assert_eq!(out.resources.len(), 1);
        assert_eq!(out.resources[0].catalog_id, "aws:AmazonEC2");
    }
}
