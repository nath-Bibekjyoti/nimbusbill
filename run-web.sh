#!/usr/bin/env bash
# Run NimbusBill in a browser (Axum web server) — Linux / macOS
set -euo pipefail
cd "$(dirname "$0")"
echo "Starting NimbusBill web UI at http://127.0.0.1:8080"
cargo run -p nimbusbill --bin nimbusbill -- serve "$@"
