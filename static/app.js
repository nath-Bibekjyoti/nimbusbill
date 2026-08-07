let catalog = null;
const resources = [];
const tokenUsage = [];
let lastEstimate = null;
let searchPick = null;
let searchTimer = null;
let availableCloudRegions = [];
let currentCloudProvider = "";

const $ = (id) => document.getElementById(id);

const GEO_AREAS = [
  "North America",
  "South America",
  "Europe",
  "Asia Pacific",
  "Middle East",
  "Africa",
  "Global",
  "Global / Other",
];

const PROVIDER_LABELS = { aws: "AWS", azure: "Azure", gcp: "GCP" };
const LLM_CLOUDS = ["aws", "azure", "gcp"];
const PERIOD_COLS = ["daily", "monthly", "quarterly", "half_yearly", "yearly"];
const PERIOD_LABELS = {
  daily: "Daily",
  monthly: "Monthly",
  quarterly: "Quarterly",
  half_yearly: "Half-Yearly",
  yearly: "Yearly",
};

function normProvider(p) {
  return String(p || "").toLowerCase();
}

function providerSelected(p, selected) {
  return selected.includes(normProvider(p));
}

const UNIT_USAGE = {
  hours: { label: "Hours / month", hint: "Billable hours per resource per month (730 ≈ always on)." },
  "gb-month": { label: "GB / month", hint: "Storage per resource per month." },
  "million-invocations": { label: "Million invocations / month", hint: "Invocations per resource per month." },
};

function usageLabels(unit) {
  const cfg = UNIT_USAGE[unit];
  if (cfg) return cfg;
  return { label: `Quantity (${unit})`, hint: `Usage per resource per month (${unit}).` };
}

function geoAreaForRegion(provider, region) {
  const p = normProvider(provider);
  const r = String(region || "").toLowerCase();
  if (!r) return "Global / Other";

  if (p === "aws") {
    if (/^(us-|ca-|mx-)/.test(r)) return "North America";
    if (/^sa-/.test(r)) return "South America";
    if (/^eu-/.test(r)) return "Europe";
    if (/^(ap-|ap[0-9]-)/.test(r)) return "Asia Pacific";
    if (/^(me-|il-)/.test(r)) return "Middle East";
    if (/^af-/.test(r)) return "Africa";
    return "Global / Other";
  }

  if (p === "gcp") {
    if (r === "global") return "Global";
    if (/^(us-|northamerica-)/.test(r)) return "North America";
    if (/^southamerica-/.test(r)) return "South America";
    if (/^europe-/.test(r)) return "Europe";
    if (/^(asia-|australia-)/.test(r)) return "Asia Pacific";
    if (/^me-/.test(r)) return "Middle East";
    if (/^africa-/.test(r)) return "Africa";
    return "Global / Other";
  }

  if (p === "azure") {
    if (r === "global") return "Global";
    if (/^(eastus|westus|centralus|northcentralus|southcentralus|westus2|westus3|eastus2|canada|mexico|usgov|unitedstates)/.test(r)) {
      return "North America";
    }
    if (/brazil/.test(r)) return "South America";
    if (/europe|france|germany|switzerland|uk|sweden|poland|norway|italy|spain|netherlands|belgium|austria|finland|denmark|ireland/.test(r)) {
      return "Europe";
    }
    if (/asia|japan|korea|india|australia|southeast|eastasia|centralindia|malaysia|indonesia|taiwan|hongkong|newzealand/.test(r)) {
      return "Asia Pacific";
    }
    if (/uae|qatar|israel|jio|saudi|bahrain/.test(r)) return "Middle East";
    if (/southafrica/.test(r)) return "Africa";
    return "Global / Other";
  }

  return "Global / Other";
}

function geoAreasForRegions(provider, cloudRegions) {
  const areas = new Set(cloudRegions.map((code) => geoAreaForRegion(provider, code)));
  return GEO_AREAS.filter((area) => areas.has(area));
}

function cloudRegionsInGeo(provider, cloudRegions, geoArea) {
  return cloudRegions
    .filter((code) => geoAreaForRegion(provider, code) === geoArea)
    .sort((a, b) => a.localeCompare(b));
}

function resetSubRegionSelect() {
  const subSel = $("sub-region-select");
  subSel.innerHTML = '<option value="">— Cloud region code —</option>';
  subSel.disabled = true;
}

function populateGeoRegionSelect(provider, cloudRegions) {
  const regionSel = $("region-select");
  regionSel.innerHTML = '<option value="">— Geographic area —</option>';
  resetSubRegionSelect();

  const areas = geoAreasForRegions(provider, cloudRegions);
  areas.forEach((area) => {
    const o = document.createElement("option");
    o.value = area;
    o.textContent = area;
    regionSel.appendChild(o);
  });
  regionSel.disabled = areas.length === 0;
  if (areas.length === 1) regionSel.selectedIndex = 1;
}

function currentProvider() {
  if (searchPick) return normProvider(searchPick.provider);
  return normProvider($("cloud-select").value);
}

async function init() {
  try {
    const res = await fetch("/api/catalog");
    catalog = await res.json();
    populateCategories();
    populateLlmCloudSelect();
    syncLlmCloudFromInfra();
    bindEvents();
    bindSearch();
    updateUnitFields("hours");
    updateSidebarState();
    refreshSyncStatus();
  } catch (e) {
    $("status").textContent = "Failed to load catalog.";
    $("status").className = "status error";
  }
}

