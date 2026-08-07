use crate::db::Database;
use crate::db::PriceTarget;
use super::catalog::azure as catalog_azure;
use anyhow::{Context, Result};

pub async fn fetch_live(db: &Database, service: &str, sku: &str, region: &str) -> Result<String> {
    let service_name = db
        .service_attr_value("azure", service)?
        .unwrap_or_else(|| service.to_string());
    let target = PriceTarget {
        service_key: service.into(),
        sku: sku.into(),
        region: region.into(),
        unit: "unit".into(),
        offer_code: None,
        attr_key: Some("serviceName".into()),
        attr_value: Some(service_name),
        billing_service_id: None,
        sku_description_hint: Some(sku.into()),
    };
    fetch_for_target(&target).await
}

async fn fetch_for_target(target: &PriceTarget) -> Result<String> {
    let service_name = target
        .attr_value
        .as_deref()
        .context("azure target missing serviceName")?;
    catalog_azure::fetch_retail_price(service_name, &target.sku, &target.region, true).await
}
