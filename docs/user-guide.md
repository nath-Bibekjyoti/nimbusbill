# User guide

Step-by-step instructions for using **NimbusBill**.

---

## Starting the application

### Desktop (recommended)

1. Install via MSI, or run from source: `.\run-desktop.ps1` (Windows) / `cargo tauri dev` (any OS)
2. The **NimbusBill** window opens with the full UI

### Browser

1. Run `.\run-web.ps1` (Windows) or `./run-web.sh` (Linux/macOS)
2. Open **http://127.0.0.1:8080**

### CLI (no UI)

```bash
nimbusbill serve      # same API + UI in browser
nimbusbill sync       # refresh catalog + LLM models (see below)
nimbusbill search ec2 # find services from terminal
```

---

## Main screen overview

The UI uses a **50/50 split**:

| Left column | Right column (**Your Estimate**) |
|-------------|----------------------------------|
| Cloud provider checkboxes | Selected service / LLM chips |
| Live pricing toggle | **Calculate Costs** button |
| Infrastructure builder (search or browse) | **Download** bar (CSV · Excel · PDF) |
| LLM / Token Usage (optional) | Cost result tables |

The header shows **sync status** (last update per provider) and a **Sync catalog** button.

---

## Syncing the catalog

Click **Sync catalog** in the header, or run `nimbusbill sync` from the terminal.

### What Sync catalog **does**

| Item | Refreshed? |
|------|------------|
| AWS / Azure / GCP **service list** (names, regions, SKUs metadata) | Yes |
| **LLM model catalog** (Bedrock, Azure Foundry, Vertex rates) | Yes (always on manual sync) |
| FTS search index | Yes (via service list) |

### What Sync catalog **does not** do

| Item | How to get it |
|------|----------------|
| **Infrastructure unit prices** (e.g. EC2 $/hour in `ap-southeast-1`) | Enable **Use live price capture**, click **Calculate Costs** once — prices are fetched only for your selected services/regions and cached locally |
| Bulk price download for every service | Not supported (by design — keeps sync fast) |

**First sync** may take a few minutes (hundreds of services from AWS/Azure/GCP APIs). Later syncs are faster when the catalog is still fresh (default staleness: 6 hours).

Watch the header sync status for rows like `Catalog`, `AWS`, `Azure`, `GCP`, and **LLM**.

| Provider | Requirement |
|----------|-------------|
| AWS | None (public Price List API) |
| Azure | None (public Retail Prices API) |
| GCP | `GCP_PRICING_API_KEY` environment variable |

> **Note:** A background log line `llm catalog fresh; background sync skipped` is normal between manual syncs. Clicking **Sync catalog** always refreshes LLM models.

---

## Step 1: Select cloud providers

Check one or more:

- **AWS**
- **Azure**
- **GCP**

Each provider with data gets its own results section. Empty providers (e.g. Azure selected but no Azure resources) are hidden from results.

### Use live price capture

| Setting | Behavior |
|---------|----------|
| **Unchecked** (default) | **Cache only** — instant calculate; uses prices already stored in SQLite from a previous live fetch |
| **Checked** | Fetches fresh prices for **your estimate line items only** (not a full catalog sync), caches them, then calculates |

**Recommended workflow:**

1. Leave live pricing **off** while adjusting quantities (fast iteration).
2. Turn live pricing **on**, click **Calculate Costs** once to populate the cache (requires network access to cloud pricing APIs).
3. Turn live pricing **off** again for fast cached estimates.

If infrastructure shows **$0.00** with a warning about no cached price, you need step 2 — **Sync catalog alone will not fix infra unit prices**.

---

## Step 2: Add infrastructure services

Two ways to find services:

### Option A — Search (fastest)

1. Type in the **Search services** box (e.g. `redis`, `kafka`, `ec2`)
2. Results appear after a short debounce — click a hit to pre-fill the form
3. Pick **geographic area** → **cloud region** → **configuration (SKU)**
4. Set **resource count** and usage (hours, GB-month, invocations, etc.)
5. Click **Add Service**

Search uses the synced catalog (FTS). Run **Sync catalog** if nothing appears.

### Option B — Browse by category

1. **Category** → Compute, Storage, Database, Messaging, Security, etc.
2. **Service** → filtered by category
3. **Cloud** → filtered by providers you checked
4. **Geographic area** → **Location (region)** → **Configuration (SKU)**
5. **Resource count** + usage field (label changes by unit — e.g. hours, GB-month, million invocations)

