# REST API reference

Base URL depends on runtime:

| Mode | Base URL |
|------|----------|
| Desktop (Tauri) | `http://127.0.0.1:<ephemeral-port>` |
| Web / CLI serve | `http://127.0.0.1:8080` (default, set `NIMUSBILL_ADDR`) |

All JSON endpoints return JSON unless noted. No authentication in v0.1.

---

## `GET /`

Serves the main UI (`index.html`).

**Response:** `text/html`

---

## `GET /health`

Health check.

**Response:** `200` — body: `ok`

---

## `GET /api/catalog`

Returns the full service catalog (from SQLite `provider_services`) and per-cloud LLM models.

**Response:** `200`

```json
{
  "categories": [
    {
      "id": "compute",
      "label": "Compute",
      "services": [
        {
          "id": "aws:AmazonEC2",
          "name": "Ec2",
          "category": "compute",
          "unit": "hours",
          "providers": [
            {
              "provider": "aws",
              "service_key": "AmazonEC2",
              "default_sku": "t3.micro",
              "regions": ["us-east-1", "us-west-2", "eu-west-1"]
            }
          ]
        }
      ]
    }
  ],
  "llm_models": [
    {
      "id": "gpt-4o",
      "label": "GPT-4o (Azure OpenAI)",
      "provider": "azure",
      "input_per_mtok": "2.50",
      "output_per_mtok": "10.00",
      "regions": ["eastus"]
    }
  ]
}
```

Service `id` values are provider-native (`{provider}:{service_key}`) after catalog sync.

`AmazonBedrockFoundationModels` is excluded from infrastructure categories — use LLM models instead.

---

## `GET /api/catalog/search`

Fast prefix search over synced services (SQLite FTS5).

**Query parameters:**

| Param | Required | Description |
|-------|----------|-------------|
| `q` | Yes | Search text (e.g. `redis`, `kafka`, `ec2`) |
| `provider` | No | Filter: `aws`, `azure`, or `gcp` |
| `limit` | No | Max results (default 25, max 100) |

**Response:** `200` — array of hits

```json
[
  {
    "catalog_id": "aws:AmazonElastiCache",
    "provider": "aws",
    "service_key": "AmazonElastiCache",
    "display_name": "Amazon ElastiCache",
    "category_id": "database",
    "category_label": "Database",
    "unit": "hours",
    "default_sku": "cache.t3.micro",
    "regions": ["us-east-1", "eu-west-1"]
  }
]
```

---

## `GET /api/catalog/skus`

List configurable SKUs (instance types, tiers, etc.) for a service in a region.

**Query parameters:**

| Param | Required | Description |
|-------|----------|-------------|
| `catalog_id` | Yes | e.g. `aws:AmazonEC2` |
| `provider` | Yes | `aws`, `azure`, or `gcp` |
| `region` | Yes | Cloud region code |
| `live` | No | `1` or `true` to fetch fresh SKU list from cloud API |

**Response:** `200`

```json
{
  "skus": ["t3.micro", "t3.small", "m5.large"],
  "default_sku": "t3.micro"
}
```

GCP may return `{ "options": [{ "value": "...", "label": "..." }] }` instead of plain SKU strings.

---

## `POST /api/llm-models`

Add a custom LLM model with token rates (per cloud provider).

**Request:**

```json
{
  "id": "my-model",
  "label": "My Model",
  "provider": "aws",
  "input_per_mtok": "1.00",
  "output_per_mtok": "3.00"
}
```

**Response:** `200` — full updated `CatalogResponse`

---

## `POST /api/estimate`

Calculate infrastructure and token costs.

**Request:**

