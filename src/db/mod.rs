mod schema;
mod catalog;
mod provider_catalog;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub use catalog::{load_catalog_from_conn, seed_catalog, PriceTarget};
pub use provider_catalog::{CatalogSearchHit, ProviderServiceIngest, ProviderServiceMeta, SyncStatusEntry};
pub use schema::migrate;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).context("open sqlite database")?;
        migrate(&conn)?;
        seed_catalog(&conn).context("seed catalog")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn upsert_price(
        &self,
        provider: &str,
        service: &str,
        sku: &str,
        region: &str,
        unit: &str,
        price: &str,
        currency: &str,
        fetched_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO price_cache (provider, service, sku, region, unit, price, currency, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(provider, service, sku, region) DO UPDATE SET
               unit = excluded.unit,
               price = excluded.price,
               currency = excluded.currency,
               fetched_at = excluded.fetched_at",
            params![
                provider,
                service,
                sku,
                region,
                unit,
                price,
                currency,
                fetched_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn lookup_price(
        &self,
        provider: &str,
        service: &str,
        sku: &str,
        region: &str,
    ) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().expect("db lock");
        let exact = "SELECT price, currency FROM price_cache
             WHERE provider = ?1 AND service = ?2 AND sku = ?3 AND region = ?4";
        let mut stmt = conn.prepare(exact)?;
        let mut rows = stmt.query(params![provider, service, sku, region])?;
        if let Some(row) = rows.next()? {
            return Ok(Some((row.get(0)?, row.get(1)?)));
        }

        // Same SKU in another region (common when sync has not reached every region yet).
        let mut stmt = conn.prepare(
            "SELECT price, currency FROM price_cache
             WHERE provider = ?1 AND service = ?2 AND sku = ?3
             ORDER BY fetched_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service, sku])?;
        if let Some(row) = rows.next()? {
            return Ok(Some((row.get(0)?, row.get(1)?)));
        }

        // Any SKU for this service in the requested region.
        let mut stmt = conn.prepare(
            "SELECT price, currency FROM price_cache
             WHERE provider = ?1 AND service = ?2 AND region = ?3
             ORDER BY fetched_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service, region])?;
        if let Some(row) = rows.next()? {
            return Ok(Some((row.get(0)?, row.get(1)?)));
        }

        // Anchor-region sync may store a different SKU label than the UI sends.
        let mut stmt = conn.prepare(
            "SELECT price, currency FROM price_cache
             WHERE provider = ?1 AND service = ?2
             ORDER BY fetched_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service])?;
        if let Some(row) = rows.next()? {
            return Ok(Some((row.get(0)?, row.get(1)?)));
        }

        Ok(None)
    }

    pub fn service_attr_value(&self, provider: &str, service_key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT attr_value FROM provider_services
             WHERE provider = ?1 AND service_key = ?2 AND attr_value IS NOT NULL
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service_key])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    pub fn service_billing_id(&self, provider: &str, service_key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT billing_service_id FROM provider_services
             WHERE provider = ?1 AND service_key = ?2 AND billing_service_id IS NOT NULL
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service_key])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    pub fn service_attr_key(&self, provider: &str, service_key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT attr_key FROM provider_services
             WHERE provider = ?1 AND service_key = ?2 AND attr_key IS NOT NULL
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![provider, service_key])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    /// Distinct SKUs with cached prices for a service (optionally scoped to a region).
    pub fn list_cached_skus(
        &self,
        provider: &str,
        service: &str,
        region: Option<&str>,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut skus = Vec::new();
        if let Some(region) = region {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT sku FROM price_cache
                 WHERE provider = ?1 AND service = ?2 AND region = ?3
                 ORDER BY sku",
            )?;
            let rows = stmt.query_map(params![provider, service, region], |row| row.get(0))?;
            for row in rows {
                skus.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT sku FROM price_cache
                 WHERE provider = ?1 AND service = ?2
                 ORDER BY sku",
            )?;
            let rows = stmt.query_map(params![provider, service], |row| row.get(0))?;
            for row in rows {
                skus.push(row?);
            }
        }
        Ok(skus)
    }

    pub fn catalog_coverage(&self, provider: &str) -> Result<(i64, i64, i64)> {
        let conn = self.conn.lock().expect("db lock");
        let services: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_services WHERE provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;
        let regions: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT r.region)
             FROM provider_service_regions r
             JOIN provider_services ps ON ps.catalog_id = r.catalog_id
             WHERE ps.provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;
        let pairs: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM provider_service_regions r
             JOIN provider_services ps ON ps.catalog_id = r.catalog_id
             WHERE ps.provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;
        Ok((services, regions, pairs))
    }

    pub fn price_cache_coverage(&self, provider: &str) -> Result<(i64, i64)> {
        let conn = self.conn.lock().expect("db lock");
        let prices: i64 = conn.query_row(
            "SELECT COUNT(*) FROM price_cache WHERE provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;
        let regions: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT region) FROM price_cache WHERE provider = ?1",
            params![provider],
            |row| row.get(0),
        )?;
        Ok((prices, regions))
    }

    pub fn record_sync(&self, provider: &str, status: &str, detail: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO sync_log (provider, status, detail, synced_at) VALUES (?1, ?2, ?3, ?4)",
            params![provider, status, detail, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn save_estimate(&self, id: &str, payload: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO estimates (id, payload, created_at) VALUES (?1, ?2, ?3)",
            params![id, payload, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn upsert_token_price(
        &self,
        provider: &str,
        model: &str,
        input_per_mtok: &str,
        output_per_mtok: &str,
        currency: &str,
        fetched_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO token_price_cache (provider, model, input_per_mtok, output_per_mtok, currency, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(provider, model) DO UPDATE SET
               input_per_mtok = excluded.input_per_mtok,
               output_per_mtok = excluded.output_per_mtok,
               currency = excluded.currency,
               fetched_at = excluded.fetched_at",
            params![
                provider,
                model,
                input_per_mtok,
                output_per_mtok,
                currency,
                fetched_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn lookup_token_price(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT input_per_mtok, output_per_mtok FROM token_price_cache
             WHERE provider = ?1 AND model = ?2",
        )?;
        let mut rows = stmt.query(params![provider, model])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }
}
