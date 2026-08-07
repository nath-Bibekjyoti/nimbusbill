use nimbusbill::api::{self, resolve_static_dir};
use nimbusbill::paths::{default_db_path, ensure_db_parent};
use nimbusbill::{Database, SyncConfig};
use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nimbusbill=info".parse().unwrap()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let handle = app.handle().clone();

            tauri::async_runtime::block_on(async move {
                start_desktop(&handle).await
            })
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running NimbusBill desktop");
}

async fn start_desktop(handle: &tauri::AppHandle) -> anyhow::Result<()> {
    let db_path = default_db_path();
    ensure_db_parent(&db_path)?;

    let db = Database::open(&db_path)?;
    let static_dir = desktop_static_dir(handle);
    let sync_config = SyncConfig::default();

    let addr = api::start_background(db, sync_config, static_dir).await?;
    let url: url::Url = format!("http://{addr}").parse()?;

    WebviewWindowBuilder::new(handle, "main", WebviewUrl::External(url))
        .title("NimbusBill")
        .inner_size(1280.0, 860.0)
        .min_inner_size(960.0, 640.0)
        .build()?;

    tracing::info!("desktop UI at http://{addr}");
    Ok(())
}

fn desktop_static_dir(handle: &tauri::AppHandle) -> PathBuf {
    if let Ok(resource) = handle.path().resolve("static", tauri::path::BaseDirectory::Resource) {
        if resource.join("app.js").exists() {
            return resource;
        }
    }
    resolve_static_dir()
}