async function refreshSyncStatus() {
  const el = $("sync-status");
  if (!el) return;
  try {
    const res = await fetch("/api/sync/status");
    if (!res.ok) throw new Error("status failed");
    const body = await res.json();
    const rows = Array.isArray(body) ? body : body.sync || [];

    const SYNC_LABELS = { aws: "AWS", azure: "Azure", gcp: "GCP", llm: "LLM", catalog: "Catalog" };
    const SYNC_ORDER = ["aws", "azure", "gcp", "llm", "catalog"];
    const byProvider = Object.fromEntries(rows.map((r) => [normProvider(r.provider), r]));

    const lines = SYNC_ORDER.map((key) => {
      const row = byProvider[key];
      if (!row?.synced_at) return null;
      const label = SYNC_LABELS[key] || key.toUpperCase();
      const when = new Date(row.synced_at).toLocaleString();
      const cls = row.status === "ok" ? "ok" : "err";
      return `<div class="${cls}">${label} last update: ${when}</div>`;
    }).filter(Boolean);

    if (!lines.length && body.catalog_last_updated) {
      const when = new Date(body.catalog_last_updated).toLocaleString();
      lines.push(`<div class="ok">Catalog last update: ${when}</div>`);
    }

    if (!lines.length) {
      el.textContent = "Not synced yet";
      return;
    }
    el.innerHTML = lines.join("");
  } catch {
    el.textContent = "Sync status unavailable.";
  }
}

let syncPollTimer = null;

function stopSyncPoll() {
  if (syncPollTimer) {
    clearInterval(syncPollTimer);
    syncPollTimer = null;
  }
}

function startSyncPoll() {
  stopSyncPoll();
  let ticks = 0;
  syncPollTimer = setInterval(async () => {
    ticks += 1;
    await refreshSyncStatus();
    if (ticks % 3 === 0) {
      try {
        const catRes = await fetch("/api/catalog");
        if (catRes.ok) {
          catalog = await catRes.json();
          populateCategories();
          populateLlmCloudSelect();
          syncLlmCloudFromInfra();
          onLlmCloudChange();
          const catId = $("category-select").value;
          if (catId) onCategoryChange();
        }
      } catch {
        /* ignore transient reload errors */
      }
    }
    if (ticks >= 120) stopSyncPoll();
  }, 5000);
}

function bindSearch() {
  const input = $("service-search");
  const list = $("search-results");
  if (!input || !list) return;

  input.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchPick = null;
    const q = input.value.trim();
    if (q.length < 2) {
      list.classList.add("hidden");
      list.innerHTML = "";
      return;
    }
    searchTimer = setTimeout(() => runSearch(q), 120);
  });

  input.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      list.classList.add("hidden");
    }
  });

  document.addEventListener("click", (e) => {
    if (!list.contains(e.target) && e.target !== input) {
      list.classList.add("hidden");
    }
  });
}

async function runSearch(q) {
  const list = $("search-results");
  const providers = getSelectedProviders();
  const provider = providers.length === 1 ? providers[0] : "";
  const url = `/api/catalog/search?q=${encodeURIComponent(q)}&limit=25${provider ? `&provider=${provider}` : ""}`;

  try {
    const res = await fetch(url);
    if (!res.ok) throw new Error("search failed");
    const hits = await res.json();
    list.innerHTML = "";
    if (!hits.length) {
      list.innerHTML = '<li class="empty">No matches</li>';
      list.classList.remove("hidden");
      return;
    }
    const allowed = new Set(getSelectedProviders());
    const filtered = providers.length !== 1 ? hits.filter((h) => allowed.has(normProvider(h.provider))) : hits;
    if (!filtered.length) {
      list.innerHTML = '<li class="empty">No matches for selected cloud providers</li>';
      list.classList.remove("hidden");
      return;
    }
    filtered.forEach((hit) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      li.innerHTML = `<strong>${hit.display_name}</strong><div class="meta">${PROVIDER_LABELS[hit.provider] || hit.provider} · ${hit.category_label} · ${hit.regions.length} region(s)</div>`;
      li.addEventListener("click", () => selectSearchHit(hit));
      list.appendChild(li);
    });
    list.classList.remove("hidden");
  } catch {
    list.innerHTML = '<li class="empty">Search failed</li>';
    list.classList.remove("hidden");
  }
}

async function selectSearchHit(hit) {
  searchPick = hit;
  $("service-search").value = hit.display_name;
  $("search-results").classList.add("hidden");
  updateUnitFields(hit.unit);

  currentCloudProvider = normProvider(hit.provider);
  availableCloudRegions = hit.regions.slice();
  populateGeoRegionSelect(currentCloudProvider, availableCloudRegions);
  $("add-service-btn").disabled = true;
  if ($("region-select").value) await onGeoRegionChange();
}

function formatRegionLabel(geoArea, cloudRegion) {
  if (geoArea && cloudRegion) return `${geoArea} · ${cloudRegion}`;
  return cloudRegion || geoArea || "";
}

function fmtRate(val) {
  if (val == null || val === "") return null;
  const n = parseFloat(val);
  return isNaN(n) ? null : n.toFixed(2);
}

