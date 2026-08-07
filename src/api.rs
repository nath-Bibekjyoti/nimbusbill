use crate::catalog::AddLlmModelRequest;
use crate::db::Database;
use crate::estimate;
use crate::export::{ExportRequest, export};
use crate::import;
use crate::models::{EstimateRequest, SyncConfig};
use crate::sync;
use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Query, State},
    http::{StatusCode, header},
    response::{Html, Response},
    routing::{get, post},
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::{services::ServeDir, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub sync_config: SyncConfig,
}

pub fn default_static_dir() -> PathBuf {
    PathBuf::from("static")
}

/// Resolve static assets: prefer repo `static/` (dev), then bundled next to exe.
pub fn resolve_static_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    if manifest.join("app.js").exists() {
        return manifest;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("static");
            if bundled.join("app.js").exists() {
                return bundled;
            }
        }
    }
    default_static_dir()
}

pub async fn serve(
    addr: SocketAddr,
    db: Database,
    sync_config: SyncConfig,
    static_dir: PathBuf,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_listener(listener, db, sync_config, static_dir).await
}

/// Bind `127.0.0.1:0`, spawn the server in the background, return the bound address.
pub async fn start_background(
    db: Database,
    sync_config: SyncConfig,
    static_dir: PathBuf,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(e) = serve_listener(listener, db, sync_config, static_dir).await {
            tracing::error!(error = %e, "embedded server stopped");
        }
    });
    Ok(addr)
}

