use super::Database;
use crate::db::catalog::PriceTarget;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Row ingested from a cloud provider catalog API.
#[derive(Debug, Clone)]
pub struct ProviderServiceIngest {
    pub catalog_id: String,
    pub provider: String,
    pub service_key: String,
    pub display_name: String,
    pub category_id: String,
    pub unit: String,
    pub default_sku: String,
    pub offer_code: Option<String>,
    pub attr_key: Option<String>,
    pub attr_value: Option<String>,
    pub billing_service_id: Option<String>,
    pub sku_description_hint: Option<String>,
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSearchHit {
    pub catalog_id: String,
    pub provider: String,
    pub service_key: String,
    pub display_name: String,
    pub category_id: String,
    pub category_label: String,
    pub unit: String,
    pub default_sku: String,
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusEntry {
    pub provider: String,
    pub status: String,
    pub detail: Option<String>,
    pub synced_at: String,
}

impl Database {
    pub fn provider_catalog_count(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("db lock");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_services",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn catalog_regions(&self, catalog_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("db lock");
        load_regions(&conn, catalog_id)
    }

    /// Distinct regions already stored for a provider (used when a live API fetch fails mid-sync).
    pub fn provider_region_union(&self, provider: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT DISTINCT r.region
             FROM provider_service_regions r
             JOIN provider_services ps ON ps.catalog_id = r.catalog_id
             WHERE ps.provider = ?1
             ORDER BY r.region",
        )?;
        let rows = stmt.query_map(params![provider], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_provider_service(
        &self,
        row: &ProviderServiceIngest,
        synced_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        let regions = Self::coalesce_regions(&conn, &row.catalog_id, &row.regions)?;
        conn.execute(
            "INSERT INTO provider_services (
                catalog_id, provider, service_key, display_name, category_id, unit,
                default_sku, offer_code, attr_key, attr_value,
                billing_service_id, sku_description_hint, synced_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(catalog_id) DO UPDATE SET
                display_name = excluded.display_name,
                category_id = excluded.category_id,
                unit = excluded.unit,
                default_sku = excluded.default_sku,
                offer_code = excluded.offer_code,
                attr_key = excluded.attr_key,
                attr_value = excluded.attr_value,
                billing_service_id = excluded.billing_service_id,
                sku_description_hint = excluded.sku_description_hint,
                synced_at = excluded.synced_at",
            params![
                row.catalog_id,
                row.provider,
                row.service_key,
                row.display_name,
                row.category_id,
                row.unit,
                row.default_sku,
                row.offer_code,
                row.attr_key,
                row.attr_value,
                row.billing_service_id,
                row.sku_description_hint,
                synced_at.to_rfc3339(),
            ],
        )?;
        conn.execute(
            "DELETE FROM provider_service_regions WHERE catalog_id = ?1",
            params![row.catalog_id],
        )?;
        for region in &regions {
            conn.execute(
                "INSERT INTO provider_service_regions (catalog_id, region) VALUES (?1, ?2)",
                params![row.catalog_id, region],
            )?;
        }
        Ok(())
    }

    /// Prefer a fuller region set already in SQLite when a sync pass returns sparse/junk data.
    fn coalesce_regions(
        conn: &rusqlite::Connection,
        catalog_id: &str,
        incoming: &[String],
    ) -> Result<Vec<String>> {
        let existing = load_regions(conn, catalog_id)?;
        if incoming.is_empty() {
            return Ok(existing);
        }
        // ponytail: partial API scans (e.g. one edge POP) must not wipe a good cached region list
        if incoming.len() < 3 && existing.len() > incoming.len() {
            return Ok(existing);
        }
        Ok(incoming.to_vec())
    }

    pub fn list_price_targets(&self, provider: &str) -> Result<Vec<PriceTarget>> {
        let conn = self.conn.lock().expect("db lock");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_services WHERE provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;

        if count > 0 {
            let mut stmt = conn.prepare(
                "SELECT ps.service_key, ps.default_sku, r.region, ps.unit,
                        ps.offer_code, ps.attr_key, ps.attr_value,
                        ps.billing_service_id, ps.sku_description_hint
                 FROM provider_services ps
                 JOIN provider_service_regions r ON r.catalog_id = ps.catalog_id
                 WHERE ps.provider = ?1",
            )?;
            let rows = stmt.query_map(params![provider], |row| {
                Ok(PriceTarget {
                    service_key: row.get(0)?,
                    sku: row.get(1)?,
                    region: row.get(2)?,
                    unit: row.get(3)?,
                    offer_code: row.get(4)?,
                    attr_key: row.get(5)?,
                    attr_value: row.get(6)?,
                    billing_service_id: row.get(7)?,
                    sku_description_hint: row.get(8)?,
                })
            })?;
            return rows.collect::<Result<Vec<_>, _>>().map_err(Into::into);
        }

        // Legacy fallback when provider_services empty
        let mut stmt = conn.prepare(
            "SELECT e.service_key, e.default_sku, r.region, s.unit,
                    NULL, NULL, NULL, NULL, NULL
             FROM catalog_provider_entries e
             JOIN catalog_regions r ON r.entry_id = e.id
             JOIN catalog_services s ON s.id = e.service_id
             WHERE e.provider = ?1",
        )?;
        let rows = stmt.query_map(params![provider], |row| {
            Ok(PriceTarget {
                service_key: row.get(0)?,
                sku: row.get(1)?,
                region: row.get(2)?,
                unit: row.get(3)?,
                offer_code: row.get(4)?,
                attr_key: row.get(5)?,
                attr_value: row.get(6)?,
                billing_service_id: row.get(7)?,
                sku_description_hint: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn search_catalog(
        &self,
        query: &str,
        provider: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CatalogSearchHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().expect("db lock");
        let fts = q
            .replace('"', "")
            .split_whitespace()
            .map(|t| format!("{t}*"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let lim = limit.clamp(1, 100) as i64;

        let mut hits = if let Some(p) = provider {
            let mut stmt = conn.prepare(
                "SELECT ps.catalog_id, ps.provider, ps.service_key, ps.display_name,
                        ps.category_id, c.label, ps.unit, ps.default_sku
                 FROM provider_services_fts fts
                 JOIN provider_services ps ON ps.id = fts.rowid
                 JOIN catalog_categories c ON c.id = ps.category_id
                 WHERE provider_services_fts MATCH ?1 AND ps.provider = ?2
                 ORDER BY rank
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![fts, p, lim], map_search_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT ps.catalog_id, ps.provider, ps.service_key, ps.display_name,
                        ps.category_id, c.label, ps.unit, ps.default_sku
                 FROM provider_services_fts fts
                 JOIN provider_services ps ON ps.id = fts.rowid
                 JOIN catalog_categories c ON c.id = ps.category_id
                 WHERE provider_services_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![fts, lim], map_search_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for hit in &mut hits {
            hit.regions = load_regions(&conn, &hit.catalog_id)?;
        }
        Ok(hits)
    }

    pub fn seconds_since_last_ok_sync(&self, provider: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().expect("db lock");
        let ts: Option<String> = conn
            .query_row(
                "SELECT synced_at FROM sync_log
                 WHERE provider = ?1 AND status = 'ok'
                 ORDER BY id DESC LIMIT 1",
                params![provider],
                |row| row.get(0),
            )
            .optional()?;
        Self::age_secs_from_rfc3339(ts.as_deref())
    }

    pub fn catalog_last_updated(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        conn.query_row(
            "SELECT last_updated FROM catalog_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn catalog_age_secs(&self) -> Result<Option<i64>> {
        let ts = self.catalog_last_updated()?;
        Self::age_secs_from_rfc3339(ts.as_deref())
    }

    pub fn sync_provider_age_secs(&self, provider: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().expect("db lock");
        let ts: Option<String> = conn
            .query_row(
                "SELECT synced_at FROM sync_log WHERE provider = ?1 AND status = 'ok' ORDER BY id DESC LIMIT 1",
                params![provider],
                |row| row.get(0),
            )
            .optional()?;
        Self::age_secs_from_rfc3339(ts.as_deref())
    }

    pub fn set_catalog_last_updated(&self, at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        let ts = at.to_rfc3339();
        conn.execute(
            "INSERT INTO catalog_meta (id, last_updated) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET last_updated = excluded.last_updated",
            params![ts],
        )?;
        Ok(())
    }

    fn age_secs_from_rfc3339(ts: Option<&str>) -> Result<Option<i64>> {
        let Some(ts) = ts else {
            return Ok(None);
        };
        let parsed = DateTime::parse_from_rfc3339(ts)
            .with_context(|| format!("bad timestamp: {ts}"))?;
        let age = Utc::now().signed_duration_since(parsed.with_timezone(&Utc));
        Ok(Some(age.num_seconds()))
    }

    pub fn latest_sync_status(&self) -> Result<Vec<SyncStatusEntry>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT provider, status, detail, synced_at
             FROM sync_log s
             WHERE id = (SELECT MAX(id) FROM sync_log WHERE provider = s.provider)
             ORDER BY provider",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SyncStatusEntry {
                provider: row.get(0)?,
                status: row.get(1)?,
                detail: row.get(2)?,
                synced_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn resolve_provider_service(
        &self,
        catalog_id: &str,
        provider: &str,
    ) -> Result<Option<(String, String, String)>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT service_key, default_sku, unit
             FROM provider_services
             WHERE catalog_id = ?1 AND provider = ?2",
        )?;
        let mut rows = stmt.query(params![catalog_id, provider])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn provider_service_by_key(
        &self,
        provider: &str,
        service_key: &str,
    ) -> Result<Option<ProviderServiceMeta>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT service_key, default_sku, unit, attr_key, offer_code, billing_service_id, attr_value
             FROM provider_services
             WHERE provider = ?1 AND service_key = ?2
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service_key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ProviderServiceMeta {
                service_key: row.get(0)?,
                default_sku: row.get(1)?,
                unit: row.get(2)?,
                attr_key: row.get(3)?,
                offer_code: row.get(4)?,
                billing_service_id: row.get(5)?,
                attr_value: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn provider_service_meta(
        &self,
        catalog_id: &str,
        provider: &str,
    ) -> Result<Option<ProviderServiceMeta>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT service_key, default_sku, unit, attr_key, offer_code, billing_service_id, attr_value
             FROM provider_services
             WHERE catalog_id = ?1 AND provider = ?2",
        )?;
        let mut rows = stmt.query(params![catalog_id, provider])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ProviderServiceMeta {
                service_key: row.get(0)?,
                default_sku: row.get(1)?,
                unit: row.get(2)?,
                attr_key: row.get(3)?,
                offer_code: row.get(4)?,
                billing_service_id: row.get(5)?,
                attr_value: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderServiceMeta {
    pub service_key: String,
    pub default_sku: String,
    pub unit: String,
    pub attr_key: Option<String>,
    pub offer_code: Option<String>,
    pub billing_service_id: Option<String>,
    pub attr_value: Option<String>,
}

fn map_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogSearchHit> {
    Ok(CatalogSearchHit {
        catalog_id: row.get(0)?,
        provider: row.get(1)?,
        service_key: row.get(2)?,
        display_name: row.get(3)?,
        category_id: row.get(4)?,
        category_label: row.get(5)?,
        unit: row.get(6)?,
        default_sku: row.get(7)?,
        regions: Vec::new(),
    })
}

fn load_regions(conn: &rusqlite::Connection, catalog_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT region FROM provider_service_regions WHERE catalog_id = ?1 ORDER BY region",
    )?;
    let rows = stmt.query_map(params![catalog_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::NamedTempFile;

    #[test]
    fn coalesce_regions_keeps_existing_on_sparse_sync() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        db.upsert_provider_service(
            &ProviderServiceIngest {
                catalog_id: "azure:load-balancer".into(),
                provider: "azure".into(),
                service_key: "load-balancer".into(),
                display_name: "Load Balancer".into(),
                category_id: "networking".into(),
                unit: "hours".into(),
                default_sku: "Standard".into(),
                offer_code: None,
                attr_key: Some("serviceName".into()),
                attr_value: Some("Load Balancer".into()),
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["eastus".into(), "westeurope".into(), "global".into()],
            },
            Utc::now(),
        )
        .unwrap();
        db.upsert_provider_service(
            &ProviderServiceIngest {
                catalog_id: "azure:load-balancer".into(),
                provider: "azure".into(),
                service_key: "load-balancer".into(),
                display_name: "Load Balancer".into(),
                category_id: "networking".into(),
                unit: "hours".into(),
                default_sku: "Standard".into(),
                offer_code: None,
                attr_key: Some("serviceName".into()),
                attr_value: Some("Load Balancer".into()),
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["attdetroit1".into()],
            },
            Utc::now(),
        )
        .unwrap();
        let regions = db.catalog_regions("azure:load-balancer").unwrap();
        assert_eq!(regions.len(), 3);
        assert!(regions.contains(&"eastus".to_string()));
    }

    #[test]
    fn fts_search_finds_redis() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        db.upsert_provider_service(
            &ProviderServiceIngest {
                catalog_id: "aws:AmazonElastiCache".into(),
                provider: "aws".into(),
                service_key: "AmazonElastiCache".into(),
                display_name: "Amazon ElastiCache for Redis".into(),
                category_id: "database".into(),
                unit: "hours".into(),
                default_sku: "cache.t3.medium".into(),
                offer_code: Some("AmazonElastiCache".into()),
                attr_key: None,
                attr_value: None,
                billing_service_id: None,
                sku_description_hint: None,
                regions: vec!["us-east-1".into()],
            },
            Utc::now(),
        )
        .unwrap();

        let hits = db.search_catalog("redis", Some("aws"), 10).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].display_name.contains("ElastiCache"));
    }
}
