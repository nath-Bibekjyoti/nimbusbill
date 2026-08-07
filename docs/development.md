# Development guide

Instructions for building, testing, and extending **NimbusBill**.

---

## Environment setup

### 1. Install Rust

```powershell
# https://rustup.rs/
rustup default stable
rustc --version   # 1.77+
```

Linux / macOS: same via `rustup`.

### 2. Windows build tools

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++**.

Required for `rusqlite`, Tauri, and native dependencies.

### 3. Tauri CLI (desktop only)

```powershell
cargo install tauri-cli --version "^2.0" --locked
cargo tauri --version
```

### 4. Clone and verify

```powershell
cd CosrEstimator
cargo test -p nimbusbill
```

---

## Running locally

| Goal | Command |
|------|---------|
| Desktop (dev) | `.\run-desktop.ps1` or `cargo tauri dev` |
| Desktop (release) | `.\build-desktop.ps1` or `cargo tauri build` |
| Web UI | `.\run-web.ps1` or `./run-web.sh` |
| CLI sync | `cargo run --bin nimbusbill -- sync` |
| CLI serve | `cargo run --bin nimbusbill -- serve` |
| Library tests | `cargo test -p nimbusbill` |

### WSL vs Windows

If you develop on Windows but run in WSL (`~/NimbusBill`), keep copies in sync:

```bash
rsync -a --exclude target /mnt/c/Users/358033/Workspace/Adhoc/Scripts/CosrEstimator/ ~/NimbusBill/
cd ~/NimbusBill && cargo build -p nimbusbill --bin nimbusbill
```

Compile errors in WSL often mean the tree is behind the Windows repo.

### Environment variables

| Variable | Effect |
|----------|--------|
| `NIMUSBILL_DB` | SQLite path (override default app-data location) |
| `NIMUSBILL_CONCURRENCY` | Parallel HTTP workers during catalog sync (default ≈ CPU count) |
| `NIMUSBILL_ADDR` | Web bind address (default `127.0.0.1:8080`) |
| `GCP_PRICING_API_KEY` | GCP Cloud Billing Catalog API key |
| `RUST_LOG=nimbusbill=debug` | Verbose logging |
| `RUST_LOG=debug` | Full trace including Axum |

Example:

```powershell
$env:RUST_LOG = "nimbusbill=debug"
$env:GCP_PRICING_API_KEY = "your-key"
cargo run --bin nimbusbill -- sync --concurrency 16
```

---

## Project layout (detailed)

```
src/
├── lib.rs                  # Library root; re-exports modules
├── api.rs                  # Axum server, routes, static serving
├── cli.rs                  # Clap CLI (sync, serve, estimate, search, status)
├── paths.rs                # Cross-platform default DB path
├── bin/
│   ├── nimbusbill.rs       # CLI entry point
│   └── serve.rs            # Legacy web binary (nimbusbill-web)
├── catalog.rs              # API types (CatalogResponse, LLM entries)
├── estimate.rs             # Estimate engine + tests
├── export.rs               # CSV/XLSX/PDF + export tests
├── input.rs                # YAML/JSON/text workload parsers
├── models.rs               # Domain types, SyncConfig, providers
├── db/
│   ├── mod.rs              # Database wrapper
│   ├── schema.rs           # SQL migrations
│   ├── catalog.rs          # Catalog load, LLM models, upsert
│   └── provider_catalog.rs # provider_services + FTS search
├── pricing/
│   ├── mod.rs              # estimate_resource, live vs cache paths
│   ├── aws.rs              # AWS adapter
│   ├── azure.rs            # Azure adapter
│   ├── gcp.rs              # GCP adapter
│   └── tokens.rs           # LLM token cost math
└── sync/
    ├── mod.rs              # run_once, run_once_force, fetch_live_price
    ├── parallel.rs         # Concurrent worker pool (Semaphore)
    ├── pricing_common.rs   # HTTP clients (90s catalog, 15s live)
    ├── aws.rs              # AWS live price fetch
    ├── azure.rs            # Azure live price fetch
    ├── gcp.rs                # GCP live price fetch
    ├── gcp_billing.rs      # GCP billing API helpers
    ├── llm_catalog.rs      # Bedrock / Foundry / Vertex sync + baseline
    ├── tokens.rs           # Re-export sync_all
    └── catalog/
        ├── mod.rs          # catalog::sync_all (parallel providers)
        ├── aws.rs          # AWS offers index ingestion (metadata)
        ├── azure.rs        # Azure retail prices scan
        ├── gcp.rs          # GCP billing services + SKUs
        ├── specs.rs        # Category/unit inference, is_llm_token_service
        └── live_tests.rs   # #[ignore] live API tests

static/
├── index.html              # 50/50 layout, estimate sidebar
├── app.js                  # UI: search, sync poll, estimates, export
└── styles.css

src-tauri/
├── Cargo.toml
├── tauri.conf.json
└── src/
    ├── main.rs
    └── lib.rs              # Tauri + embedded Axum server
```