function llmOptionLabel(m) {
  const inRate = fmtRate(m.input_per_mtok);
  const outRate = fmtRate(m.output_per_mtok);
  const rates =
    inRate != null && outRate != null
      ? ` — $${inRate}/M in · $${outRate}/M out`
      : "";
  return `${m.label}${rates}`;
}

function populateLlmCloudSelect(candidates) {
  const list = candidates?.length ? candidates : LLM_CLOUDS;
  const sel = $("llm-cloud-select");
  const prev = sel.value;
  sel.innerHTML = '<option value="">— Select cloud —</option>';
  list.forEach((p) => {
    const opt = document.createElement("option");
    opt.value = p;
    opt.textContent = PROVIDER_LABELS[p];
    sel.appendChild(opt);
  });
  if (prev && [...sel.options].some((o) => o.value === prev)) {
    sel.value = prev;
  } else if (list.length === 1) {
    sel.value = list[0];
  }
}

function infraProvidersForLlm() {
  const aiMl = resources.filter((r) => r.categoryId === "ai_ml");
  const source = aiMl.length ? aiMl : resources;
  const fromInfra = [...new Set(source.map((r) => r.provider))].filter((p) =>
    LLM_CLOUDS.includes(p)
  );
  if (fromInfra.length) return fromInfra;
  return getSelectedProviders().filter((p) => LLM_CLOUDS.includes(p));
}

function llmRegionsForCloud(cloud) {
  return [
    ...new Set(resources.filter((r) => r.provider === cloud).map((r) => r.region)),
  ];
}

function modelAvailableInRegions(model, regions) {
  if (!regions.length || !model.regions?.length) return true;
  return model.regions.some((r) => regions.includes(r));
}

function syncLlmCloudFromInfra() {
  if (!$("enable-llm").checked) return;
  const candidates = infraProvidersForLlm();
  populateLlmCloudSelect(candidates);
  const sel = $("llm-cloud-select");
  const auto = $("llm-cloud-auto");
  const field = $("llm-cloud-field");
  if (candidates.length === 1) {
    sel.value = candidates[0];
    field.classList.add("hidden");
    auto.textContent = `Cloud: ${PROVIDER_LABELS[candidates[0]]} (from infrastructure)`;
    auto.classList.remove("hidden");
  } else {
    field.classList.remove("hidden");
    auto.classList.add("hidden");
    if (candidates.length && !candidates.includes(sel.value)) {
      sel.value = candidates[0];
    }
  }
  onLlmCloudChange();
}

function onLlmCloudChange() {
  const cloud = $("llm-cloud-select").value;
  const sel = $("llm-model");
  sel.innerHTML = '<option value="">— Select model —</option>';
  sel.disabled = !cloud;
  if (!cloud || !catalog?.llm_models) return;

  const regions = llmRegionsForCloud(cloud);
  const models = catalog.llm_models
    .filter((m) => m.provider === cloud)
    .filter((m) => modelAvailableInRegions(m, regions));

  if (!models.length) {
    const opt = document.createElement("option");
    opt.value = "";
    opt.textContent = regions.length
      ? "No models for selected region(s) — Sync catalog"
      : "No models — Sync catalog";
    sel.appendChild(opt);
    return;
  }

  models.forEach((m) => {
      const opt = document.createElement("option");
      opt.value = m.id;
      opt.textContent = llmOptionLabel(m);
      opt.dataset.provider = m.provider;
      opt.dataset.label = m.label;
      if (m.input_per_mtok) opt.dataset.inputRate = m.input_per_mtok;
      if (m.output_per_mtok) opt.dataset.outputRate = m.output_per_mtok;
      sel.appendChild(opt);
    });
}

function populateLlmModels() {
  syncLlmCloudFromInfra();
}

function populateCategories() {
  const sel = $("category-select");
  sel.innerHTML = '<option value="">— Select category —</option>';
  if (catalog?.categories) {
    catalog.categories.forEach((cat) => {
      const opt = document.createElement("option");
      opt.value = cat.id;
      opt.textContent = cat.label;
      sel.appendChild(opt);
    });
  }
}

function bindEvents() {
  $("category-select").addEventListener("change", onCategoryChange);
  $("service-select").addEventListener("change", onServiceChange);
  $("cloud-select").addEventListener("change", onCloudChange);
  $("region-select").addEventListener("change", onGeoRegionChange);
  $("sub-region-select").addEventListener("change", onCloudRegionChange);
  $("sku-select").addEventListener("change", updateAddServiceButton);
  $("add-service-btn").addEventListener("click", addService);
  $("llm-cloud-select").addEventListener("change", onLlmCloudChange);
  $("add-llm-btn").addEventListener("click", addLlm);
  $("add-custom-llm-btn").addEventListener("click", addCustomLlm);
  $("enable-llm").addEventListener("change", (e) => {
    $("llm-panel").classList.toggle("hidden", !e.target.checked);
    if (e.target.checked) syncLlmCloudFromInfra();
  });
  $("calculate-btn").addEventListener("click", calculate);
  $("sync-btn")?.addEventListener("click", triggerSync);
  $("import-file")?.addEventListener("change", onImportFile);
  document.querySelectorAll(".btn.export").forEach((btn) => {
    btn.addEventListener("click", () => downloadReport(btn.dataset.format));
  });
}

