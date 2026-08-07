#!/usr/bin/env bash
# Force catalog + price sync (same as UI "Sync catalog" button)
set -euo pipefail
cd "$(dirname "$0")"
exec cargo run -p nimbusbill --bin nimbusbill -- sync "$@"