---

## Catalog sync (how to extend)

Services are discovered from cloud APIs into `provider_services` — **not** edited in `catalog.rs`.

### Flow

1. `sync::run_sync(force)` — manual sync uses `force=true`
2. `catalog::sync_all()` when `force || catalog_needs_sync()` — AWS ∥ Azure ∥ GCP metadata
3. **No bulk infra price sync** — `price_cache` is filled by live calculate (`fetch_live_price`)
4. `tokens::sync_all()` when `force || llm_catalog_needs_sync()` — LLM rates; baseline seed if APIs fail
5. FTS index (`provider_services_fts`) updates automatically via triggers

### Excluding token-priced services from infra

Edit `is_llm_token_service()` in `src/sync/catalog/specs.rs` — e.g. `AmazonBedrockFoundationModels` → use LLM section only.

### Adding inference for new service types

Edit `src/sync/catalog/specs.rs` — map offer codes / service names to `category_id` and `unit` via `infer_category()` / `infer_unit()`.

### Tuning concurrency

```bash
NIMUSBILL_CONCURRENCY=32 cargo run --bin nimbusbill -- sync
```

Uses `src/sync/parallel.rs` — Tokio semaphore, shared `reqwest` client. Intentionally **not** multi-process (SQLite single-writer).

### GCP setup