async function onImportFile(e) {
  const file = e.target.files?.[0];
  if (!file) return;
  const status = $("status");
  status.textContent = `Loading ${file.name}…`;
  status.className = "status";
  try {
    const fd = new FormData();
    fd.append("file", file);
    const res = await fetch("/api/import", { method: "POST", body: fd });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    applyImport(data);
    const warn = data.warnings?.length ? ` (${data.warnings.length} warning(s))` : "";
    status.textContent = `Loaded ${data.resources.length} service(s) from ${file.name}${warn}.`;
    status.className = data.warnings?.length ? "status" : "status ok";
    if (data.warnings?.length) console.warn("Import warnings:", data.warnings);
  } catch {
    status.textContent = "Could not load workload file.";
    status.className = "status error";
  } finally {
    e.target.value = "";
  }
}

function applyImport(data) {
  searchPick = null;
  resources.length = 0;
  tokenUsage.length = 0;
  (data.resources || []).forEach((r) => resources.push(r));
  (data.token_usage || []).forEach((t) => {
    const cloud = t.cloud_provider || t.provider;
    tokenUsage.push({
      key: `${cloud}-${t.model}`,
      cloud_provider: cloud,
      model: t.model,
      provider: t.provider,
      display_name: t.display_name || t.model,
      input_tokens_per_month: t.input_tokens_per_month,
      output_tokens_per_month: t.output_tokens_per_month,
    });
  });
  if (data.providers?.length) {
    document.querySelectorAll('input[name="provider"]').forEach((cb) => {
      cb.checked = data.providers.includes(cb.value);
    });
  }
  $("live-pricing").checked = !!data.live_pricing;
  const hasLlm = (data.token_usage || []).length > 0;
  $("enable-llm").checked = hasLlm;
  $("llm-panel").classList.toggle("hidden", !hasLlm);
  renderResourceChips();
  renderLlmChips();
  syncLlmCloudFromInfra();
}

async function triggerSync() {
  const btn = $("sync-btn");
  const status = $("status");
  btn.disabled = true;
  status.textContent = "Starting sync…";
  status.className = "status";
  try {
    const res = await fetch("/api/sync", { method: "POST" });
    if (!res.ok) throw new Error("sync failed");
    status.textContent = "Sync running (catalog + LLM models — check status above).";
    status.className = "status ok";
    startSyncPoll();
    await refreshSyncStatus();
  } catch {
    status.textContent = "Could not start sync.";
    status.className = "status error";
  } finally {
    btn.disabled = false;
  }
}

function onCategoryChange() {
  searchPick = null;
  availableCloudRegions = [];
  currentCloudProvider = "";
  const catId = $("category-select").value;
  const serviceSel = $("service-select");
  serviceSel.innerHTML = '<option value="">— Select service —</option>';
  $("cloud-select").innerHTML = '<option value="">— Select cloud —</option>';
  $("region-select").innerHTML = '<option value="">— Geographic area —</option>';
  $("cloud-select").disabled = true;
  $("region-select").disabled = true;
  resetSubRegionSelect();
  resetSkuSelect();
  $("add-service-btn").disabled = true;
  updateUnitFields("");

  if (!catId) {
    serviceSel.disabled = true;
    return;
  }

  const cat = catalog.categories.find((c) => c.id === catId);
  const selectedProviders = getSelectedProviders();
  cat.services
    .filter((svc) => svc.providers.some((p) => providerSelected(p.provider, selectedProviders)))
    .forEach((svc) => {
    const opt = document.createElement("option");
    opt.value = svc.id;
    opt.textContent = `${svc.name} (${PROVIDER_LABELS[svc.providers[0]?.provider] || svc.providers[0]?.provider})`;
    opt.dataset.unit = svc.unit;
    opt.dataset.category = catId;
    opt.dataset.categoryLabel = cat.label;
    serviceSel.appendChild(opt);
  });
  serviceSel.disabled = false;
}

function onServiceChange() {
  const svcId = $("service-select").value;
  const cloudSel = $("cloud-select");
  cloudSel.innerHTML = '<option value="">— Select cloud —</option>';
  $("region-select").innerHTML = '<option value="">— Geographic area —</option>';
  $("region-select").disabled = true;
  resetSubRegionSelect();
  availableCloudRegions = [];
  currentCloudProvider = "";
  $("add-service-btn").disabled = true;

  if (!svcId) {
    cloudSel.disabled = true;
    return;
  }

  const svc = findService(svcId);
  updateUnitFields(svc.unit);

  const selectedProviders = getSelectedProviders();
  svc.providers
    .filter((p) => providerSelected(p.provider, selectedProviders))
    .forEach((p) => {
      const opt = document.createElement("option");
      opt.value = normProvider(p.provider);
      opt.textContent = PROVIDER_LABELS[p.provider] || p.provider;
      opt.dataset.regions = JSON.stringify(p.regions);
      cloudSel.appendChild(opt);
    });

  cloudSel.disabled = cloudSel.options.length <= 1;
  if (cloudSel.options.length === 2) {
    cloudSel.selectedIndex = 1;
    onCloudChange();
  }
}

