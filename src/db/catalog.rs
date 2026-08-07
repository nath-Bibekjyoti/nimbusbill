use crate::catalog::{
    CatalogCategory, CatalogProviderEntry, CatalogResponse, CatalogService, LlmCatalogEntry,
};
use crate::models::CloudProvider;
use anyhow::Result;
use rusqlite::{params, Connection};

use super::Database;

/// One row to fetch from a cloud pricing API (from provider_services + regions).
#[derive(Debug, Clone)]
pub struct PriceTarget {
    pub service_key: String,
    pub sku: String,
    pub region: String,
    pub unit: String,
    pub offer_code: Option<String>,
    pub attr_key: Option<String>,
    pub attr_value: Option<String>,
    pub billing_service_id: Option<String>,
    pub sku_description_hint: Option<String>,
}

impl Database {
    pub fn catalog_is_empty(&self) -> Result<bool> {
        let conn = self.conn.lock().expect("db lock");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM catalog_services",
            [],
            |row| row.get(0),
        )?;
        Ok(count == 0)
    }

    pub fn load_catalog(&self) -> Result<CatalogResponse> {
        let conn = self.conn.lock().expect("db lock");
        load_catalog_from_conn(&conn)
    }

    pub fn resolve_catalog_resource(
        &self,
        catalog_id: &str,
        provider: CloudProvider,
    ) -> Result<Option<(String, String, String)>> {
        if let Some(found) = self.resolve_provider_service(catalog_id, provider.as_str())? {
            return Ok(Some(found));
        }
        let conn = self.conn.lock().expect("db lock");
        let provider_key = provider.as_str();
        let mut stmt = conn.prepare(
            "SELECT e.service_key, e.default_sku, s.unit
             FROM catalog_provider_entries e
             JOIN catalog_services s ON s.id = e.service_id
             WHERE e.service_id = ?1 AND e.provider = ?2",
        )?;
        let mut rows = stmt.query(params![catalog_id, provider_key])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_llm_model(
        &self,
        id: &str,
        label: &str,
        provider: &str,
        regions: Option<&[String]>,
    ) -> Result<()> {
        let regions_json = regions
            .filter(|r| !r.is_empty())
            .map(|r| serde_json::to_string(r).unwrap_or_default());
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO llm_models (id, label, provider, regions) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               label = excluded.label,
               provider = excluded.provider,
               regions = excluded.regions",
            params![id, label, provider, regions_json],
        )?;
        Ok(())
    }

    pub fn llm_model_count(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("db lock");
        conn.query_row("SELECT COUNT(*) FROM llm_models", [], |r| r.get(0))
            .map_err(Into::into)
    }

    pub fn upsert_custom_llm(
        &self,
        id: &str,
        label: &str,
        provider: &str,
        input_per_mtok: &str,
        output_per_mtok: &str,
    ) -> Result<()> {
        self.upsert_llm_model(id, label, provider, None)?;
        self.upsert_token_price(
            provider,
            id,
            input_per_mtok,
            output_per_mtok,
            "USD",
            chrono::Utc::now(),
        )
    }

    pub fn ensure_bootstrap(&self) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        ensure_bootstrap_conn(&conn)
    }

    pub fn upsert_service(
        &self,
        id: &str,
        category_id: &str,
        name: &str,
        unit: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO catalog_services (id, category_id, name, unit) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               category_id = excluded.category_id,
               name = excluded.name,
               unit = excluded.unit",
            params![id, category_id, name, unit],
        )?;
        Ok(())
    }

    pub fn replace_provider_entry(
        &self,
        service_id: &str,
        provider: &str,
        service_key: &str,
        default_sku: &str,
        regions: &[String],
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO catalog_provider_entries (service_id, provider, service_key, default_sku)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(service_id, provider) DO UPDATE SET
               service_key = excluded.service_key,
               default_sku = excluded.default_sku",
            params![service_id, provider, service_key, default_sku],
        )?;
        let entry_id: i64 = conn.query_row(
            "SELECT id FROM catalog_provider_entries WHERE service_id = ?1 AND provider = ?2",
            params![service_id, provider],
            |row| row.get(0),
        )?;
        conn.execute(
            "DELETE FROM catalog_regions WHERE entry_id = ?1",
            params![entry_id],
        )?;
        for region in regions {
            conn.execute(
                "INSERT INTO catalog_regions (entry_id, region) VALUES (?1, ?2)",
                params![entry_id, region],
            )?;
        }
        Ok(())
    }
}

