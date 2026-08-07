//! Cross-platform default paths (Windows, Linux, macOS).

use std::path::PathBuf;

/// Default SQLite database path.
///
/// Override with `NIMUSBILL_DB`. Otherwise uses OS-specific app data dir:
/// - Windows: `%APPDATA%\NimbusBill\data\nimbusbill.db`
/// - macOS: `~/Library/Application Support/NimbusBill/nimbusbill.db`
/// - Linux: `~/.local/share/nimbusbill/nimbusbill.db`
pub fn default_db_path() -> PathBuf {
    std::env::var("NIMUSBILL_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| platform_data_dir().join("nimbusbill.db"))
}

fn platform_data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "NimbusBill", "NimbusBill")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Ensure parent directory exists before opening the database.
pub fn ensure_db_parent(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}