async function onCloudChange() {
  const cloudSel = $("cloud-select");
  resetSkuSelect();
  $("add-service-btn").disabled = true;

  if (!cloudSel.value) {
    availableCloudRegions = [];
    currentCloudProvider = "";
    $("region-select").innerHTML = '<option value="">— Geographic area —</option>';
    $("region-select").disabled = true;
    resetSubRegionSelect();
    return;
  }

  currentCloudProvider = normProvider(cloudSel.value);
  const opt = cloudSel.selectedOptions[0];
  availableCloudRegions = JSON.parse(opt.dataset.regions || "[]");
  populateGeoRegionSelect(currentCloudProvider, availableCloudRegions);
  await onGeoRegionChange();
}

async function onGeoRegionChange() {
  const geoArea = $("region-select").value;
  const subSel = $("sub-region-select");
  $("add-service-btn").disabled = true;
  resetSkuSelect();

  subSel.innerHTML = '<option value="">— Cloud region code —</option>';
  if (!geoArea || !availableCloudRegions.length) {
    subSel.disabled = true;
    return;
  }

  const provider = currentProvider();
  const codes = cloudRegionsInGeo(provider, availableCloudRegions, geoArea);
  codes.forEach((code) => {
    const o = document.createElement("option");
    o.value = code;
    o.textContent = code;
    subSel.appendChild(o);
  });
  subSel.disabled = codes.length === 0;
  if (codes.length === 1) subSel.selectedIndex = 1;
  await onCloudRegionChange();
}

async function onCloudRegionChange() {
  const cloudRegion = $("sub-region-select").value;
  $("add-service-btn").disabled = true;
  if (!cloudRegion) {
    resetSkuSelect();
    return;
  }

  if (searchPick) {
    await loadSkuOptions(searchPick.catalog_id, searchPick.provider, cloudRegion, searchPick.default_sku);
    updateAddServiceButton();
    return;
  }

  const svcId = $("service-select").value;
  const provider = $("cloud-select").value;
  const entry = findService(svcId)?.providers.find((p) => normProvider(p.provider) === normProvider(provider));
  await loadSkuOptions(svcId, provider, cloudRegion, entry?.default_sku);
  updateAddServiceButton();
}

function updateAddServiceButton() {
  const geoArea = $("region-select").value;
  const cloudRegion = $("sub-region-select").value;
  const sku = $("sku-select").value;
  const ready = searchPick
    ? geoArea && cloudRegion && sku
    : $("category-select").value &&
      $("service-select").value &&
      $("cloud-select").value &&
      geoArea &&
      cloudRegion &&
      sku;
  $("add-service-btn").disabled = !ready;
}

function resetSkuSelect() {
  const skuSel = $("sku-select");
  skuSel.innerHTML = '<option value="">— Select configuration —</option>';
  skuSel.disabled = true;
}

function ensureSkuOption(skuSel, fallback) {
  if (!fallback) return false;
  if (![...skuSel.options].some((o) => o.value === fallback)) {
    const o = document.createElement("option");
    o.value = fallback;
    o.textContent = fallback;
    skuSel.appendChild(o);
  }
  if (!skuSel.value) skuSel.value = fallback;
  skuSel.disabled = false;
  return !!skuSel.value;
}

async function loadSkuOptions(catalogId, provider, region, defaultSku) {
  const skuSel = $("sku-select");
  resetSkuSelect();
  if (!catalogId || !provider || !region) return false;

  skuSel.disabled = true;
  try {
    const url = `/api/catalog/skus?catalog_id=${encodeURIComponent(catalogId)}&provider=${encodeURIComponent(provider)}&region=${encodeURIComponent(region)}&live=1`;
    const res = await fetch(url);
    if (!res.ok) throw new Error("sku load failed");
    const data = await res.json();
    if (data.unit) updateUnitFields(data.unit);

    const options =
      data.sku_options?.length > 0
        ? data.sku_options
        : (data.skus || (defaultSku ? [defaultSku] : [])).map((sku) => ({ value: sku, label: sku }));

    options.forEach((opt) => {
      const o = document.createElement("option");
      o.value = opt.value;
      o.textContent = opt.label || opt.value;
      skuSel.appendChild(o);
    });

    const defaultVal = data.default_sku || defaultSku;
    if (defaultVal && [...skuSel.options].some((o) => o.value === defaultVal)) {
      skuSel.value = defaultVal;
    } else if (skuSel.options.length === 2) {
      skuSel.selectedIndex = 1;
    }

    if (skuSel.options.length <= 1) {
      ensureSkuOption(skuSel, defaultVal || defaultSku);
    } else {
      skuSel.disabled = false;
    }
    return !!skuSel.value;
  } catch {
    ensureSkuOption(skuSel, defaultSku);
    return !!skuSel.value;
  }
}

function updateUnitFields(unit) {
  const { label, hint } = usageLabels(unit || "hours");
  $("usage-label").textContent = label;
  $("usage-hint").textContent = hint;
}

function readUsageInputs(unit) {
  const count = parseInt($("count-input").value, 10);
  const usage = parseInt($("usage-input").value, 10);
  if (isNaN(count) || count <= 0 || isNaN(usage) || usage < 0) return null;
  return {
    count,
    usage,
    quantity: count * usage,
    instance_count: count,
    hours: unit === "hours" ? usage : null,
  };
}

function showAddServiceError(msg) {
  const status = $("status");
  status.textContent = msg;
  status.className = "status error";
}

function findService(id) {
  for (const cat of catalog.categories) {
    const svc = cat.services.find((s) => s.id === id);
    if (svc) return svc;
  }
  return null;
}

