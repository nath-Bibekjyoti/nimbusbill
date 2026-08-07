# Run NimbusBill in a browser (Axum web server)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "Starting NimbusBill web UI at http://127.0.0.1:8080"
Write-Host "Tip: cargo run -p nimbusbill --bin nimbusbill -- serve"
Write-Host "Press Ctrl+C to stop."
cargo run -p nimbusbill --bin nimbusbill -- serve
