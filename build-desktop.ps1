# Build NimbusBill Windows installer + exe
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "Rust is not installed. Install from https://rustup.rs/"
}

if (-not (cargo tauri --version 2>$null)) {
    Write-Host "Installing Tauri CLI (one-time)..."
    cargo install tauri-cli --version "^2.0" --locked
}

Write-Host "Building NimbusBill for Windows..."
cargo tauri build

Write-Host ""
Write-Host "Done. Artifacts:"
Write-Host "  EXE: src-tauri\target\release\nimbusbill-desktop.exe"
Write-Host "  MSI: src-tauri\target\release\bundle\msi\"
