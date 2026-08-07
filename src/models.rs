use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudProvider {
    Aws,
    Azure,
    Gcp,
}

impl CloudProvider {
    pub fn all() -> &'static [CloudProvider] {
        &[CloudProvider::Aws, CloudProvider::Azure, CloudProvider::Gcp]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CloudProvider::Aws => "aws",
            CloudProvider::Azure => "azure",
            CloudProvider::Gcp => "gcp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CloudProvider::Aws => "AWS",
            CloudProvider::Azure => "Azure",
            CloudProvider::Gcp => "GCP",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "aws" => Some(CloudProvider::Aws),
            "azure" => Some(CloudProvider::Azure),
            "gcp" => Some(CloudProvider::Gcp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingPeriod {
    Daily,
    Monthly,
    Quarterly,
    HalfYearly,
    Yearly,
}

impl BillingPeriod {
    pub fn months(self) -> Decimal {
        match self {
            BillingPeriod::Daily => Decimal::new(1, 2) / Decimal::from(30u32), // ~1/30 month
            BillingPeriod::Monthly => Decimal::ONE,
            BillingPeriod::Quarterly => Decimal::from(3u32),
            BillingPeriod::HalfYearly => Decimal::from(6u32),
            BillingPeriod::Yearly => Decimal::from(12u32),
        }
    }

    pub fn all() -> &'static [BillingPeriod] {
        &[
            BillingPeriod::Daily,
            BillingPeriod::Monthly,
            BillingPeriod::Quarterly,
            BillingPeriod::HalfYearly,
            BillingPeriod::Yearly,
        ]
    }

    pub fn column_label(self) -> &'static str {
        match self {
            BillingPeriod::Daily => "Daily",
            BillingPeriod::Monthly => "Monthly",
            BillingPeriod::Quarterly => "Quarterly",
            BillingPeriod::HalfYearly => "Half-Yearly",
            BillingPeriod::Yearly => "Yearly",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSpec {
    pub service: String,
    pub sku: Option<String>,
    pub region: String,
    pub quantity: Decimal,
    pub unit: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub tags: Vec<(String, String)>,
    #[serde(default)]
    pub catalog_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub sub_region: Option<String>,
    #[serde(default)]
    pub region_label: Option<String>,
    #[serde(default)]
    pub instance_count: Option<Decimal>,
    #[serde(default)]
    pub hours: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageSpec {
    pub model: String,
    /// Pricing lookup key in `token_price_cache` (matches cloud for managed LLMs).
    pub provider: String,
    pub input_tokens_per_month: u64,
    pub output_tokens_per_month: u64,
    #[serde(default)]
    pub cloud_provider: Option<CloudProvider>,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl TokenUsageSpec {
    pub fn cloud(&self) -> Option<CloudProvider> {
        self.cloud_provider.or_else(|| CloudProvider::parse(&self.provider))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementSet {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub resources: Vec<ResourceSpec>,
    #[serde(default)]
    pub token_usage: Vec<TokenUsageSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiImportResource {
    pub key: String,
    pub catalog_id: String,
    pub provider: String,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_region: Option<String>,
    pub sku: String,
    pub instance_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<u64>,
    pub quantity: u64,
    pub name: String,
    pub category: String,
    pub category_id: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiImportResponse {
    pub name: String,
    pub providers: Vec<String>,
    pub live_pricing: bool,
    pub resources: Vec<UiImportResource>,
    pub token_usage: Vec<TokenUsageSpec>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiResourceInput {
    pub catalog_id: String,
    pub provider: CloudProvider,
    pub region: String,
    #[serde(default)]
    pub sub_region: Option<String>,
    pub quantity: Decimal,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub instance_count: Option<Decimal>,
    #[serde(default)]
    pub hours: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateRequest {
    #[serde(default = "default_name")]
    pub name: String,
    pub providers: Vec<CloudProvider>,
    #[serde(default)]
    pub live_pricing: bool,
    #[serde(default)]
    pub resources: Vec<UiResourceInput>,
    #[serde(default)]
    pub token_usage: Vec<TokenUsageSpec>,
}

fn default_name() -> String {
    "estimate".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodBreakdown {
    pub daily: Decimal,
    pub monthly: Decimal,
    pub quarterly: Decimal,
    pub half_yearly: Decimal,
    pub yearly: Decimal,
}

impl PeriodBreakdown {
    pub fn from_monthly(monthly: Decimal) -> Self {
        Self {
            daily: monthly / Decimal::from(30u32),
            monthly,
            quarterly: monthly * Decimal::from(3u32),
            half_yearly: monthly * Decimal::from(6u32),
            yearly: monthly * Decimal::from(12u32),
        }
    }

    pub fn zero() -> Self {
        Self::from_monthly(Decimal::ZERO)
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            daily: self.daily + other.daily,
            monthly: self.monthly + other.monthly,
            quarterly: self.quarterly + other.quarterly,
            half_yearly: self.half_yearly + other.half_yearly,
            yearly: self.yearly + other.yearly,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRow {
    pub category: String,
    pub service: String,
    pub description: String,
    pub unit_price: Decimal,
    pub quantity: Decimal,
    pub unit: String,
    /// Human-readable usage for the results table (e.g. "1 × 730 h" not raw billable hours).
    pub usage_display: String,
    pub costs: PeriodBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostTable {
    pub rows: Vec<CostRow>,
    pub totals: PeriodBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUiEstimate {
    pub provider: CloudProvider,
    pub infrastructure: CostTable,
    pub tokens: CostTable,
    pub combined: PeriodBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiEstimateResponse {
    pub id: Uuid,
    pub name: String,
    pub live_pricing: bool,
    pub providers: Vec<ProviderUiEstimate>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineItem {
    pub provider: CloudProvider,
    pub category: String,
    pub description: String,
    pub unit_price: Decimal,
    pub quantity: Decimal,
    pub unit: String,
    pub monthly_cost: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodCost {
    pub period: BillingPeriod,
    pub infrastructure: Decimal,
    pub tokens: Decimal,
    pub total: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEstimate {
    pub provider: CloudProvider,
    pub line_items: Vec<LineItem>,
    pub periods: Vec<PeriodCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateResult {
    pub id: Uuid,
    pub requirement: RequirementSet,
    pub estimates: Vec<ProviderEstimate>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub interval_secs: u64,
    pub providers: Vec<CloudProvider>,
    /// Parallel HTTP workers; 0 = auto (CPU / NIMUSBILL_CONCURRENCY).
    #[serde(default)]
    pub concurrency: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            interval_secs: 86_400,
            providers: CloudProvider::all().to_vec(),
            concurrency: 0,
        }
    }
}
