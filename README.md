# NimbusBill

**NimbusBill** is a multi-cloud infrastructure and LLM token cost projection tool, built entirely in **Rust**. It helps teams estimate spending across **AWS**, **Azure**, and **GCP** before deployment, with optional LLM/token cost modeling and export to **CSV**, **XLSX**, or **PDF**.

---

## What it does

| Capability | Description |
|------------|-------------|
| **Multi-cloud comparison** | Estimate the same workload on AWS, Azure, and GCP side by side |
| **API-driven catalog** | Discover services from AWS/Azure/GCP public APIs into SQLite |
| **Fast search** | FTS search across hundreds of synced services (`/api/catalog/search`) |
| **Billing periods** | Daily, monthly, quarterly, half-yearly, and yearly totals |
| **LLM token costs** | Per-cloud model rates (Bedrock, Azure OpenAI, Vertex) |
| **Price caching** | Local SQLite cache; on-demand fetch per estimate line (live calculate) |
| **Live pricing** | Optional fresh API fetch for selected services/regions only |
| **Export** | CSV, Excel (XLSX), or PDF |
| **CLI** | `sync`, `serve`, `search`, `status`, `estimate` — cross-platform |

---

## How it works (high level)

```
┌──────────────────────────────────────────────────────────────────┐
│  UI (static/) — 50/50 layout · search · estimate sidebar         │
└────────────────────────────┬─────────────────────────────────────┘
                             │ HTTP (REST)
┌────────────────────────────▼─────────────────────────────────────┐
│  Axum API (src/api.rs) + CLI (src/cli.rs)                        │
│  /api/catalog/search · /api/sync · /api/estimate · /api/export   │
└────────────────────────────┬─────────────────────────────────────┘
                             │
     ┌───────────────────────┼───────────────────────┐
     ▼                       ▼                       ▼
 estimate.rs            provider_catalog         export.rs
 (cost engine)          (FTS search)             (CSV/XLSX/PDF)
     │                       │                       │
     └───────────────────────┼───────────────────────┘
                             ▼
                      SQLite (nimbusbill.db)
           provider_services · price_cache · token_price_cache
                             ▲
                             │ catalog + LLM sync (Tokio); prices on calculate
              ┌──────────────┴──────────────┐
              │  sync/catalog/  AWS·Azure·GCP │
              │  sync/llm_catalog.rs          │
              └─────────────────────────────┘
```

**Three ways to run:** **CLI** (`nimbusbill`), **desktop** (Tauri + WebView), or **web** (`nimbusbill serve`). All share the same Rust core, UI, and API.

**Desktop:** Tauri starts Axum on `127.0.0.1:<ephemeral port>` and loads it in the system WebView (WebView2 / WKWebView / WebKitGTK).

**Web:** Axum on `http://127.0.0.1:8080` — open in any browser.

---

## Quick start

### Prerequisites