pub fn load_catalog_from_conn(conn: &Connection) -> Result<CatalogResponse> {
    let ps_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM provider_services",
        [],
        |row| row.get(0),
    )?;
    if ps_count > 0 {
        return load_catalog_from_provider_services(conn);
    }
    load_catalog_legacy(conn)
}

fn load_catalog_from_provider_services(conn: &Connection) -> Result<CatalogResponse> {
    let mut cat_stmt = conn.prepare(
        "SELECT id, label FROM catalog_categories ORDER BY sort_order, id",
    )?;
    let categories: Vec<(String, String)> = cat_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut svc_stmt = conn.prepare(
        "SELECT catalog_id, category_id, display_name, unit, provider, service_key, default_sku
         FROM provider_services ORDER BY display_name",
    )?;
    let services: Vec<(String, String, String, String, String, String, String)> = svc_stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut region_stmt =
        conn.prepare("SELECT catalog_id, region FROM provider_service_regions ORDER BY region")?;
    let regions: Vec<(String, String)> = region_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let llm_models = load_llm_models(conn)?;

    let mut response_categories = Vec::new();
    for (cat_id, cat_label) in categories {
        let cat_services: Vec<CatalogService> = services
            .iter()
            .filter(|(_, cid, _, _, _, _, _)| cid == &cat_id)
            .filter(|(_, _, _, _, _, service_key, _)| {
                !crate::sync::is_llm_token_service(service_key)
            })
            .filter_map(|(catalog_id, _, name, unit, provider, service_key, default_sku)| {
                let cloud = match provider.as_str() {
                    "aws" => CloudProvider::Aws,
                    "azure" => CloudProvider::Azure,
                    "gcp" => CloudProvider::Gcp,
                    _ => return None,
                };
                let entry_regions: Vec<String> = regions
                    .iter()
                    .filter(|(cid, _)| cid == catalog_id)
                    .map(|(_, r)| r.clone())
                    .collect();
                Some(CatalogService {
                    id: catalog_id.clone(),
                    name: name.clone(),
                    category: crate::catalog::ServiceCategory::from_id(&cat_id),
                    unit: unit.clone(),
                    providers: vec![CatalogProviderEntry {
                        provider: cloud,
                        service_key: service_key.clone(),
                        default_sku: default_sku.clone(),
                        regions: entry_regions,
                    }],
                })
            })
            .collect();

        if !cat_services.is_empty() {
            response_categories.push(CatalogCategory {
                id: cat_id,
                label: cat_label,
                services: cat_services,
            });
        }
    }

    Ok(CatalogResponse {
        categories: response_categories,
        llm_models,
    })
}

fn load_catalog_legacy(conn: &Connection) -> Result<CatalogResponse> {
    let mut cat_stmt = conn.prepare(
        "SELECT id, label FROM catalog_categories ORDER BY sort_order, id",
    )?;
    let categories: Vec<(String, String)> = cat_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut svc_stmt = conn.prepare(
        "SELECT id, category_id, name, unit FROM catalog_services ORDER BY name",
    )?;
    let services: Vec<(String, String, String, String)> = svc_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut entry_stmt = conn.prepare(
        "SELECT id, service_id, provider, service_key, default_sku
         FROM catalog_provider_entries",
    )?;
    let entries: Vec<(i64, String, String, String, String)> = entry_stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut region_stmt = conn.prepare("SELECT entry_id, region FROM catalog_regions")?;
    let regions: Vec<(i64, String)> = region_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let llm_models = load_llm_models(conn)?;

    let mut response_categories = Vec::new();
    for (cat_id, cat_label) in categories {
        let cat_services: Vec<CatalogService> = services
            .iter()
            .filter(|(_, cid, _, _)| cid == &cat_id)
            .map(|(sid, _, name, unit)| {
                let providers: Vec<CatalogProviderEntry> = entries
                    .iter()
                    .filter(|(_, esid, _, _, _)| esid == sid)
                    .filter_map(|(eid, _, provider, service_key, default_sku)| {
                        let provider = match provider.as_str() {
                            "aws" => CloudProvider::Aws,
                            "azure" => CloudProvider::Azure,
                            "gcp" => CloudProvider::Gcp,
                            _ => return None,
                        };
                        let entry_regions: Vec<String> = regions
                            .iter()
                            .filter(|(rid, _)| rid == eid)
                            .map(|(_, r)| r.clone())
                            .collect();
                        Some(CatalogProviderEntry {
                            provider,
                            service_key: service_key.clone(),
                            default_sku: default_sku.clone(),
                            regions: entry_regions,
                        })
                    })
                    .collect();
                CatalogService {
                    id: sid.clone(),
                    name: name.clone(),
                    category: crate::catalog::ServiceCategory::from_id(&cat_id),
                    unit: unit.clone(),
                    providers,
                }
            })
            .collect();

        if !cat_services.is_empty() {
            response_categories.push(CatalogCategory {
                id: cat_id,
                label: cat_label,
                services: cat_services,
            });
        }
    }

    Ok(CatalogResponse {
        categories: response_categories,
        llm_models,
    })
}