**Example:** A VM running 24×7 for a month ≈ `730` hours per resource.

Click **Add Service**. Resources appear as chips in **Your Estimate**; click **×** to remove.

> **Bedrock / LLM token APIs** (e.g. `AmazonBedrockFoundationModels`) belong under **LLM / Token Usage**, not Infrastructure Services.

---

## Step 3: LLM / token costs (optional)

1. Check **LLM / Token Usage** to expand the section
2. **Cloud** is auto-selected from your infrastructure when only one provider is in use; otherwise pick AWS (Bedrock), Azure (OpenAI), or GCP (Vertex)
3. Select a **model** — list is filtered by regions used in your infrastructure services
4. Enter **input tokens per month** and **output tokens per month**
5. Click **Add Model**

Token costs roll into the **matching cloud provider** total (not a separate global bucket).

You can add multiple models across different clouds. If the model list is empty, click **Sync catalog** (refreshes LLM rates from cloud APIs; baseline models are seeded at startup if APIs are unreachable).

---

## Step 4: Calculate

Click **Calculate Costs** in the **Your Estimate** sidebar.

Results appear below the download bar, **per cloud provider** (only providers with data):

### Infrastructure costs table

| Column | Meaning |
|--------|---------|
| Category | Service category |
| Service | Display name (+ optional description / warning) |
| Unit Price | From cache or live fetch |
| Usage | Human-readable usage (e.g. `4 × 730 h`, `2 × 1460 M invocations`) |
| Daily … Yearly | Projected cost per billing period |
| **Subtotal** | Sum of infrastructure rows |

Long text in Service and Usage columns wraps to avoid overlapping adjacent columns.

### LLM / token costs table

Same period columns, one row per model (if you added token usage).

### Total cost table

**Grand Total** = Infrastructure + Tokens for each period.

With **Use live price capture** enabled, a **LIVE PRICES** badge appears on the provider header.

---

## Step 5: Export

After calculating, the **Download** bar appears in the sidebar with three buttons:

| Button | Format | Best for |
|--------|--------|----------|
| **CSV** | Comma-separated | Spreadsheets, pipelines |
| **Excel** | Multi-sheet workbook | One sheet per provider |
| **PDF** | Printable report | Sharing, archiving |

Filenames look like `nimbusbill-estimate-20260807-143000.xlsx`.

Export buttons are hidden until you have a completed estimate.

---

## Tips

- **Compare clouds:** Add the same workload on AWS and Azure as separate entries with different Cloud selections
- **Find obscure services:** Use search (`ElastiCache`, `CloudFront`, `BigQuery`) instead of scrolling categories
- **Quick iteration:** Leave live pricing off while adjusting quantities; turn it on once to populate the price cache
- **Obscure AWS services:** Some offers (e.g. professional/security subscriptions) may have no public on-demand price in the AWS Price List API — they may stay at $0 even with live pricing
- **WSL / corporate network:** If live pricing fails, try running from Windows or ensure WSL can reach `pricing.us-east-1.amazonaws.com` and Azure retail API endpoints

---

## Where data is stored

Default SQLite path (override with `NIMUSBILL_DB`):

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\NimbusBill\data\nimbusbill.db` |
| macOS | `~/Library/Application Support/NimbusBill/nimbusbill.db` |
| Linux | `~/.local/share/nimbusbill/nimbusbill.db` |

The database holds the service catalog, cached prices (populated on live calculate), token rates, sync history, and saved estimates.

---

## Troubleshooting

| Issue | What to try |
|-------|-------------|
| Window is blank (desktop) | Wait for embedded server startup; check terminal logs |
| Search returns nothing | Run **Sync catalog** or `nimbusbill sync` |
| Infrastructure costs show $0.00 | Enable **Use live price capture**, Calculate once (needs pricing API access); Sync catalog does not load infra unit prices |
| LLM models missing / stale | Click **Sync catalog**; check header for **LLM last update** |
| Calculate very slow with live pricing off | Rebuild from latest source — cached mode should be instant (no network) |
| GCP services missing | Set `GCP_PRICING_API_KEY` and sync again |
| Export buttons missing | Calculate costs first |
| Table text overlapping | Hard-refresh the page (`Ctrl+F5`) for latest CSS |
| `cargo tauri` not found | `cargo install tauri-cli --version "^2.0" --locked` |

For developer issues, see [development.md](development.md).
