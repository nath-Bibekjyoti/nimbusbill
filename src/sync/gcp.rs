use crate::db::Database;
use crate::db::PriceTarget;
use super::gcp_billing;
use anyhow::{Context, Result};

pub async fn fetch_live(db: &Database, service: &str, sku: &str, region: &str) -> Result<String> {
    let billing_id = db
        .service_billing_id("gcp", service)?
        .context("gcp service missing billing_service_id")?;
    let target = PriceTarget {
        service_key: service.into(),
        sku: sku.into(),
        region: region.into(),
        unit: "unit".into(),
        offer_code: None,
        attr_key: None,
        attr_value: None,
        billing_service_id: Some(billing_id),
        sku_description_hint: Some(sku.into()),
    };
    fetch_for_target(&target).await
}

async fn fetch_for_target(target: &PriceTarget) -> Result<String> {
    let billing_id = target
        .billing_service_id
        .as_deref()
        .context("gcp target missing billing_service_id")?;
    let hint = target
        .sku_description_hint
        .as_deref()
        .unwrap_or(&target.sku);
    let skus = gcp_billing::list_skus_live(billing_id).await?;
    let sku = gcp_billing::find_sku(&skus, hint)
        .or_else(|| skus.first())
        .context("gcp sku not found")?;
    gcp_billing::sku_price(sku).context("gcp sku has no price")
}
