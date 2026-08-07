# Documentation index

Welcome to the **NimbusBill** documentation.

| Guide | Audience | Description |
|-------|----------|-------------|
| [../README.md](../README.md) | Everyone | Product overview, quick start, CLI |
| [architecture.md](architecture.md) | Developers | System design, selective pricing, database |
| [api.md](api.md) | Integrators | REST API reference |
| [user-guide.md](user-guide.md) | End users | UI walkthrough (sidebar, sync, live pricing) |
| [development.md](development.md) | Contributors | Build, test, extend catalog sync |

---

## Quick links

### Run

| Platform | Desktop | Web / API |
|----------|---------|-----------|
| Windows | `.\run-desktop.ps1` or `cargo tauri dev` | `.\run-web.ps1` |
| Linux / macOS | `cargo tauri dev` | `./run-web.sh` |
| Any (CLI) | — | `cargo run --bin nimbusbill -- serve` |

### CLI

```bash
nimbusbill sync              # catalog metadata + LLM models (manual force sync)
nimbusbill serve             # web UI + API
nimbusbill search redis      # FTS catalog search
nimbusbill status            # last sync per provider
nimbusbill estimate workload.yaml [--live] [--export pdf -o out.pdf]
```

Build the CLI: `cargo build --bin nimbusbill`

### Build installer (Windows)

`.\build-desktop.ps1` or `cargo tauri build`

Tauri also targets **Linux** and **macOS** — same codebase, platform WebView.

---

## Key concepts

| Topic | Summary |
|-------|---------|
| **Sync catalog** | Refreshes service list + LLM models. Does **not** bulk-download infra unit prices. |
| **Live price capture** | On Calculate: fetches prices for your selected lines only, caches them in SQLite. |
| **Cached calculate** | Live pricing off → instant, uses cache; $0 if never live-fetched. |
| **Bedrock / LLM** | Token models under LLM / Token Usage — not Infrastructure Services. |

---

## Environment variables

| Variable | Purpose |
|----------|---------|
| `NIMUSBILL_DB` | SQLite database path |
| `NIMUSBILL_CONCURRENCY` | Parallel HTTP workers during catalog sync |
| `NIMUSBILL_ADDR` | Web server bind address |
| `GCP_PRICING_API_KEY` | GCP Cloud Billing Catalog API |
| `RUST_LOG` | e.g. `nimbusbill=debug` |