| Requirement | Purpose |
|-------------|---------|
| [Rust](https://rustup.rs/) 1.77+ | Build the application |
| **MSVC Build Tools** (Windows) | Native dependencies (SQLite, etc.) |
| **WebView2** (Windows 10/11) | Desktop app rendering (usually preinstalled) |
| **Tauri CLI** (one-time) | Build/run the desktop shell |

Install Tauri CLI once:

```powershell
cargo install tauri-cli --version "^2.0" --locked
```

No Node.js or npm required.

---

## How to run

### Option A — Windows desktop app (recommended)

Development (live reload):

```powershell
cd C:\Users\358033\Workspace\Adhoc\Scripts\CosrEstimator
.\run-desktop.ps1
```

Or directly:

```powershell
cargo tauri dev
```

A **NimbusBill** window opens with the full UI.

**Build for distribution:**

```powershell
.\build-desktop.ps1
# or: cargo tauri build
```

| Output | Path |
|--------|------|
| Portable executable | `src-tauri\target\release\nimbusbill-desktop.exe` |
| Windows installer (MSI) | `src-tauri\target\release\bundle\msi\NimbusBill_0.1.0_x64_en-US.msi` |

End users double-click the MSI or EXE — no Rust or terminal needed.

**Desktop data location:** `%APPDATA%\NimbusBill\data\nimbusbill.db` (see [CLI](#cli) for macOS/Linux paths)

---

### Option B — Web UI in a browser

For developers or server-style deployment:

```powershell
.\run-web.ps1
# or: cargo run --bin nimbusbill -- serve
# or: cargo run --bin nimbusbill-web
```

Linux / macOS:

```bash
chmod +x run-web.sh
./run-web.sh
```

Open **http://127.0.0.1:8080**

**Database file:** platform app-data dir by default (see below). Override with `NIMUSBILL_DB`.

---

## CLI

Cross-platform command-line interface (`nimbusbill`):

```bash
cargo run --bin nimbusbill -- sync                    # catalog + LLM models (not bulk infra prices)
cargo run --bin nimbusbill -- sync --concurrency 32
cargo run --bin nimbusbill -- serve --addr 0.0.0.0:8080
cargo run --bin nimbusbill -- search redis --provider aws
cargo run --bin nimbusbill -- status
cargo run --bin nimbusbill -- estimate examples/sample-workload.yaml --live
cargo run --bin nimbusbill -- estimate workload.yaml --export pdf -o report.pdf
```

| Variable | Effect |
|----------|--------|
| `NIMUSBILL_DB` | SQLite path |
| `NIMUSBILL_CONCURRENCY` | Parallel HTTP workers (default: ~CPU count) |
| `NIMUSBILL_ADDR` | Web bind address |
| `GCP_PRICING_API_KEY` | GCP catalog + prices |

**Default DB locations:** `%APPDATA%\NimbusBill\data\` (Windows), `~/Library/Application Support/NimbusBill/` (macOS), `~/.local/share/nimbusbill/` (Linux).

Sync uses **parallel async HTTP** (Tokio workers), not multi-process — SQLite stays single-writer and safe on all platforms.

---

## Using the UI (summary)

**Layout:** configuration on the left; **Your Estimate** sidebar on the right (chips, calculate, download, results).

1. **Select cloud providers** — AWS, Azure, GCP (one or more)
2. **Use live price capture** — fetch fresh prices for your estimate only; leave off for fast cached calculate
3. **Add infrastructure** — Search or Category → Service → Region → SKU → **Add Service**
4. **LLM costs (optional)** — enable token section, pick model (auto-filtered by region), enter monthly tokens
5. **Calculate Costs** — view tables per provider in the sidebar (infrastructure, tokens, combined)
6. **Download** — **CSV**, **Excel**, or **PDF** (after calculate)

**Sync catalog** refreshes the service list and LLM models. Infrastructure unit prices are populated when you calculate with live pricing enabled once.

See [docs/user-guide.md](docs/user-guide.md) for step-by-step details.

---

## Documentation index

| Document | Contents |
|----------|----------|
| [docs/architecture.md](docs/architecture.md) | Modules, data flow, database schema, pricing sync |
| [docs/api.md](docs/api.md) | REST API reference |
| [docs/user-guide.md](docs/user-guide.md) | Full UI walkthrough |
| [docs/development.md](docs/development.md) | Project layout, testing, extending the catalog |

---

## Project structure

```
CosrEstimator/              # repo folder
├── src/                    # Rust library (core logic)
│   ├── api.rs              # Axum HTTP server
│   ├── cli.rs              # CLI (sync, serve, estimate, search)
│   ├── paths.rs            # Cross-platform DB paths
│   ├── estimate.rs         # Cost calculation engine
│   ├── export.rs           # CSV / XLSX / PDF generation
│   ├── db/                 # SQLite, provider_services + FTS
│   ├── pricing/            # AWS, Azure, GCP adapters
│   └── sync/               # Catalog + LLM sync; live price fetch on calculate
│       ├── llm_catalog.rs
│       ├── parallel.rs
│       └── catalog/        # AWS/Azure/GCP metadata ingestion
├── static/                 # Web UI (HTML, CSS, JS)
├── src-tauri/              # Tauri desktop shell (cross-platform)
├── src/bin/
│   ├── nimbusbill.rs       # CLI binary
│   └── serve.rs            # Legacy web binary
├── examples/               # Sample YAML workloads
├── docs/                   # Architecture, API, user guide
├── run-desktop.ps1         # Start desktop app (dev)
├── build-desktop.ps1       # Build MSI / EXE
├── run-web.ps1             # Web server (Windows)
└── run-web.sh              # Web server (Linux/macOS)
```

---

## Supported clouds & service categories

**Cloud providers:** AWS, Azure, GCP

**Infrastructure categories:**

- Compute (VMs, serverless, Kubernetes)
- Storage (object, block)
- Database (relational, NoSQL)
- Networking (load balancers, CDN)
- Security (WAF, key management)
- AI/ML (ML platforms)

**LLM models (examples):** GPT-4o, GPT-4o Mini, Claude Sonnet, Bedrock Claude, Gemini 1.5 Pro

> Catalog sync from public AWS/Azure/GCP APIs into SQLite. Infrastructure **unit prices** are cached on live calculate, not bulk-synced. See [docs/architecture.md](docs/architecture.md).

---

## Tech stack

| Layer | Technology |
|-------|------------|
| Language | Rust 2021 |
| HTTP server | Axum 0.8 |
| Database | SQLite (rusqlite, bundled) + FTS5 |
| Desktop | Tauri 2 (WebView2 / WKWebView / WebKitGTK) |
| CLI | Clap 4 |
| UI | Vanilla HTML / CSS / JavaScript |
| Export | csv, rust_xlsxwriter, printpdf |
| Async runtime | Tokio (parallel sync) |

---

## License

See repository license file (if present). Version **0.1.0**.