```json
{
  "name": "my-estimate",
  "providers": ["aws", "azure"],
  "live_pricing": false,
  "resources": [
    {
      "catalog_id": "aws:AmazonEC2",
      "provider": "aws",
      "region": "Asia Pacific",
      "sub_region": "ap-southeast-1",
      "sku": "t3.micro",
      "instance_count": "4",
      "hours": "730",
      "quantity": "2920"
    }
  ],
  "token_usage": [
    {
      "model": "bedrock-claude-sonnet",
      "provider": "aws",
      "cloud_provider": "aws",
      "display_name": "Claude Sonnet (Bedrock)",
      "input_tokens_per_month": 1000000,
      "output_tokens_per_month": 250000
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | No | Report label |
| `providers` | string[] | Yes* | `aws`, `azure`, `gcp` (*or infer from token_usage) |
| `live_pricing` | bool | No | `true`: fetch fresh prices for estimate lines only; `false`: cache only (no network) |
| `resources` | array | No | Infrastructure line items |
| `resources[].catalog_id` | string | Yes | From catalog or search |
| `resources[].provider` | string | Yes | Cloud for this resource |
| `resources[].region` | string | Yes | Geographic area label or region |
| `resources[].sub_region` | string | No | Cloud region code (e.g. `ap-southeast-1`) |
| `resources[].sku` | string | No | Configuration / instance type |
| `resources[].instance_count` | string | No | Number of resources |
| `resources[].hours` | string | No | Hours per resource per month (when unit is hours) |
| `resources[].quantity` | string | Yes | Billing quantity (decimal as string) |
| `token_usage` | array | No | LLM usage — costs roll into **matching provider block** |

**Response:** `200` — `UiEstimateResponse` with per-provider infrastructure + token tables. Rows include `usage_display` for human-readable usage text.

**Errors:** `400` — invalid request

### `live_pricing` behavior

| Value | Infrastructure | LLM tokens |
|-------|----------------|------------|
| `false` | SQLite `price_cache` only; $0 + warning on cache miss | Cached token rates |
| `true` | Live fetch per line → upsert cache → calculate (15s timeout per call) | `sync_selected()` for models in request |

---

## `POST /api/export`

Generate a downloadable report from a completed estimate.

**Request:**

```json
{
  "format": "xlsx",
  "estimate": { }
}
```

| Field | Values |
|-------|--------|
| `format` | `csv`, `xlsx`, `pdf` |
| `estimate` | Full `UiEstimateResponse` from `/api/estimate` |

**Response:** `200` — file bytes

Headers:

- `Content-Type`: appropriate MIME type
- `Content-Disposition`: `attachment; filename="nimbusbill-estimate-YYYYMMDD-HHMMSS.{ext}"`

---

## `POST /api/sync`

Trigger a **manual** catalog sync in the background (same as `nimbusbill sync` → `run_once_force`).

Refreshes:

- AWS / Azure / GCP service metadata (if forced or catalog stale)
- **LLM model catalog** (always on manual sync)

Does **not** bulk-download infrastructure unit prices.

**Request:** empty body

**Response:** `200`

```json
{ "started": true }
```

Poll `GET /api/sync/status` for completion. Sync runs asynchronously.

---

## `GET /api/sync/status`

Last sync result per provider plus catalog freshness metadata.

**Response:** `200`

```json
{
  "catalog_last_updated": "2026-08-07T14:30:00+00:00",
  "catalog_fresh": true,
  "catalog_stale_secs": 21600,
  "sync": [
    {
      "provider": "catalog",
      "status": "ok",
      "detail": "aws=212 azure=847 gcp=156 entries · aws=212svc/32reg/...",
      "synced_at": "2026-08-07T14:30:00+00:00"
    },
    {
      "provider": "llm",
      "status": "ok",
      "detail": "42 LLM models (catalog)",
      "synced_at": "2026-08-07T14:31:00+00:00"
    },
    {
      "provider": "aws",
      "status": "ok",
      "detail": "metadata only — prices fetched per service/region on calculate",
      "synced_at": "2026-08-07T14:31:00+00:00"
    }
  ]
}
```

Provider keys: `catalog`, `aws`, `azure`, `gcp`, `llm`.

---

## Static assets

| Path | Description |
|------|-------------|
| `/static/styles.css` | UI styles |
| `/static/app.js` | UI logic, search, sync status, estimates |

---

## Example: curl

```bash
# Catalog
curl "http://127.0.0.1:8080/api/catalog"

# Search
curl "http://127.0.0.1:8080/api/catalog/search?q=redis&provider=aws&limit=10"

# SKUs for EC2 in Singapore
curl "http://127.0.0.1:8080/api/catalog/skus?catalog_id=aws:AmazonEC2&provider=aws&region=ap-southeast-1"

# Sync status
curl "http://127.0.0.1:8080/api/sync/status"

# Trigger sync (async)
curl -X POST "http://127.0.0.1:8080/api/sync"

# Estimate (cached pricing)
curl -X POST "http://127.0.0.1:8080/api/estimate" \
  -H "Content-Type: application/json" \
  -d '{"providers":["aws"],"resources":[{"catalog_id":"aws:AmazonEC2","provider":"aws","region":"Asia Pacific","sub_region":"ap-southeast-1","sku":"t3.micro","instance_count":"1","hours":"730","quantity":"730"}]}'

# Estimate (live pricing — fetches and caches unit prices)
curl -X POST "http://127.0.0.1:8080/api/estimate" \
  -H "Content-Type: application/json" \
  -d '{"providers":["aws"],"live_pricing":true,"resources":[{"catalog_id":"aws:AmazonEC2","provider":"aws","region":"Asia Pacific","sub_region":"ap-southeast-1","sku":"t3.micro","quantity":"730"}]}'
```

PowerShell uses `` ` `` for line continuation instead of `\`.

---

## CLI equivalent

| API | CLI |
|-----|-----|
| `POST /api/sync` | `nimbusbill sync` |
| `GET /api/sync/status` | `nimbusbill status` |
| `GET /api/catalog/search` | `nimbusbill search redis --provider aws` |
| Serve UI + API | `nimbusbill serve` |
| Workload file | `nimbusbill estimate examples/sample-workload.yaml [--live]` |

See [development.md](development.md) for YAML workload format (`src/input.rs`).