fn load_llm_models(conn: &Connection) -> Result<Vec<LlmCatalogEntry>> {
    let mut llm_stmt = conn.prepare(
        "SELECT m.id, m.label, m.provider, t.input_per_mtok, t.output_per_mtok, m.regions
         FROM llm_models m
         LEFT JOIN token_price_cache t ON t.provider = m.provider AND t.model = m.id
         WHERE m.provider IN ('aws', 'azure', 'gcp')
         ORDER BY m.provider, m.label",
    )?;
    let models = llm_stmt
        .query_map([], |row| {
            let regions_raw: Option<String> = row.get(5)?;
            let regions = regions_raw
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            Ok(LlmCatalogEntry {
                id: row.get(0)?,
                label: row.get(1)?,
                provider: row.get(2)?,
                input_per_mtok: row.get(3)?,
                output_per_mtok: row.get(4)?,
                regions,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(models)
}

pub fn ensure_bootstrap_conn(conn: &Connection) -> Result<()> {
    for (id, label, sort) in CATEGORY_SEED {
        conn.execute(
            "INSERT INTO catalog_categories (id, label, sort_order) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET label = excluded.label, sort_order = excluded.sort_order",
            params![id, label, sort],
        )?;
    }

    Ok(())
}

/// Legacy name — only bootstraps categories; catalog rows come from sync.
pub fn seed_catalog(conn: &Connection) -> Result<()> {
    ensure_bootstrap_conn(conn)
}

const CATEGORY_SEED: &[(&str, &str, i32)] = &[
    ("compute", "Compute", 0),
    ("storage", "Storage", 1),
    ("database", "Database", 2),
    ("messaging", "Messaging", 3),
    ("networking", "Networking", 4),
    ("security", "Security", 5),
    ("ai_ml", "AI / ML", 6),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::provider_catalog::ProviderServiceIngest;
    use crate::db::schema::migrate;
    use chrono::Utc;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    #[test]
    fn bootstrap_and_load_includes_llm() {
        let file = NamedTempFile::new().unwrap();
        let conn = Connection::open(file.path()).unwrap();
        migrate(&conn).unwrap();
        ensure_bootstrap_conn(&conn).unwrap();
        let cat = load_catalog_from_conn(&conn).unwrap();
        assert!(cat.llm_models.is_empty());
    }

    #[test]
    fn provider_catalog_load_and_search() {
        let file = NamedTempFile::new().unwrap();
        let db = crate::db::Database::open(file.path()).unwrap();
        db.upsert_provider_service(
            &ProviderServiceIngest {
                catalog_id: "aws:AmazonElastiCache".into(),
                provider: "aws".into(),
                service_key: "AmazonElastiCache".into(),
                display_name: "Amazon ElastiCache".into(),
                category_id: "database".into(),
                unit: "hours".into(),
                default_sku: "cache.t3.medium".into(),
                offer_code: Some("AmazonElastiCache".into()),
                attr_key: None,
                attr_value: None,
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["us-east-1".into(), "eu-west-1".into()],
            },
            Utc::now(),
        )
        .unwrap();
        db.upsert_provider_service(
            &ProviderServiceIngest {
                catalog_id: "aws:AmazonMSK".into(),
                provider: "aws".into(),
                service_key: "AmazonMSK".into(),
                display_name: "Amazon MSK".into(),
                category_id: "messaging".into(),
                unit: "hours".into(),
                default_sku: "kafka.m5.large".into(),
                offer_code: Some("AmazonMSK".into()),
                attr_key: None,
                attr_value: None,
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["us-east-1".into()],
            },
            Utc::now(),
        )
        .unwrap();

        let cat = db.load_catalog().unwrap();
        let ids: Vec<String> = cat
            .categories
            .iter()
            .flat_map(|c| c.services.iter().map(|s| s.id.clone()))
            .collect();
        assert!(ids.iter().any(|id| id == "aws:AmazonElastiCache"));
        assert!(ids.iter().any(|id| id == "aws:AmazonMSK"));

        let targets = db.list_price_targets("aws").unwrap();
        assert!(targets.iter().any(|t| t.service_key == "AmazonElastiCache"));
        assert!(targets.iter().any(|t| t.service_key == "AmazonMSK"));
    }
}
