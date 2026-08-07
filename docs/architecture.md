# Architecture

How **NimbusBill** is structured internally — catalog ingestion, selective pricing, estimates, and delivery modes.

---

## Design goals

1. **Single Rust codebase** — Axum API + optional Tauri shell; no separate Node backend
2. **Cloud-sourced catalog** — services, regions, and SKU metadata discovered from public AWS / Azure / GCP APIs into SQLite
3. **Selective pricing** — infrastructure unit prices fetched on demand (live calculate), not bulk-synced
4. **Offline-first estimates** — cached prices so what-if scenarios work without live calls once populated
5. **Parallel async sync** — Tokio concurrent HTTP (not multi-process; SQLite is single-writer)
6. **Triple delivery** — **CLI**, **browser**, or **desktop** (Windows, Linux, macOS via Tauri)
7. **Minimal frontend** — vanilla HTML/CSS/JS served as static assets

---

## Runtime modes

### CLI (`nimbusbill`)

```
nimbusbill
  ├── sync      → catalog::sync_all + LLM catalog (force); no bulk infra price sync
  ├── serve     → Axum on NIMUSBILL_ADDR (default 127.0.0.1:8080)
  ├── search    → SQLite FTS5 on provider_services
  ├── status    → latest sync_log rows
  └── estimate  → YAML/JSON workload → JSON or PDF/CSV/XLSX export
```

Entry: `src/bin/nimbusbill.rs` → `src/cli.rs`

### Desktop (Tauri)

```
nimbusbill-desktop
  └── Tauri shell (src-tauri/)
        ├── Spawns Axum on 127.0.0.1:0  (api::start_background)
        ├── Opens system WebView → http://127.0.0.1:<port>/
        ├── DB: platform app-data dir (see paths.rs)
        └── Bundled static/ at build time
```

Cross-platform: **Windows** (WebView2), **macOS** (WKWebView), **Linux** (WebKitGTK).

### Web (Axum)

```
nimbusbill serve
  └── Axum on 127.0.0.1:8080 (default)
        ├── Serves static/ from repo or beside executable
        └── DB: platform app-data dir (override with NIMUSBILL_DB)
```

Legacy binary `nimbusbill-web` (`src/bin/serve.rs`) behaves the same.

All modes call `serve_listener()` in `src/api.rs` for the HTTP stack.

---

## Module map

| Module | Responsibility |
|--------|----------------|
| `api.rs` | Axum routes, static files, startup sync + daemon |
| `cli.rs` | Clap CLI (sync, serve, estimate, search, status) |
| `paths.rs` | Cross-platform default DB path |
| `catalog.rs` | API types (`CatalogResponse`, LLM entries) |
| `estimate.rs` | Cost engine, period rollups, usage display |
| `export.rs` | CSV, XLSX, PDF |
| `models.rs` | DTOs, `SyncConfig`, providers |
| `input.rs` | YAML/JSON/text workload parsers |
| `db/` | SQLite, migrations, FTS catalog, price cache |
| `pricing/` | Cache lookup + live fetch per line item |
| `sync/` | Catalog + LLM sync orchestration |
| `sync/catalog/` | AWS / Azure / GCP catalog discovery |
| `sync/llm_catalog.rs` | Bedrock / Foundry / Vertex token rates |
| `sync/parallel.rs` | Concurrent worker pool (Semaphore + JoinSet) |
| `static/` | Browser UI (50/50 layout, estimate sidebar) |

---

## Catalog pipeline

Services are **not** hardcoded. Sync populates `provider_services` from cloud APIs:

| Provider | API | Stored |
|----------|-----|--------|
| **AWS** | [Price List offers index](https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/index.json) + shared region list | Every offer (~200+), regions, default SKU metadata |
| **Azure** | [Retail Prices API](https://prices.azure.com/api/retail/prices) (paginated) | Every unique `serviceName`, regions, representative SKU |
| **GCP** | [Cloud Billing Catalog API](https://cloud.google.com/billing/docs/how-to/get-pricing-table-api) (`GCP_PRICING_API_KEY`) | Every billing service, first SKU + regions |

Token-priced APIs such as `AmazonBedrockFoundationModels` are **excluded** from infrastructure catalog — use LLM / Token Usage instead.

```
sync::run_sync(force)
  │
  ├── [if force || catalog stale]
  │     catalog::sync_all()     ← try_join!(aws, azure, gcp) in parallel
  │       ├── aws::sync         ← parallel per-offer ingest (metadata only)
  │       ├── azure::sync       ← paginated retail scan
  │       └── gcp::sync         ← parallel per-service SKU metadata
  │
  ├── provider price sync rows  ← metadata-only audit ("prices on calculate")
  │
  └── [if force || llm stale]
        tokens::sync_all()      ← llm_catalog: Bedrock + Foundry + Vertex rates
              └── seed_baseline() fallback if APIs unreachable
```

**Manual sync** (`POST /api/sync`, `nimbusbill sync`) calls `run_once_force` — always refreshes LLM catalog.

**Background daemon** (`run_once`) skips catalog/LLM when still fresh (`CATALOG_STALE_SECS` = 6 hours).

**Concurrency:** `SyncConfig.concurrency` or `NIMUSBILL_CONCURRENCY` (default ≈ CPU count, max 64). Shared `reqwest` client with connection pooling in `pricing_common.rs`. Live calculate uses a shorter-timeout client (`http_client_live`, 15s).

**Catalog IDs:** `{provider}:{service_key}` — e.g. `aws:AmazonEC2`, `azure:virtual-machines`, `gcp:compute-engine`.

---

## Infrastructure pricing (on demand)

Bulk price sync was removed. Prices enter `price_cache` via:

| Trigger | Behavior |
|---------|----------|
| **Calculate** with `live_pricing: true` | `fetch_live_price()` per resource line → upsert cache → calculate |
| **Calculate** with `live_pricing: false` | `lookup_price()` from cache only; **no network**; $0 if cache miss |
| **Live calculate** with multiple resources | Parallel `JoinSet` per provider |

Live fetch paths:

- AWS: `sync/catalog/aws.rs` → regional offer index (`fetch_price_for_target(..., live=true)`)
- Azure: `sync/catalog/azure.rs` → retail filter query
- GCP: `sync/gcp_billing.rs` → SKU list + price extraction

LLM token rates: `sync/llm_catalog.rs` — full catalog on manual sync; `sync_selected()` on live calculate for models in the estimate only.

---

## Database schema (SQLite)

### `provider_services`

Cloud-native catalog rows (discovered from APIs).

| Column | Description |
|--------|-------------|
| `catalog_id` | Unique key, e.g. `aws:AmazonElastiCache` |
| `provider`, `service_key`, `display_name` | Identity + label |
| `category_id`, `unit` | Inferred taxonomy |
| `default_sku`, `offer_code`, `attr_key`, `attr_value` | Price lookup metadata |
| `billing_service_id`, `sku_description_hint` | GCP billing API hints |

### `provider_service_regions`

Regions per `catalog_id`.

### `provider_services_fts`

FTS5 virtual table on `display_name`, `service_key`, `provider` — powers `GET /api/catalog/search` and `nimbusbill search`.

### `price_cache`

Cached unit prices `(provider, service, sku, region)` — populated by live calculate, not catalog sync.

### `token_price_cache`

LLM rates per `(provider, model)` — Bedrock / Azure OpenAI / Vertex.

### `llm_models`

Model catalog (id, label, provider, optional regions JSON).

### `sync_log`

Append-only audit: `catalog`, `aws`, `azure`, `gcp`, `llm` with status + detail.

### `estimates`

Saved estimate JSON snapshots.

Legacy tables (`catalog_services`, `catalog_provider_entries`, …) remain for migration compatibility; live catalog reads from `provider_services` when populated.

---

## Request flow (estimate)

```
POST /api/estimate  (or CLI: nimbusbill estimate)
  │
  ▼
estimate::run_ui_async()
  │
  ├── For each selected provider (infra checkboxes + token clouds):
  │     ├── resolve_catalog_resource(catalog_id, provider)
  │     ├── pricing::estimate_resources_live(live_pricing)
  │     │     live=true  → fetch_live_price per line, fallback cache
  │     │     live=false → cache only (instant)
  │     └── pricing::estimate_all_token_rows(live_pricing)
  │           live=true  → sync_selected LLM rates for estimate models
  │
  ├── PeriodBreakdown (daily → yearly)
  ├── Save to estimates table
  └── Return UiEstimateResponse
```

---

## UI layout

```
┌─────────────────────────────────────────────────────────────────┐
│ Header: logo · sync status · [Sync catalog]                     │
├──────────────────────────────┬──────────────────────────────────┤
│ Main column (config)         │ estimate-sidebar (Your Estimate)   │
│ · Cloud providers            │ · Selected chips (infra + LLM)     │
│ · Live price capture         │ · [Calculate Costs]                │
│ · Infrastructure builder     │ · Download: CSV | Excel | PDF    │
│ · LLM / Token Usage          │ · Result tables per provider       │
└──────────────────────────────┴──────────────────────────────────┘
```

---

## Export pipeline

| Format | Crate |
|--------|-------|
| CSV | `csv` |
| XLSX | `rust_xlsxwriter` |
| PDF | `printpdf` |

Filenames: `nimbusbill-estimate-YYYYMMDD-HHMMSS.{ext}`

---

## Security notes

- Default bind: **localhost only**
- No authentication in v0.1 — local / single-user
- Tauri CSP restricts WebView to localhost API

---

## Workspace layout

| Crate | Role |
|-------|------|
| `nimbusbill` | Library + `nimbusbill` CLI + `nimbusbill-web` |
| `nimbusbill-desktop` | Tauri shell (`src-tauri/`) |

Default workspace member: `src-tauri` (desktop builds from repo root).

---

## Live integration tests

`src/sync/catalog/live_tests.rs` — gated with `#[ignore]`:

```bash
cargo test live_aws_catalog_sync -- --ignored
GCP_PRICING_API_KEY=... cargo test live_gcp_catalog_sync -- --ignored
```