function getSelectedProviders() {
  return [...document.querySelectorAll('input[name="provider"]:checked')].map((el) => el.value);
}

function estimateProviders() {
  const selected = getSelectedProviders();
  if (!$("enable-llm").checked) return selected;
  const set = new Set(selected);
  tokenUsage.forEach((t) => {
    if (t.cloud_provider) set.add(t.cloud_provider);
  });
  return [...set];
}

function isLlmTokenCatalogId(catalogId) {
  return String(catalogId || "").includes("AmazonBedrockFoundationModels");
}

function addService() {
  const sku = $("sku-select").value;
  const geoArea = $("region-select").value;
  const cloudRegion = $("sub-region-select").value;
  const status = $("status");

  if (searchPick) {
    if (!geoArea) {
      showAddServiceError("Select a region (e.g. Europe, North America).");
      return;
    }
    if (!cloudRegion) {
      showAddServiceError("Select a location (cloud region).");
      return;
    }
    if (!sku) {
      showAddServiceError("Select a configuration (SKU) before adding a service.");
      return;
    }
    const unit = searchPick.unit;
    const usage = readUsageInputs(unit);
    if (!usage) {
      showAddServiceError("Resource count must be at least 1.");
      return;
    }

    if (isLlmTokenCatalogId(searchPick.catalog_id)) {
      showAddServiceError("Bedrock is token-priced — enable LLM / Token Usage and add a model there.");
      return;
    }

    const key = `${searchPick.catalog_id}-${searchPick.provider}-${cloudRegion}-${sku}`;
    const entry = {
      key,
      catalog_id: searchPick.catalog_id,
      provider: searchPick.provider,
      region: cloudRegion,
      sub_region: geoArea,
      sku,
      instance_count: usage.count,
      hours: usage.hours,
      quantity: usage.quantity,
      name: searchPick.display_name,
      category: searchPick.category_label,
      categoryId: searchPick.category_id,
      unit,
    };
    const existing = resources.findIndex((r) => r.key === key);
    if (existing >= 0) resources[existing] = entry;
    else resources.push(entry);
    renderResourceChips();
    status.textContent = `Added ${entry.name}.`;
    status.className = "status ok";
    return;
  }

  const catId = $("category-select").value;
  const svcId = $("service-select").value;
  const provider = $("cloud-select").value;

  if (!catId || !svcId || !provider || !geoArea || !cloudRegion) {
    showAddServiceError("Complete category, service, cloud, region, and location before adding.");
    return;
  }
  if (isLlmTokenCatalogId(svcId)) {
    showAddServiceError("Bedrock is token-priced — enable LLM / Token Usage and add a model there.");
    return;
  }
  if (!sku) {
    showAddServiceError("Select a configuration (SKU) before adding a service.");
    return;
  }

  const svc = findService(svcId);
  const cat = catalog.categories.find((c) => c.id === catId);
  const usage = readUsageInputs(svc.unit);
  if (!usage) {
    showAddServiceError("Resource count must be at least 1.");
    return;
  }

  const key = `${svcId}-${provider}-${cloudRegion}-${sku}`;

  const existing = resources.findIndex((r) => r.key === key);
  const entry = {
    key,
    catalog_id: svcId,
    provider,
    region: cloudRegion,
    sub_region: geoArea,
    sku,
    instance_count: usage.count,
    hours: usage.hours,
    quantity: usage.quantity,
    name: svc.name,
    category: cat.label,
    categoryId: catId,
    unit: svc.unit,
  };

  if (existing >= 0) resources[existing] = entry;
  else resources.push(entry);

  renderResourceChips();
  syncLlmCloudFromInfra();
  status.textContent = `Added ${entry.name}.`;
  status.className = "status ok";
}

function addLlm() {
  const cloud = $("llm-cloud-select").value;
  const sel = $("llm-model");
  if (!cloud || !sel.value) return;

  const opt = sel.selectedOptions[0];
  const key = `${cloud}-${sel.value}`;
  const entry = {
    key,
    cloud_provider: cloud,
    model: sel.value,
    provider: cloud,
    display_name: opt.dataset.label,
    input_tokens_per_month: parseInt($("input-tokens").value, 10) || 0,
    output_tokens_per_month: parseInt($("output-tokens").value, 10) || 0,
  };

  const idx = tokenUsage.findIndex((t) => t.key === key);
  if (idx >= 0) tokenUsage[idx] = entry;
  else tokenUsage.push(entry);

  const cb = document.querySelector(`input[name="provider"][value="${cloud}"]`);
  if (cb) cb.checked = true;

  renderLlmChips();
}

async function addCustomLlm() {
  const id = $("custom-model-id").value.trim();
  const label = $("custom-model-label").value.trim();
  const provider = $("custom-model-provider").value.trim() || "custom";
  const input_per_mtok = $("custom-input-rate").value;
  const output_per_mtok = $("custom-output-rate").value;
  const status = $("status");

  if (!id || !label) {
    status.textContent = "Model ID and display name are required.";
    status.className = "status error";
    return;
  }

  status.textContent = "Saving custom model…";
  status.className = "status";

  try {
    const res = await fetch("/api/llm-models", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        id,
        label,
        provider,
        input_per_mtok,
        output_per_mtok,
      }),
    });
    if (!res.ok) throw new Error(await res.text());
    catalog = await res.json();
    populateLlmCloudSelect();
    $("custom-model-provider").value = provider;
    $("llm-cloud-select").value = provider;
    onLlmCloudChange();
    $("llm-model").value = id;
    status.textContent = `Saved custom model "${label}".`;
    status.className = "status ok";
  } catch {
    status.textContent = "Failed to save custom model.";
    status.className = "status error";
  }
}

