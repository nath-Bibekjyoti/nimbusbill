use crate::db::Database;
use crate::db::PriceTarget;
use super::catalog::aws as catalog_aws;
use super::catalog::specs;
use anyhow::Result;

pub async fn fetch_live(db: &Database, service: &str, sku: &str, region: &str) -> Result<(String, String)> {
    let meta = db.provider_service_by_key("aws", service)?;
    let default_sku = meta
        .as_ref()
        .map(|m| m.default_sku.as_str())
        .unwrap_or(service);
    let effective = match sku {
        "" | "default" => default_sku.to_string(),
        s if s == service => default_sku.to_string(),
        s => s.to_string(),
    };
    let target = PriceTarget {
        service_key: service.into(),
        sku: effective.clone(),
        region: region.into(),
        unit: meta
            .as_ref()
            .map(|m| m.unit.clone())
            .unwrap_or_else(|| "unit".into()),
        offer_code: meta
            .as_ref()
            .and_then(|m| m.offer_code.clone())
            .or_else(|| Some(service.into())),
        attr_key: meta
            .as_ref()
            .and_then(|m| m.attr_key.clone())
            .or_else(|| {
                specs::aws_attr_key(
                    meta.as_ref()
                        .and_then(|m| m.offer_code.as_deref())
                        .unwrap_or(service),
                )
                .map(str::to_string)
            }),
        attr_value: meta
            .as_ref()
            .and_then(|m| m.attr_key.as_ref().map(|_| effective)),
        billing_service_id: None,
        sku_description_hint: None,
    };
    catalog_aws::fetch_price_for_target(&target, true).await
}
