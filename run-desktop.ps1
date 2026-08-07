# Run NimbusBill desktop app (development)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "Rust is not installed. Install from https://rustup.rs/"
}

if (-not (cargo tauri --version 2>$null)) {
    Write-Host "Installing Tauri CLI (one-time)..."
    cargo install tauri-cli --version "^2.0" --locked
}

Write-Host "Starting NimbusBill desktop..."
cargo tauri dev