function updateSidebarState() {
  const hasServices = resources.length > 0;
  const hasLlm = tokenUsage.length > 0;
  $("sidebar-empty").classList.toggle("hidden", hasServices || hasLlm);
  $("sidebar-services").classList.toggle("hidden", !hasServices);
  $("sidebar-llm").classList.toggle("hidden", !hasLlm);
}

function renderResourceChips() {
  const container = $("selected-services");
  container.innerHTML = "";
  resources.forEach((r, i) => {
    const usageLabel =
      r.unit === "hours"
        ? `${r.instance_count} × ${r.hours} h/mo`
        : `${r.instance_count} × ${r.quantity / r.instance_count} ${r.unit}/mo`;
    const regionLabel = formatRegionLabel(r.sub_region, r.region);
    const chip = document.createElement("span");
    chip.className = "chip";
    chip.innerHTML = `
      <span class="cat-badge cat-${r.categoryId}">${r.category}</span>
      <strong>${r.name}</strong> · ${r.sku} · ${regionLabel} · ${usageLabel}
      <button class="remove" data-i="${i}" title="Remove">&times;</button>`;
    chip.querySelector(".remove").addEventListener("click", () => {
      resources.splice(i, 1);
      renderResourceChips();
    });
    container.appendChild(chip);
  });
  updateSidebarState();
  syncLlmCloudFromInfra();
}

function renderLlmChips() {
  const container = $("selected-llm");
  container.innerHTML = "";
  tokenUsage.forEach((t, i) => {
    const meta = catalog?.llm_models?.find(
      (m) => m.id === t.model && m.provider === t.cloud_provider
    );
    const rateHint =
      meta?.input_per_mtok && meta?.output_per_mtok
        ? ` · $${fmtRate(meta.input_per_mtok)}/M in · $${fmtRate(meta.output_per_mtok)}/M out`
        : "";
    const chip = document.createElement("span");
    chip.className = "chip";
    chip.innerHTML = `
      <span class="cat-badge cat-ai_ml">${PROVIDER_LABELS[t.cloud_provider] || t.cloud_provider}</span>
      <strong>${t.display_name}</strong>${rateHint} · in ${fmtNum(t.input_tokens_per_month)} / out ${fmtNum(t.output_tokens_per_month)}
      <button class="remove" data-i="${i}">&times;</button>`;
    chip.querySelector(".remove").addEventListener("click", () => {
      tokenUsage.splice(i, 1);
      renderLlmChips();
    });
    container.appendChild(chip);
  });
  updateSidebarState();
}

function providerHasEstimateData(pe) {
  return (
    (pe.infrastructure?.rows?.length ?? 0) > 0 ||
    (pe.tokens?.rows?.length ?? 0) > 0
  );
}

function fmtUsageQty(r) {
  let text;
  if (r.usage_display) {
    text = r.usage_display;
  } else if (r.unit === "hours") {
    text = `${fmtNum(r.quantity)} h`;
  } else {
    text = `${fmtNum(r.quantity)} ${r.unit}`;
  }
  return text
    .replace(/million-invocations/g, "M invocations")
    .replace(/gb-month/g, "GB-mo");
}

function fmtNum(n) {
  return Number(n).toLocaleString();
}