async fn serve_listener(
    listener: TcpListener,
    db: Database,
    sync_config: SyncConfig,
    static_dir: PathBuf,
) -> Result<()> {
    sync::seed_token_prices(&db)?;

    let addr = listener.local_addr()?;
    let state = Arc::new(AppState {
        db: db.clone(),
        sync_config: sync_config.clone(),
    });

    let sync_db = db.clone();
    let sync_config_initial = sync_config.clone();
    tokio::spawn(async move {
        if let Err(e) = sync::run_once(&sync_db, &sync_config_initial).await {
            tracing::error!(error = %e, "initial sync failed");
        }
    });

    let sync_db = db.clone();
    tokio::spawn(async move {
        sync::run_daemon(sync_db, sync_config).await;
    });

    let static_dir = static_dir.canonicalize().unwrap_or(static_dir);
    tracing::info!(path = %static_dir.display(), "serving static files");

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/catalog", get(get_catalog))
        .route("/api/catalog/search", get(search_catalog))
        .route("/api/catalog/skus", get(list_catalog_skus))
        .route("/api/sync/status", get(sync_status))
        .route("/api/llm-models", post(add_llm_model))
        .route("/api/estimate", post(estimate_handler))
        .route("/api/import", post(import_handler))
        .route("/api/export", post(export_handler))
        .route("/api/sync", post(trigger_sync))
        .nest_service("/static", ServeDir::new(static_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!("UI available at http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn health() -> &'static str {
    "ok"
}

async fn get_catalog(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let catalog = state.db.load_catalog().map_err(|e| {
        tracing::error!(error = %e, "catalog load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(serde_json::to_value(catalog).unwrap()))
}

#[derive(serde::Deserialize)]
struct CatalogSearchQuery {
    q: String,
    provider: Option<String>,
    limit: Option<usize>,
}

async fn search_catalog(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CatalogSearchQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let hits = state
        .db
        .search_catalog(&query.q, query.provider.as_deref(), query.limit.unwrap_or(25))
        .map_err(|e| {
            tracing::error!(error = %e, "catalog search failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(serde_json::to_value(hits).unwrap()))
}

#[derive(serde::Deserialize)]
struct CatalogSkuQuery {
    catalog_id: String,
    provider: String,
    region: String,
    #[serde(default)]
    live: bool,
}

async fn list_catalog_skus(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CatalogSkuQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let meta = state
        .db
        .provider_service_meta(&query.catalog_id, &query.provider)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut skus = state
        .db
        .list_cached_skus(&query.provider, &meta.service_key, Some(&query.region))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut sku_options: Vec<serde_json::Value> = Vec::new();

    if query.live {
        match query.provider.as_str() {
            "aws" => {
                let offer = meta.offer_code.as_deref().unwrap_or(&meta.service_key);
                let key = meta
                    .attr_key
                    .as_deref()
                    .or(crate::sync::aws_attr_key(offer));
                if let Some(key) = key {
                    if let Ok(live_skus) =
                        crate::sync::list_offer_skus(offer, &query.region, key).await
                    {
                        skus = live_skus;
                    }
                }
            }
            "azure" => {
                if let Some(service_name) = meta.attr_value.as_deref() {
                    if let Ok(live_skus) =
                        crate::sync::list_retail_skus(service_name, &query.region).await
                    {
                        skus = live_skus;
                    }
                }
            }
            "gcp" => {
                if let Some(billing_id) = meta.billing_service_id.as_deref() {
                    if let Ok(opts) =
                        crate::sync::list_service_skus(billing_id, &query.region).await
                    {
                        sku_options = opts
                            .iter()
                            .map(|(value, label)| {
                                serde_json::json!({ "value": value, "label": label })
                            })
                            .collect();
                        skus = opts.into_iter().map(|(value, _)| value).collect();
                    }
                }
            }
            _ => {}
        }
    }

    if skus.is_empty() {
        skus = state
            .db
            .list_cached_skus(&query.provider, &meta.service_key, None)
            .unwrap_or_default();
    }

    if !meta.default_sku.is_empty() && !skus.contains(&meta.default_sku) {
        skus.insert(0, meta.default_sku.clone());
    } else if skus.is_empty() && !meta.default_sku.is_empty() {
        skus.push(meta.default_sku.clone());
    }

    Ok(Json(serde_json::json!({
        "skus": skus,
        "sku_options": sku_options,
        "default_sku": meta.default_sku,
        "attr_key": meta.attr_key,
        "unit": meta.unit,
    })))
}

async fn sync_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let status = state.db.latest_sync_status().map_err(|e| {
        tracing::error!(error = %e, "sync status failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let catalog_last_updated = state.db.catalog_last_updated().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog_fresh = state
        .db
        .catalog_age_secs()
        .map(|age| age.is_some_and(|s| s < sync::CATALOG_STALE_SECS as i64))
        .unwrap_or(false);
    Ok(Json(serde_json::json!({
        "catalog_last_updated": catalog_last_updated,
        "catalog_fresh": catalog_fresh,
        "catalog_stale_secs": sync::CATALOG_STALE_SECS,
        "sync": status,
    })))
}

async fn add_llm_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddLlmModelRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if body.id.trim().is_empty() || body.label.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !["aws", "azure", "gcp"].contains(&body.provider.trim()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    state.db.upsert_custom_llm(
        body.id.trim(),
        body.label.trim(),
        body.provider.trim(),
        body.input_per_mtok.trim(),
        body.output_per_mtok.trim(),
    ).map_err(|e| {
        tracing::error!(error = %e, "add llm model failed");
        StatusCode::BAD_REQUEST
    })?;
    let catalog = state.db.load_catalog().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(catalog).unwrap()))
}

async fn import_handler(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut filename = "upload.json".to_string();
    let mut bytes = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        if field.name() == Some("file") {
            filename = field
                .file_name()
                .unwrap_or("upload.json")
                .to_string();
            bytes = field
                .bytes()
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?
                .to_vec();
        }
    }

    if bytes.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let result = import::import_bytes(&state.db, &filename, &bytes).map_err(|e| {
        tracing::error!(error = %e, "import failed");
        StatusCode::BAD_REQUEST
    })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn estimate_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EstimateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let result = estimate::run_ui_async(&state.db, &request)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "estimate failed");
            StatusCode::BAD_REQUEST
        })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn export_handler(Json(request): Json<ExportRequest>) -> Result<Response, StatusCode> {
    let file = export(&request).map_err(|e| {
        tracing::error!(error = %e, "export failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file.filename),
        )
        .body(Body::from(file.bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn trigger_sync(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.clone();
    let config = state.sync_config.clone();
    tokio::spawn(async move {
        if let Err(e) = sync::run_once_force(&db, &config).await {
            tracing::error!(error = %e, "manual sync failed");
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_static_dir_exists() {
        let dir = resolve_static_dir();
        assert!(dir.join("app.js").exists(), "missing {}", dir.display());
    }
}
