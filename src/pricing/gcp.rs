use super::{PricingProvider, estimate_resource_from_cache};
use crate::db::Database;
use crate::models::{CloudProvider, LineItem, ResourceSpec};
use anyhow::Result;

pub struct GcpPricing;

impl PricingProvider for GcpPricing {
    fn provider(&self) -> CloudProvider {
        CloudProvider::Gcp
    }

    fn estimate_resource(&self, db: &Database, spec: &ResourceSpec) -> Result<LineItem> {
        estimate_resource_from_cache(
            db,
            CloudProvider::Gcp,
            spec,
            "gcp",
        )
    }
}