function fmtMoney(val) {
  const n = parseFloat(val);
  if (isNaN(n)) return "—";
  return "$" + n.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

async function calculate() {
  const providers = estimateProviders();
  const status = $("status");
  status.className = "status";

  if (!providers.length) {
    status.textContent = "Select at least one cloud provider, or add an LLM model.";
    status.className = "status error";
    return;
  }
  const enableLlm = $("enable-llm").checked;
  if (!resources.length && !(enableLlm && tokenUsage.length)) {
    status.textContent = enableLlm
      ? "Add at least one infrastructure service, or add an LLM model."
      : "Add at least one infrastructure service (LLM is optional).";
    status.className = "status error";
    return;
  }

  const live = $("live-pricing").checked;

  const body = {
    name: "ui-estimate",
    providers,
    live_pricing: live,
    resources: resources.map((r) => ({
      catalog_id: r.catalog_id,
      provider: r.provider,
      region: r.region,
      sub_region: r.sub_region,
      sku: r.sku,
      instance_count: r.instance_count != null ? String(r.instance_count) : undefined,
      hours: r.hours != null ? String(r.hours) : undefined,
      quantity: String(r.quantity),
    })),
    token_usage: enableLlm
      ? tokenUsage.map((t) => ({
          model: t.model,
          provider: t.provider,
          cloud_provider: t.cloud_provider,
          display_name: t.display_name,
          input_tokens_per_month: t.input_tokens_per_month,
          output_tokens_per_month: t.output_tokens_per_month,
        }))
      : [],
  };

  status.textContent = live ? "Fetching live prices…" : "Calculating…";
  $("calculate-btn").disabled = true;

  try {
    const res = await fetch("/api/estimate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    lastEstimate = data;
    $("export-bar").classList.remove("hidden");
    renderResults(data);
    status.textContent = "Estimate complete.";
    status.className = "status ok";
  } catch (e) {
    status.textContent = "Estimate failed. Check your selections.";
    status.className = "status error";
  } finally {
    $("calculate-btn").disabled = false;
  }
}

function renderResults(data) {
  const container = $("results");
  container.innerHTML = "";

  const active = data.providers.filter(providerHasEstimateData);
  if (!active.length) {
    container.innerHTML = '<p class="hint">No cost data for the selected providers.</p>';
    return;
  }

  active.forEach((pe) => {
    const block = document.createElement("div");
    block.className = "provider-block";

    const liveBadge = data.live_pricing ? '<span class="live-badge">LIVE PRICES</span>' : "";
    block.innerHTML = `<h3>${PROVIDER_LABELS[pe.provider] || pe.provider}${liveBadge}</h3>`;

    block.appendChild(buildTableSection("Infrastructure Costs", pe.infrastructure, false));
    if (pe.tokens?.rows?.length) {
      block.appendChild(
        buildTableSection(
          `LLM / Token Costs (${PROVIDER_LABELS[pe.provider] || pe.provider})`,
          pe.tokens,
          false
        )
      );
      block.appendChild(buildCombinedSection(pe.combined));
    }

    container.appendChild(block);
  });
}

function buildTableSection(title, table, isCombined) {
  const section = document.createElement("div");
  section.className = "table-section" + (isCombined ? " combined-table" : "");

  const headers = isCombined
    ? PERIOD_COLS.map((p) => `<th>${PERIOD_LABELS[p]}</th>`).join("")
    : `<th>Category</th><th>Service</th><th>Unit Price</th><th>Usage</th>` +
      PERIOD_COLS.map((p) => `<th>${PERIOD_LABELS[p]}</th>`).join("");

  let bodyRows = "";
  if (!table.rows.length) {
    bodyRows = `<tr class="empty-row"><td colspan="${isCombined ? 5 : 9}">No items</td></tr>`;
  } else if (isCombined) {
    bodyRows = `<tr><td>Total</td>${periodCells(table.totals || table)}</tr>`;
  } else {
    bodyRows = table.rows
      .map(
        (r) => {
          const zero = parseFloat(r.unit_price) === 0;
          return `<tr class="${zero ? "zero-price" : ""}">
            <td>${r.category}</td>
            <td class="cell-service">${r.service}${r.description ? `<br><span class="hint-inline">${r.description}</span>` : ""}${zero ? '<br><span class="hint-inline warn">No cached price — enable Use live price capture, Calculate once (needs pricing API access), then turn it off for fast cached estimates</span>' : ""}</td>
            <td>${fmtMoney(r.unit_price)}</td>
            <td class="cell-usage">${fmtUsageQty(r)}</td>
            ${periodCells(r.costs)}
          </tr>`;
        }
      )
      .join("");
  }

  const foot = isCombined
    ? ""
    : `<tfoot><tr><td colspan="4">Subtotal</td>${periodCells(table.totals)}</tr></tfoot>`;

  section.innerHTML = `
    ${title ? `<h4>${title}</h4>` : ""}
    <div class="table-wrap">
      <table>
        <thead><tr>${isCombined ? "<th></th>" + headers : headers}</tr></thead>
        <tbody>${bodyRows}</tbody>
        ${foot}
      </table>
    </div>`;
  return section;
}

function buildCombinedSection(combined) {
  const section = document.createElement("div");
  section.className = "table-section combined-table";
  section.innerHTML = `
    <h4>Total Cost (Infrastructure + Tokens)</h4>
    <div class="table-wrap">
      <table>
        <thead><tr><th></th>${PERIOD_COLS.map((p) => `<th>${PERIOD_LABELS[p]}</th>`).join("")}</tr></thead>
        <tbody><tr><td><strong>Grand Total</strong></td>${periodCells(combined)}</tr></tbody>
      </table>
    </div>`;
  return section;
}

function periodCells(costs) {
  return PERIOD_COLS.map((p) => `<td>${fmtMoney(costs[p])}</td>`).join("");
}

async function downloadReport(format) {
  if (!lastEstimate) return;
  const status = $("status");
  document.querySelectorAll(".btn.export").forEach((b) => (b.disabled = true));
  status.textContent = `Generating ${format.toUpperCase()}…`;

  try {
    const res = await fetch("/api/export", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ format, estimate: lastEstimate }),
    });
    if (!res.ok) throw new Error("export failed");

    const blob = await res.blob();
    const disposition = res.headers.get("Content-Disposition") || "";
    const match = disposition.match(/filename="(.+)"/);
    const filename = match ? match[1] : `nimbusbill-estimate.${format}`;

    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
    status.textContent = `Downloaded ${filename}`;
    status.className = "status ok";
  } catch {
    status.textContent = "Export failed.";
    status.className = "status error";
  } finally {
    document.querySelectorAll(".btn.export").forEach((b) => (b.disabled = false));
  }
}

// Re-filter cloud options when provider checkboxes change
document.querySelectorAll('input[name="provider"]').forEach((el) => {
  el.addEventListener("change", () => {
    if ($("category-select").value) onCategoryChange();
    if ($("service-select").value) onServiceChange();
    syncLlmCloudFromInfra();
  });
});

init();
