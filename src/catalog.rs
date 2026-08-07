use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCategory {
    Compute,
    Storage,
    Database,
    Messaging,
    Networking,
    Security,
    AiMl,
}

impl ServiceCategory {
    pub fn all() -> &'static [ServiceCategory] {
        &[
            ServiceCategory::Compute,
            ServiceCategory::Storage,
            ServiceCategory::Database,
            ServiceCategory::Messaging,
            ServiceCategory::Networking,
            ServiceCategory::Security,
            ServiceCategory::AiMl,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            ServiceCategory::Compute => "Compute",
            ServiceCategory::Storage => "Storage",
            ServiceCategory::Database => "Database",
            ServiceCategory::Messaging => "Messaging",
            ServiceCategory::Networking => "Networking",
            ServiceCategory::Security => "Security",
            ServiceCategory::AiMl => "AI / ML",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ServiceCategory::Compute => "compute",
            ServiceCategory::Storage => "storage",
            ServiceCategory::Database => "database",
            ServiceCategory::Messaging => "messaging",
            ServiceCategory::Networking => "networking",
            ServiceCategory::Security => "security",
            ServiceCategory::AiMl => "ai_ml",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "storage" => Self::Storage,
            "database" => Self::Database,
            "messaging" => Self::Messaging,
            "networking" => Self::Networking,
            "security" => Self::Security,
            "ai_ml" => Self::AiMl,
            _ => Self::Compute,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProviderEntry {
    pub provider: crate::models::CloudProvider,
    pub service_key: String,
    pub default_sku: String,
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogService {
    pub id: String,
    pub name: String,
    pub category: ServiceCategory,
    pub unit: String,
    pub providers: Vec<CatalogProviderEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogResponse {
    pub categories: Vec<CatalogCategory>,
    pub llm_models: Vec<LlmCatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCategory {
    pub id: String,
    pub label: String,
    pub services: Vec<CatalogService>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCatalogEntry {
    pub id: String,
    pub label: String,
    /// Cloud provider (`aws`, `azure`, `gcp`) — token rates are per cloud managed service.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_per_mtok: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_per_mtok: Option<String>,
    /// AWS/Azure/GCP regions where this model is available (empty = all regions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddLlmModelRequest {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub input_per_mtok: String,
    pub output_per_mtok: String,
}

pub fn resolve_catalog_resource(
    db: &crate::db::Database,
    catalog_id: &str,
    provider: crate::models::CloudProvider,
) -> Option<(String, String, String)> {
    db.resolve_catalog_resource(catalog_id, provider)
        .ok()
        .flatten()
}