1. Enable [Cloud Billing API](https://console.cloud.google.com/apis/library/cloudbilling.googleapis.com)
2. Create an API key
3. `export GCP_PRICING_API_KEY=...`

Without the key, AWS + Azure catalog sync still works; GCP catalog sync is skipped.

---

## Pricing (on demand)

| Path | When |
|------|------|
| `pricing::estimate_resource(..., live=false)` | Cache lookup only — no HTTP |
| `pricing::estimate_resource(..., live=true)` | `sync::fetch_live_price` → upsert → calculate |
| `sync::refresh_token_prices` | Live calculate with token usage |

Live HTTP uses `fetch_json_retry_live` (15s timeout) in `pricing_common.rs`.

To add a new live fetch provider, implement `fetch_live` in `sync/{provider}.rs` and wire `fetch_live_price` in `sync/mod.rs`.

---

## Workload files (CLI estimate)

YAML example (`examples/sample-workload.yaml`):

```yaml
name: sample
providers: [aws, azure]
live_pricing: false
resources:
  - catalog_id: aws:AmazonEC2
    provider: aws
    region: us-east-1
    quantity: "730"
token_usage:
  - model: gpt-4o
    provider: azure
    input_tokens_per_month: 5000000
    output_tokens_per_month: 1000000
```

```bash
cargo run --bin nimbusbill -- estimate examples/sample-workload.yaml
cargo run --bin nimbusbill -- estimate workload.yaml --live --export xlsx -o out.xlsx
```

Parser: `src/input.rs`

---

## Running tests

```powershell
# All unit tests (live API tests are skipped — see below)
cargo test -p nimbusbill --lib

# Specific modules
cargo test -p nimbusbill --lib estimate::
cargo test -p nimbusbill --lib export::
cargo test -p nimbusbill --lib db::provider_catalog::
```

Linux / macOS: same commands (`cargo test -p nimbusbill --lib`).

Tests use `tempfile` for isolated SQLite databases.

### Live API tests

Live tests live in `src/sync/catalog/live_tests.rs` and are marked `#[ignore]`, so a normal `cargo test` **does not** run them. Use `--ignored` to run **only** ignored tests, or `--include-ignored` to run unit tests **and** live tests together.

| Test | API | Requirements |
|------|-----|--------------|
| `live_aws_catalog_sync` | AWS Price List | Network |
| `live_azure_catalog_sync` | Azure Retail Prices | Network; full scan is slow |
| `live_gcp_catalog_sync` | GCP Cloud Billing Catalog | `GCP_PRICING_API_KEY`; [Cloud Billing API](https://console.cloud.google.com/apis/library/cloudbilling.googleapis.com) enabled |

**Run one live test** (PowerShell):

```powershell
cargo test -p nimbusbill --lib live_aws_catalog_sync -- --ignored --nocapture

cargo test -p nimbusbill --lib live_azure_catalog_sync -- --ignored --nocapture

$env:GCP_PRICING_API_KEY = "your-key"
cargo test -p nimbusbill --lib live_gcp_catalog_sync -- --ignored --nocapture
```

**Run one live test** (Linux / macOS / WSL):

```bash
cargo test -p nimbusbill --lib live_aws_catalog_sync -- --ignored --nocapture

cargo test -p nimbusbill --lib live_azure_catalog_sync -- --ignored --nocapture

export GCP_PRICING_API_KEY=your-key
cargo test -p nimbusbill --lib live_gcp_catalog_sync -- --ignored --nocapture
```

**Run all live tests:**

```bash
cargo test -p nimbusbill --lib -- --ignored --nocapture
```

(GCP test prints `skip: GCP_PRICING_API_KEY not set` and returns early if the key is missing.)

**Run unit tests + live tests:**

```bash
cargo test -p nimbusbill --lib -- --include-ignored
```

**WSL + Cargo HTTPS errors:** If `cargo build` fails fetching from `index.crates.io`:

| Error | Meaning | Fix |
|-------|---------|-----|
| `[28] Timeout` | WSL cannot reach the internet (VPN/WSL networking glitch) | `wsl --shutdown` in PowerShell, reopen WSL; retry |
| `[77] SSL CA cert` | Bad or missing CA bundle path | **Do not** set `CARGO_HTTP_CAINFO` unless the file exists. Run `unset CARGO_HTTP_CAINFO SSL_CERT_FILE` and retry |

Quick check in WSL:

```bash
curl -sS https://index.crates.io/config.json -o /dev/null && echo OK
```

If that prints `OK`, Cargo should work — no proxy URL needed. Your Windows browser uses a PAC file (`AutoConfigURL`); WSL apps use direct HTTPS and normally share the same trust store via `/etc/ssl/certs/ca-certificates.crt`.

**Build on Windows** (if WSL keeps failing):

```powershell
cd C:\Users\358033\Workspace\Adhoc\Scripts\CosrEstimator
cargo run -p nimbusbill --bin nimbusbill -- serve
```

---

## Release build tips

### Desktop MSI (Windows)

```powershell
cargo tauri build
```

Tauri also builds on **Linux** and **macOS** with the same command (platform WebView required).

### Icons

```powershell
cargo tauri icon src-tauri/icons/app-icon.png
```

### Release profile

Root `Cargo.toml`:

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
```

---

## Common build errors (Windows)

| Error | Fix |
|-------|-----|
| `link.exe` not found | Install MSVC Build Tools |
| `dlltool.exe` not found | Add GNU toolchain or use MSVC target |
| WebView2 missing | Install [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) |
| Group policy blocks build scripts (os error 1260) | Run outside locked-down sandbox; contact IT |
| `static/` not found at runtime | Run from repo root, or copy `static/` next to exe |

---

## Code conventions

- **Money:** `rust_decimal::Decimal` — never `f64` for currency
- **Errors:** `anyhow::Result` in application code
- **Async:** Tokio; DB uses `Mutex<Connection>` (single-user desktop tool)
- **API JSON:** Decimal serializes as string (`serde-with-str`)
- **Catalog IDs:** `{provider}:{service_key}` e.g. `aws:AmazonEC2`
- **Deliberate shortcuts:** mark with `ponytail:` comment naming ceiling + upgrade path

---

## Related docs

- [architecture.md](architecture.md) — system design, sync pipeline
- [api.md](api.md) — HTTP reference
- [user-guide.md](user-guide.md) — end-user UI guide
