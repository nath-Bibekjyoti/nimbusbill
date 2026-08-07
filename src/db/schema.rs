use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS price_cache (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    provider    TEXT NOT NULL,
    service     TEXT NOT NULL,
    sku         TEXT NOT NULL,
    region      TEXT NOT NULL,
    unit        TEXT NOT NULL,
    price       TEXT NOT NULL,
    currency    TEXT NOT NULL DEFAULT 'USD',
    fetched_at  TEXT NOT NULL,
    UNIQUE(provider, service, sku, region)
);

CREATE INDEX IF NOT EXISTS idx_price_lookup
    ON price_cache(provider, service, region);

CREATE TABLE IF NOT EXISTS token_price_cache (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    input_per_mtok  TEXT NOT NULL,
    output_per_mtok TEXT NOT NULL,
    currency        TEXT NOT NULL DEFAULT 'USD',
    fetched_at      TEXT NOT NULL,
    UNIQUE(provider, model)
);

CREATE TABLE IF NOT EXISTS sync_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    provider    TEXT NOT NULL,
    status      TEXT NOT NULL,
    detail      TEXT,
    synced_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sync_log_provider_time
    ON sync_log(provider, synced_at DESC);

CREATE TABLE IF NOT EXISTS estimates (
    id          TEXT PRIMARY KEY,
    payload     TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS catalog_categories (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS catalog_services (
    id          TEXT PRIMARY KEY,
    category_id TEXT NOT NULL REFERENCES catalog_categories(id),
    name        TEXT NOT NULL,
    unit        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS catalog_provider_entries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    service_id  TEXT NOT NULL REFERENCES catalog_services(id),
    provider    TEXT NOT NULL,
    service_key TEXT NOT NULL,
    default_sku TEXT NOT NULL,
    UNIQUE(service_id, provider)
);

CREATE TABLE IF NOT EXISTS catalog_regions (
    entry_id    INTEGER NOT NULL REFERENCES catalog_provider_entries(id),
    region      TEXT NOT NULL,
    PRIMARY KEY (entry_id, region)
);

CREATE TABLE IF NOT EXISTS llm_models (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    provider    TEXT NOT NULL
);

-- Cloud-native catalog (discovered from provider APIs)
CREATE TABLE IF NOT EXISTS provider_services (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    catalog_id           TEXT NOT NULL UNIQUE,
    provider             TEXT NOT NULL,
    service_key          TEXT NOT NULL,
    display_name         TEXT NOT NULL,
    category_id          TEXT NOT NULL REFERENCES catalog_categories(id),
    unit                 TEXT NOT NULL DEFAULT 'hours',
    default_sku          TEXT NOT NULL,
    offer_code           TEXT,
    attr_key             TEXT,
    attr_value           TEXT,
    billing_service_id   TEXT,
    sku_description_hint TEXT,
    synced_at            TEXT NOT NULL,
    UNIQUE(provider, service_key)
);

CREATE INDEX IF NOT EXISTS idx_provider_services_provider
    ON provider_services(provider);

CREATE INDEX IF NOT EXISTS idx_provider_services_category
    ON provider_services(category_id);

CREATE TABLE IF NOT EXISTS provider_service_regions (
    catalog_id  TEXT NOT NULL REFERENCES provider_services(catalog_id) ON DELETE CASCADE,
    region      TEXT NOT NULL,
    PRIMARY KEY (catalog_id, region)
);

CREATE INDEX IF NOT EXISTS idx_provider_service_regions_catalog
    ON provider_service_regions(catalog_id);

CREATE TABLE IF NOT EXISTS catalog_meta (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    last_updated  TEXT NOT NULL
);
"#;

const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS provider_services_fts USING fts5(
    display_name,
    service_key,
    provider,
    content='provider_services',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS provider_services_ai AFTER INSERT ON provider_services BEGIN
    INSERT INTO provider_services_fts(rowid, display_name, service_key, provider)
    VALUES (new.id, new.display_name, new.service_key, new.provider);
END;

CREATE TRIGGER IF NOT EXISTS provider_services_ad AFTER DELETE ON provider_services BEGIN
    INSERT INTO provider_services_fts(provider_services_fts, rowid, display_name, service_key, provider)
    VALUES ('delete', old.id, old.display_name, old.service_key, old.provider);
END;

CREATE TRIGGER IF NOT EXISTS provider_services_au AFTER UPDATE ON provider_services BEGIN
    INSERT INTO provider_services_fts(provider_services_fts, rowid, display_name, service_key, provider)
    VALUES ('delete', old.id, old.display_name, old.service_key, old.provider);
    INSERT INTO provider_services_fts(rowid, display_name, service_key, provider)
    VALUES (new.id, new.display_name, new.service_key, new.provider);
END;
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(FTS_SCHEMA)?;
    ensure_llm_regions_column(conn)?;
    purge_llm_token_infra_services(conn)?;
    rebuild_fts_if_empty(conn)?;
    backfill_catalog_meta(conn)?;
    Ok(())
}

fn ensure_llm_regions_column(conn: &Connection) -> Result<()> {
    let has_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('llm_models') WHERE name = 'regions'",
        [],
        |row| row.get(0),
    )?;
    if has_col == 0 {
        conn.execute("ALTER TABLE llm_models ADD COLUMN regions TEXT", [])?;
    }
    Ok(())
}

fn purge_llm_token_infra_services(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM provider_services WHERE service_key = 'AmazonBedrockFoundationModels'",
        [],
    )?;
    Ok(())
}

fn backfill_catalog_meta(conn: &Connection) -> Result<()> {
    let exists: i64 = conn.query_row("SELECT COUNT(*) FROM catalog_meta", [], |row| row.get(0))?;
    if exists > 0 {
        return Ok(());
    }
    let max_ts: Option<String> = conn
        .query_row("SELECT MAX(synced_at) FROM provider_services", [], |row| row.get(0))
        .optional()?;
    if let Some(ts) = max_ts {
        conn.execute(
            "INSERT INTO catalog_meta (id, last_updated) VALUES (1, ?1)",
            params![ts],
        )?;
    }
    Ok(())
}

fn rebuild_fts_if_empty(conn: &Connection) -> Result<()> {
    let fts_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM provider_services_fts",
        [],
        |row| row.get(0),
    )?;
    let svc_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM provider_services",
        [],
        |row| row.get(0),
    )?;
    if svc_count > 0 && fts_count == 0 {
        conn.execute("INSERT INTO provider_services_fts(provider_services_fts) VALUES('rebuild')", [])?;
    }
    Ok(())
}
