import { api, apiBytes, connect, enc, events, inTauri, mcpServerPath, openPath, openUrl } from "./api";

// ---------- types ----------

interface Instance {
  id: string;
  name: string;
  mc_version: string;
  loader: "vanilla" | "fabric" | "neoforge";
  loader_version: string | null;
  installed: boolean;
  running: boolean;
  game_dir: string;
  mods_dir: string;
  ram_mb: number | null;
  live?: LiveSettings;
}

interface LiveSettings {
  hotswap: boolean;
  content_slots: boolean;
  in_memory_textures: boolean;
  unfreeze_registries: boolean;
  slot_count: number;
}

const liveOn = (i: Instance) => !!i.live && (i.live.hotswap || i.live.content_slots || i.live.unfreeze_registries);

interface ModEntry {
  file_name: string;
  enabled: boolean;
  mod_id: string | null;
  name: string | null;
  version: string | null;
  description: string | null;
  authors: string[];
  loader: string;
  size: number;
  claude_project: string | null;
  online: { source: Source; project_id: string; title: string; page_url: string; as_dependency: boolean } | null;
}

type Source = "modrinth" | "curseforge";

interface Hit {
  source: Source;
  project_id: string;
  slug: string;
  title: string;
  description: string;
  author: string;
  downloads: number;
  icon_url: string | null;
  page_url: string;
  categories: string[];
  installed: boolean;
}

interface ModVersion {
  version_id: string;
  name: string;
  version_number: string;
  date: string;
  channel: string;
  file: { url: string | null; filename: string; size: number };
  dependencies: { project_id: string; required: boolean }[];
}

interface Project {
  id: string;
  name: string;
  description: string;
  instance_id: string;
  loader: string;
  mc_version: string;
  path: string;
  last_build: { success: boolean; at: number; jar: string | null } | null;
}

interface Account {
  id: string;
  name: string;
  selected: boolean;
}

type Tab = "mods" | "browse" | "projects" | "console";

// ---------- state ----------

const S = {
  instances: [] as Instance[],
  selected: null as string | null,
  tab: "mods" as Tab,
  mods: [] as ModEntry[],
  projects: [] as Project[],
  accounts: [] as Account[],
  hasClientId: false,
  hasCfKey: false,
  browse: {
    instance: null as string | null,
    source: "modrinth" as Source,
    q: "",
    sort: "relevance",
    hits: [] as Hit[],
    total: 0,
    loading: false,
    error: "",
  },
  running: new Set<string>(),
  busy: new Set<string>(),
  modFilter: "",
  makeIdea: "",
  crash: null as null | {
    instance_id: string;
    at: number;
    exit_code: number | null;
    report_path: string | null;
    description: string | null;
    exception: string | null;
    launcher_log: string;
    suspects: { mod_id: string; name: string; claude_project: string | null }[];
  },
  updates: { instance: null as string | null, byFile: new Map<string, { title: string; latest: ModVersion }>(), checked: false },
  logs: new Map<string, string[]>(),
  progress: null as null | { task: string; current: number; total: number; message: string },
};

const LOG_LIMIT = 3000;

// ---------- helpers ----------

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector(sel) as T;

function esc(s: unknown): string {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function loaderLabel(l: string): string {
  return l === "neoforge" ? "NeoForge" : l === "fabric" ? "Fabric" : "Vanilla";
}

function fmtSize(b: number): string {
  if (b > 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(b / 1024))} KB`;
}

function ago(ts: number): string {
  const s = Math.floor(Date.now() / 1000) - ts;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

function toast(msg: string, kind: "ok" | "err" = "ok") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = msg;
  $("#toasts").appendChild(el);
  setTimeout(() => el.classList.add("out"), kind === "err" ? 7000 : 3500);
  setTimeout(() => el.remove(), kind === "err" ? 7500 : 4000);
}

async function busy<T>(key: string, fn: () => Promise<T>): Promise<T | undefined> {
  if (S.busy.has(key)) return;
  S.busy.add(key);
  renderMain();
  try {
    return await fn();
  } catch (e) {
    toast((e as Error).message, "err");
  } finally {
    S.busy.delete(key);
    renderMain();
  }
}

const current = () => S.instances.find((i) => i.id === S.selected) ?? null;

// ---------- data ----------

async function loadInstances() {
  S.instances = await api<Instance[]>("GET", "/instances");
  S.instances.forEach((i) => (i.running ? S.running.add(`game:${i.id}`) : S.running.delete(`game:${i.id}`)));
  if (!S.selected || !S.instances.some((i) => i.id === S.selected)) S.selected = S.instances[0]?.id ?? null;
  renderSidebar();
  await loadInstanceData();
}

async function loadInstanceData() {
  const inst = current();
  if (inst) {
    const [mods, projects, crash] = await Promise.all([
      api<ModEntry[]>("GET", `/instances/${enc(inst.id)}/mods`),
      api<Project[]>("GET", "/projects"),
      api("GET", `/instances/${enc(inst.id)}/crash`).catch(() => null),
    ]);
    S.mods = mods;
    S.projects = projects;
    S.crash = crash;
  } else {
    S.mods = [];
  }
  renderMain();
}

async function loadAccounts() {
  const [accounts, status] = await Promise.all([api<Account[]>("GET", "/accounts"), api("GET", "/status")]);
  S.accounts = accounts;
  S.hasClientId = status.has_client_id;
  S.hasCfKey = status.has_curseforge_key;
  (status.running as string[]).forEach((r) => S.running.add(r));
  renderSidebar();
}

// ---------- sidebar ----------

function renderSidebar() {
  const list = $("#instance-list");
  list.innerHTML = S.instances.length
    ? S.instances
        .map(
          (i) => `
      <button class="inst ${i.id === S.selected ? "active" : ""}" data-action="select" data-id="${esc(i.id)}">
        <span class="inst-icon ${esc(i.loader)}">${esc(i.loader[0].toUpperCase())}</span>
        <span class="inst-text">
          <span class="inst-name">${esc(i.name)}</span>
          <span class="inst-meta">${esc(i.mc_version)} · ${loaderLabel(i.loader)}</span>
        </span>
        ${S.running.has(`game:${i.id}`) ? '<span class="dot" title="Running"></span>' : ""}
      </button>`,
        )
        .join("")
    : `<p class="side-empty">No instances yet.</p>`;

  const acc = S.accounts.find((a) => a.selected) ?? S.accounts[0];
  $("#side-foot").innerHTML = `
    ${
      acc
        ? `<button class="account" data-action="accounts"><img src="https://mc-heads.net/avatar/${esc(acc.id)}/32" alt="" width="24" height="24" onerror="this.style.visibility='hidden'"/><span>${esc(acc.name)}</span></button>`
        : `<button class="account signin" data-action="login">Sign in with Microsoft</button>`
    }
    <button class="icon-btn" data-action="settings" title="Settings" aria-label="Settings">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>
    </button>`;
}

// ---------- main panel ----------

function renderMain() {
  const main = $("#main");
  const inst = current();
  if (!inst) {
    main.innerHTML = `
      <div class="empty">
        <img src="/icon.png" alt="" width="72" height="72"/>
        <h1>Make your first instance</h1>
        <p>Pick a Minecraft version and a mod loader. Then ask Claude Code to make a mod for it, and it'll show up here.</p>
        <button class="btn primary" data-action="new-instance">New instance</button>
      </div>`;
    return;
  }
  const running = S.running.has(`game:${inst.id}`);
  const launching = S.busy.has(`launch:${inst.id}`);
  const playLabel = running ? "Stop" : launching ? "Starting…" : inst.installed ? "Play" : "Install & play";
  main.innerHTML = `
    <header class="inst-head">
      <div>
        <h1>${esc(inst.name)}</h1>
        <div class="chips">
          <span class="chip">Minecraft ${esc(inst.mc_version)}</span>
          <span class="chip ${esc(inst.loader)}">${loaderLabel(inst.loader)}${inst.loader_version ? ` ${esc(inst.loader_version)}` : ""}</span>
          ${inst.installed ? "" : '<span class="chip warn">Not installed</span>'}
          ${liveOn(inst) ? `<span class="chip live" title="Live mode is on for the next launch">⚡ Live${inst.live!.unfreeze_registries ? " · experimental" : ""}</span>` : ""}
        </div>
      </div>
      <div class="head-actions">
        <button class="btn ghost" data-action="open-path" data-path="${esc(inst.game_dir)}" title="Open the game folder">Folder</button>
        <button class="btn ghost" data-action="instance-menu" title="Instance settings">Edit</button>
        ${inst.loader !== "vanilla" ? '<button class="btn ghost" data-action="export-pack" title="Share as a Modrinth modpack">Export</button>' : ""}
        <button class="btn ${running ? "danger" : "play"}" data-action="${running ? "stop" : "play"}" ${launching ? "disabled" : ""}>${playLabel}</button>
      </div>
    </header>
    ${crashBanner(inst)}
    <div class="tabs" role="tablist">
      ${(["mods", "browse", "projects", "console"] as Tab[])
        .map((t) => {
          const label =
            t === "mods" ? `Mods <span class="count">${S.mods.length}</span>`
            : t === "browse" ? "Browse"
            : t === "projects" ? `Claude projects <span class="count">${projectsFor(inst).length}</span>`
            : "Console";
          return `<button role="tab" class="tab ${S.tab === t ? "active" : ""}" data-action="tab" data-tab="${t}">${label}</button>`;
        })
        .join("")}
    </div>
    <section class="tab-body">${S.tab === "mods" ? modsTab(inst) : S.tab === "browse" ? browseTab(inst) : S.tab === "projects" ? projectsTab(inst) : consoleTab(inst)}</section>`;
  if (S.tab === "browse" && S.browse.instance !== inst.id && !S.browse.loading) runSearch(false);
  if (S.tab === "console") {
    const c = $("#console");
    c.scrollTop = c.scrollHeight;
  }
  for (const [id, val] of [["mod-filter", S.modFilter], ["browse-q", S.browse.q]] as const) {
    const el = document.querySelector<HTMLInputElement>(`#${id}`);
    if (el && val && refocus === id) {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }
  }
  refocus = null;
}

let refocus: string | null = null;

function crashBanner(inst: Instance): string {
  const c = S.crash;
  if (!c || c.instance_id !== inst.id || S.running.has(`game:${inst.id}`)) return "";
  const mine = c.suspects.filter((x) => x.claude_project);
  const others = c.suspects.filter((x) => !x.claude_project);
  const who = c.suspects.length
    ? `Involved: ${[...mine.map((x) => `${esc(x.name)} <span class="badge claude">✦ yours</span>`), ...others.map((x) => esc(x.name))].join(", ")}`
    : "";
  return `
    <div class="crash" role="alert">
      <div class="crash-text">
        <strong>The game crashed ${ago(c.at)}${c.description ? `: ${esc(c.description)}` : ""}</strong>
        ${c.exception ? `<code>${esc(c.exception)}</code>` : ""}
        ${who ? `<div class="small">${who}</div>` : ""}
      </div>
      <div class="crash-actions">
        <button class="btn primary claude-btn" data-action="crash-ask" ${S.busy.has("claude-open") ? "disabled" : ""}>✦ Ask Claude to fix</button>
        ${c.report_path ? `<button class="btn ghost small" data-action="open-path" data-path="${esc(c.report_path.replace(/[\\/][^\\/]+$/, ""))}">Crash reports</button>` : `<button class="btn ghost small" data-action="tab" data-tab="console">Console</button>`}
        <button class="icon-btn" data-action="crash-dismiss" title="Dismiss" aria-label="Dismiss">×</button>
      </div>
    </div>`;
}

function updateFor(inst: Instance, m: ModEntry) {
  if (S.updates.instance !== inst.id || !m.online) return null;
  return S.updates.byFile.get(m.file_name.replace(/\.disabled$/, "")) ?? null;
}

function shortVer(v: string): string {
  return v.length > 18 ? `${v.slice(0, 16)}…` : v;
}

function updatesButton(inst: Instance): string {
  if (!S.mods.some((m) => m.online)) return "";
  const n = S.updates.instance === inst.id ? S.updates.byFile.size : 0;
  if (S.busy.has("check-updates")) return '<button class="btn ghost" disabled>Checking…</button>';
  if (S.busy.has("update-all")) return '<button class="btn primary" disabled>Updating…</button>';
  if (n > 0) return `<button class="btn primary" data-action="update-all">Update all (${n})</button>`;
  if (S.updates.instance === inst.id && S.updates.checked) return '<button class="btn ghost" data-action="check-updates" title="Check again">Up to date ✓</button>';
  return '<button class="btn ghost" data-action="check-updates">Check for updates</button>';
}

async function checkUpdates() {
  const inst = current();
  if (!inst) return;
  await busy("check-updates", async () => {
    const r = await api<{ updates: { file_name: string; title: string; latest: ModVersion }[]; warnings: string[] }>("GET", `/instances/${enc(inst.id)}/updates`);
    S.updates = { instance: inst.id, byFile: new Map(r.updates.map((u) => [u.file_name, u])), checked: true };
    r.warnings.forEach((w) => toast(w, "err"));
    if (!r.updates.length) toast("All browser-installed mods are up to date");
  });
}

async function applyUpdates(files?: string[]) {
  const inst = current();
  if (!inst) return;
  const key = files ? `update:${files[0]}` : "update-all";
  await busy(key, async () => {
    const r = await api<{ installed: string[]; warnings: string[] }>("POST", `/instances/${enc(inst.id)}/updates`, { files });
    for (const f of files ?? [...S.updates.byFile.keys()]) S.updates.byFile.delete(f);
    r.warnings.forEach((w) => toast(w, "err"));
    if (r.installed.length) toast(`Updated ${r.installed.length} mod${r.installed.length === 1 ? "" : "s"}`);
  });
}

function projectsFor(inst: Instance) {
  return S.projects.filter((p) => p.instance_id === inst.id);
}

function modsTab(inst: Instance): string {
  if (inst.loader === "vanilla") {
    return `<div class="note">Vanilla instances can't load mods. Create a Fabric or NeoForge instance to use mods.</div>`;
  }
  const f = S.modFilter.toLowerCase();
  const mods = S.mods.filter((m) => !f || `${m.name} ${m.mod_id} ${m.file_name} ${m.description}`.toLowerCase().includes(f));
  const claudeCount = S.mods.filter((m) => m.claude_project).length;
  const rows = mods
    .map((m) => {
      const key = `mod:${m.file_name}`;
      const proj = m.claude_project;
      const building = proj && S.busy.has(`install:${proj}`);
      return `
      <div class="mod ${m.enabled ? "" : "disabled"}">
        <label class="switch" title="${m.enabled ? "Disable" : "Enable"}">
          <input type="checkbox" ${m.enabled ? "checked" : ""} data-action="toggle-mod" data-file="${esc(m.file_name)}" ${S.busy.has(key) ? "disabled" : ""}/>
          <span></span>
        </label>
        <div class="mod-text">
          <div class="mod-title">
            <span class="mod-name">${esc(m.name ?? m.mod_id ?? m.file_name)}</span>
            ${m.version ? `<span class="mod-ver">${esc(m.version)}</span>` : ""}
            ${proj ? `<span class="badge claude" title="Built by Claude from project ${esc(proj)}">✦ Made by Claude</span>` : ""}
            ${m.online ? `<button class="badge src ${m.online.source}" data-action="open-url" data-url="${esc(m.online.page_url)}" title="Open on ${sourceLabel(m.online.source)}">${sourceLabel(m.online.source)}${m.online.as_dependency ? " · dependency" : ""}</button>` : ""}
          </div>
          ${m.description ? `<div class="mod-desc">${esc(m.description)}</div>` : ""}
          <div class="mod-file">${esc(m.file_name)} · ${fmtSize(m.size)}</div>
        </div>
        <div class="mod-actions">
          ${updateFor(inst, m) ? `<button class="btn small update" data-action="update-one" data-file="${esc(m.file_name)}" ${S.busy.has(`update:${m.file_name}`) || S.busy.has("update-all") ? "disabled" : ""} title="Update to ${esc(updateFor(inst, m)!.latest.version_number)}">${S.busy.has(`update:${m.file_name}`) ? "Updating…" : `Update → ${esc(shortVer(updateFor(inst, m)!.latest.version_number))}`}</button>` : ""}
          ${proj ? `<button class="btn small" data-action="rebuild" data-project="${esc(proj)}" ${building ? "disabled" : ""}>${building ? "Building…" : "Rebuild"}</button>` : ""}
          <button class="icon-btn" data-action="delete-mod" data-file="${esc(m.file_name)}" title="Delete" aria-label="Delete mod">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/></svg>
          </button>
        </div>
      </div>`;
    })
    .join("");
  return `
    <div class="toolbar">
      <input id="mod-filter" class="input" placeholder="Search mods" value="${esc(S.modFilter)}" data-action="filter-mods"/>
      <span class="muted">${S.mods.filter((m) => m.enabled).length} enabled · ${claudeCount} by Claude</span>
      ${updatesButton(inst)}
      <button class="btn ghost" data-action="open-path" data-path="${esc(inst.mods_dir)}">Open mods folder</button>
    </div>
    ${
      S.mods.length
        ? `<div class="mod-list">${rows || '<p class="muted pad">No mods match.</p>'}</div>`
        : `<div class="note">No mods yet. Drop .jar files into the mods folder, or ask Claude Code to make one, for example: <code>Make a ${loaderLabel(inst.loader)} mod for my "${esc(inst.name)}" instance that makes ores glow</code></div>`
    }`;
}

function sourceLabel(s: Source): string {
  return s === "modrinth" ? "Modrinth" : "CurseForge";
}

function fmtCount(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 0 : 1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)}k`;
  return String(n);
}

function browseTab(inst: Instance): string {
  if (inst.loader === "vanilla") {
    return `<div class="note">Vanilla instances can't load mods. Create a Fabric or NeoForge instance to browse mods.</div>`;
  }
  const B = S.browse;
  const needsKey = B.source === "curseforge" && !S.hasCfKey;
  const results = B.hits
    .map((h) => {
      const key = `get:${h.source}:${h.project_id}`;
      const working = S.busy.has(key);
      return `
      <div class="hit">
        ${h.icon_url ? `<img class="hit-icon" src="${esc(h.icon_url)}" alt="" loading="lazy" onerror="this.replaceWith(Object.assign(document.createElement('div'),{className:'hit-icon blank'}))"/>` : '<div class="hit-icon blank"></div>'}
        <div class="mod-text">
          <div class="mod-title">
            <button class="mod-name linkish" data-action="open-url" data-url="${esc(h.page_url)}" title="Open on ${sourceLabel(h.source)}">${esc(h.title)}</button>
            <span class="muted small">by ${esc(h.author)}</span>
          </div>
          <div class="mod-desc">${esc(h.description)}</div>
          <div class="hit-meta">
            <span>↓ ${fmtCount(h.downloads)}</span>
            ${h.categories.slice(0, 3).map((c) => `<span class="tag">${esc(c)}</span>`).join("")}
          </div>
        </div>
        <div class="mod-actions">
          <button class="btn small ghost" data-action="browse-versions" data-pid="${esc(h.project_id)}" data-title="${esc(h.title)}">Versions</button>
          ${
            h.installed
              ? '<span class="badge ok">Installed</span>'
              : `<button class="btn small primary" data-action="browse-install" data-pid="${esc(h.project_id)}" ${working ? "disabled" : ""}>${working ? "Installing…" : "Install"}</button>`
          }
        </div>
      </div>`;
    })
    .join("");
  return `
    <div class="toolbar wrap">
      <div class="seg small-seg">
        <button data-action="browse-source" data-v="modrinth" class="${B.source === "modrinth" ? "on" : ""}">Modrinth</button>
        <button data-action="browse-source" data-v="curseforge" class="${B.source === "curseforge" ? "on" : ""}">CurseForge</button>
      </div>
      <input id="browse-q" class="input" placeholder="Search ${sourceLabel(B.source)} mods for ${esc(inst.mc_version)} ${loaderLabel(inst.loader)}" value="${esc(B.q)}" data-action="browse-q" ${needsKey ? "disabled" : ""}/>
      <select id="browse-sort" class="input sort" data-action="browse-sort" ${needsKey ? "disabled" : ""}>
        ${[["relevance", "Relevance"], ["downloads", "Downloads"], ["updated", "Recently updated"], ["newest", "Newest"]]
          .map(([v, l]) => `<option value="${v}" ${B.sort === v ? "selected" : ""}>${l}</option>`)
          .join("")}
      </select>
    </div>
    ${
      needsKey
        ? `<div class="note">CurseForge needs a free API key. Get one at <button class="link" data-action="open-url" data-url="https://console.curseforge.com/">console.curseforge.com</button>, then add it in <button class="link" data-action="settings">Settings</button>.</div>`
        : B.error
          ? `<div class="note err">${esc(B.error)}</div>`
          : B.hits.length
            ? `<p class="muted small">Showing mods for Minecraft ${esc(inst.mc_version)} with ${loaderLabel(inst.loader)}. Required dependencies install automatically.</p>
               <div class="mod-list">${results}</div>
               ${B.hits.length < B.total ? `<button class="btn ghost more" data-action="browse-more" ${B.loading ? "disabled" : ""}>${B.loading ? "Loading…" : "Load more"}</button>` : ""}`
            : `<p class="muted pad">${B.loading ? "Searching…" : "No mods found."}</p>`
    }`;
}

let searchSeq = 0;
async function runSearch(more: boolean) {
  const inst = current();
  if (!inst || inst.loader === "vanilla") return;
  const B = S.browse;
  if (B.source === "curseforge" && !S.hasCfKey) {
    B.instance = inst.id;
    B.hits = [];
    renderMain();
    return;
  }
  const seq = ++searchSeq;
  B.loading = true;
  B.error = "";
  if (!more) B.instance = inst.id;
  renderMain();
  try {
    const page = await api<{ hits: Hit[]; total: number }>(
      "GET",
      `/browse/${B.source}/search?instance_id=${enc(inst.id)}&q=${enc(B.q)}&sort=${B.sort}&offset=${more ? B.hits.length : 0}&limit=20`,
    );
    if (seq !== searchSeq) return;
    B.hits = more ? [...B.hits, ...page.hits] : page.hits;
    B.total = page.total;
  } catch (e) {
    if (seq !== searchSeq) return;
    B.error = (e as Error).message;
    if (!more) B.hits = [];
  } finally {
    if (seq === searchSeq) {
      B.loading = false;
      renderMain();
    }
  }
}

async function installOnline(pid: string, versionId?: string) {
  const inst = current();
  if (!inst) return;
  const source = S.browse.source;
  await busy(`get:${source}:${pid}`, async () => {
    const r = await api<{ installed: string[]; already_installed: string[]; warnings: string[] }>(
      "POST",
      `/instances/${enc(inst.id)}/online-mods`,
      { source, project_id: pid, version_id: versionId },
    );
    const hit = S.browse.hits.find((h) => h.project_id === pid);
    if (hit) hit.installed = true;
    const deps = r.installed.length - 1;
    if (r.installed.length) toast(`Installed ${hit?.title ?? r.installed[0]}${deps > 0 ? ` and ${deps} dependenc${deps === 1 ? "y" : "ies"}` : ""}`);
    else toast(`${hit?.title ?? "That mod"} is already installed`);
    r.warnings.forEach((w) => toast(w, "err"));
  });
}

async function versionsModal(pid: string, title: string) {
  const inst = current();
  if (!inst) return;
  const source = S.browse.source;
  openModal(`<h2>${esc(title)}</h2><p class="muted">Loading versions…</p>`);
  try {
    const vs = await api<ModVersion[]>("GET", `/browse/${source}/projects/${enc(pid)}/versions?instance_id=${enc(inst.id)}`);
    openModal(`
      <h2>${esc(title)}</h2>
      <p class="muted small">Versions for Minecraft ${esc(inst.mc_version)} with ${loaderLabel(inst.loader)}</p>
      <div class="versions">
        ${
          vs.length
            ? vs
                .slice(0, 40)
                .map(
                  (v) => `
          <div class="ver-row">
            <div class="grow">
              <div><strong>${esc(v.version_number || v.name)}</strong> ${v.channel !== "release" ? `<span class="badge live">${esc(v.channel)}</span>` : ""}</div>
              <div class="mod-file">${esc(v.file.filename)} · ${v.date ? esc(v.date.slice(0, 10)) : ""}${v.dependencies.some((d) => d.required) ? ` · ${v.dependencies.filter((d) => d.required).length} required dependenc${v.dependencies.filter((d) => d.required).length === 1 ? "y" : "ies"}` : ""}</div>
            </div>
            ${
              v.file.url
                ? `<button class="btn small" data-action="install-version" data-pid="${esc(pid)}" data-vid="${esc(v.version_id)}">Install</button>`
                : `<span class="muted small" title="The author only allows downloads from CurseForge">Manual download</span>`
            }
          </div>`,
                )
                .join("")
            : '<p class="muted">No versions for this instance.</p>'
        }
      </div>
      <div class="modal-actions"><button class="btn ghost" data-action="close-modal">Close</button></div>`);
  } catch (e) {
    closeModal();
    toast((e as Error).message, "err");
  }
}

function projectsTab(inst: Instance): string {
  const projects = projectsFor(inst);
  const rows = projects
    .map((p) => {
      const dev = S.running.has(`dev:${p.id}`);
      const building = S.busy.has(`install:${p.id}`) || S.running.has(`build:${p.id}`);
      const lb = p.last_build;
      return `
      <div class="project">
        <div class="project-text">
          <div class="mod-title">
            <span class="mod-name">${esc(p.name)}</span>
            <span class="mod-ver">${esc(p.id)}</span>
            ${lb ? `<span class="badge ${lb.success ? "ok" : "fail"}">${lb.success ? "Built" : "Build failed"} ${ago(lb.at)}</span>` : '<span class="badge">Not built yet</span>'}
            ${dev ? '<span class="badge live">Dev client running</span>' : ""}
          </div>
          ${p.description ? `<div class="mod-desc">${esc(p.description)}</div>` : ""}
          <button class="path" data-action="open-path" data-path="${esc(p.path)}" title="Open source folder">${esc(p.path)}</button>
        </div>
        <div class="mod-actions">
          <button class="btn small" data-action="rebuild" data-project="${esc(p.id)}" ${building ? "disabled" : ""}>${building ? "Building…" : "Build & install"}</button>
          ${inst.live?.hotswap && S.running.has(`game:${inst.id}`) ? `<button class="btn small update" data-action="hotswap" data-project="${esc(p.id)}" ${S.busy.has(`hotswap:${p.id}`) ? "disabled" : ""} title="Push code changes into the running game">${S.busy.has(`hotswap:${p.id}`) ? "Swapping…" : "⚡ Hotswap"}</button>` : ""}
          <button class="btn small ghost" data-action="ask-claude" data-project="${esc(p.id)}" title="Ask Claude Code to change this mod">Ask Claude</button>
          <button class="btn small ghost" data-action="${dev ? "stop-dev" : "run-dev"}" data-project="${esc(p.id)}" title="Test with an offline development client">${dev ? "Stop test" : "Test"}</button>
        </div>
      </div>`;
    })
    .join("");
  if (inst.loader === "vanilla") {
    return `<div class="note">Vanilla instances can't load mods. Create a Fabric or NeoForge instance to make mods with Claude.</div>`;
  }
  const opening = S.busy.has("claude-open");
  return `
    <div class="make">
      <label for="make-idea"><strong>✦ Make a mod with Claude</strong> <span class="muted small">for ${esc(inst.name)} · ${esc(inst.mc_version)} ${loaderLabel(inst.loader)}</span></label>
      <textarea id="make-idea" class="input" rows="3" data-action="make-idea" placeholder="Describe your mod, e.g. Ores glow faintly in the dark so they're easier to spot in caves">${esc(S.makeIdea)}</textarea>
      <div class="make-actions">
        <span class="muted small">Opens Claude Code in a new window with your idea and this instance ready. <button class="link" data-action="settings">Not connected?</button></span>
        <button class="btn ghost small" data-action="copy-prompt">Copy prompt</button>
        <button class="btn primary" data-action="make-mod" ${opening ? "disabled" : ""}>${opening ? "Opening…" : "Open in Claude Code"}</button>
      </div>
    </div>
    ${projects.length ? `<div class="mod-list">${rows}</div>` : '<p class="muted pad">No Claude projects for this instance yet.</p>'}`;
}

function consoleTab(inst: Instance): string {
  const sources = [`game:${inst.id}`, ...projectsFor(inst).flatMap((p) => [`build:${p.id}`, `dev:${p.id}`])];
  const lines = sources.flatMap((s) => (S.logs.get(s) ?? []).map((l) => ({ s, l })));
  return `
    <div class="toolbar">
      <span class="muted">Live output from the game, builds and test clients</span>
      <button class="btn ghost small" data-action="clear-console">Clear</button>
    </div>
    <pre id="console" class="console">${lines.length ? lines.map(({ s, l }) => logLine(s, l)).join("") : '<span class="muted">Nothing yet. Press Play or build a mod.</span>'}</pre>`;
}

function logLine(source: string, line: string): string {
  const kind = /\b(ERROR|FATAL|Exception|error:|FAILURE)\b/.test(line) ? "err" : /\bWARN\b/.test(line) ? "warn" : "";
  const tag = source.split(":")[0];
  return `<span class="ln ${kind}"><span class="src">${esc(tag)}</span>${esc(line)}\n</span>`;
}

// ---------- status bar ----------

function renderStatus() {
  const p = S.progress;
  const bar = $("#statusbar");
  if (!p) {
    bar.classList.remove("show");
    return;
  }
  bar.classList.add("show");
  const pct = p.total ? Math.round((p.current / p.total) * 100) : 0;
  bar.innerHTML = `<span>${esc(p.message)}</span><div class="bar"><div style="width:${pct}%"></div></div>`;
}

let progressTimer: number | undefined;

// ---------- events from the core ----------

function onEvent(e: any) {
  switch (e.type) {
    case "progress":
      S.progress = e;
      renderStatus();
      clearTimeout(progressTimer);
      progressTimer = window.setTimeout(() => {
        S.progress = null;
        renderStatus();
      }, e.current >= e.total ? 2500 : 30000);
      break;
    case "log": {
      const arr = S.logs.get(e.source) ?? [];
      arr.push(e.line);
      if (arr.length > LOG_LIMIT) arr.splice(0, arr.length - LOG_LIMIT);
      S.logs.set(e.source, arr);
      const c = document.querySelector<HTMLElement>("#console");
      const inst = current();
      if (c && inst && (e.source === `game:${inst.id}` || projectsFor(inst).some((p) => e.source.endsWith(`:${p.id}`)))) {
        const stick = c.scrollTop + c.clientHeight >= c.scrollHeight - 30;
        if (c.querySelector(".muted")) c.innerHTML = "";
        c.insertAdjacentHTML("beforeend", logLine(e.source, e.line));
        while (c.childElementCount > LOG_LIMIT) c.firstElementChild?.remove();
        if (stick) c.scrollTop = c.scrollHeight;
      }
      break;
    }
    case "process_started":
      S.running.add(e.source);
      renderSidebar();
      renderMain();
      break;
    case "process_exited":
      S.running.delete(e.source);
      if (e.source.startsWith("game:") && e.code && e.code !== 0) setTimeout(loadInstanceData, 800);
      renderSidebar();
      renderMain();
      break;
    case "instances_changed":
      loadInstances();
      break;
    case "mods_changed":
      if (e.instance_id === S.selected) loadInstanceData();
      break;
    case "projects_changed":
      loadInstanceData();
      break;
    case "accounts_changed":
      loadAccounts();
      closeModal();
      toast("Signed in");
      break;
    case "login_failed":
      closeModal();
      toast(e.error, "err");
      break;
    case "build_finished":
      toast(`${e.project_id}: ${e.message}`, e.success ? "ok" : "err");
      break;
  }
}

// ---------- modals ----------

function openModal(html: string, onMount?: (root: HTMLElement) => void) {
  const root = $("#modal-root");
  root.innerHTML = `<div class="backdrop" data-action="close-modal"></div><div class="modal" role="dialog" aria-modal="true">${html}</div>`;
  root.classList.add("show");
  onMount?.(root.querySelector(".modal") as HTMLElement);
}

function closeModal() {
  const root = $("#modal-root");
  root.classList.remove("show");
  root.innerHTML = "";
}

async function newInstanceModal() {
  openModal(`
    <h2>New instance</h2>
    <label class="field"><span>Name</span><input id="ni-name" class="input" placeholder="My modpack"/></label>
    <div class="field"><span>Mod loader</span>
      <div class="seg" id="ni-loader">
        <button data-v="fabric" class="on">Fabric</button><button data-v="neoforge">NeoForge</button><button data-v="vanilla">Vanilla</button>
      </div>
    </div>
    <div class="row">
      <label class="field grow"><span>Minecraft version</span><select id="ni-mc" class="input"><option>Loading…</option></select></label>
      <label class="field grow"><span>Loader version</span><select id="ni-lv" class="input"></select></label>
    </div>
    <label class="check"><input type="checkbox" id="ni-snap"/> Show snapshots</label>
    <p id="ni-hint" class="muted small"></p>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="ni-create">Create and install</button>
    </div>`);

  const mc = $<HTMLSelectElement>("#ni-mc");
  const lv = $<HTMLSelectElement>("#ni-lv");
  const hint = $("#ni-hint");
  let loader = "fabric";

  const loadLoaders = async () => {
    lv.innerHTML = "";
    hint.textContent = "";
    if (loader === "vanilla") {
      lv.disabled = true;
      return;
    }
    lv.disabled = true;
    lv.innerHTML = "<option>Loading…</option>";
    try {
      const v = await api<{ version: string; stable: boolean }[]>("GET", `/loaders/${loader}/${enc(mc.value)}`);
      if (!v.length) {
        lv.innerHTML = "<option value=''>Not available</option>";
        hint.textContent = loader === "neoforge" ? "NeoForge supports Minecraft 1.20.2 and newer." : `Fabric doesn't support ${mc.value}.`;
        return;
      }
      const firstStable = v.find((x) => x.stable)?.version ?? v[0].version;
      lv.innerHTML = v.slice(0, 40).map((x) => `<option value="${esc(x.version)}" ${x.version === firstStable ? "selected" : ""}>${esc(x.version)}${x.stable ? "" : " (beta)"}</option>`).join("");
      lv.disabled = false;
    } catch (e) {
      lv.innerHTML = "<option value=''>Couldn't load</option>";
      hint.textContent = (e as Error).message;
    }
  };

  const loadVersions = async () => {
    try {
      const snaps = $<HTMLInputElement>("#ni-snap").checked;
      const r = await api<{ versions: { id: string }[] }>("GET", `/versions?snapshots=${snaps}`);
      mc.innerHTML = r.versions.map((v) => `<option>${esc(v.id)}</option>`).join("");
      await loadLoaders();
    } catch (e) {
      mc.innerHTML = "<option>Offline</option>";
      hint.textContent = (e as Error).message;
    }
  };

  $("#ni-loader").addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest("button");
    if (!b) return;
    document.querySelectorAll("#ni-loader button").forEach((x) => x.classList.toggle("on", x === b));
    loader = b.dataset.v!;
    loadLoaders();
  });
  mc.addEventListener("change", loadLoaders);
  $("#ni-snap").addEventListener("change", loadVersions);
  $("#ni-create").addEventListener("click", async () => {
    if (loader !== "vanilla" && !lv.value) {
      toast("Pick a loader version", "err");
      return;
    }
    const btn = $<HTMLButtonElement>("#ni-create");
    btn.disabled = true;
    btn.textContent = "Creating…";
    try {
      const inst = await api<Instance>("POST", "/instances", {
        name: $<HTMLInputElement>("#ni-name").value || undefined,
        mc_version: mc.value,
        loader,
        loader_version: loader === "vanilla" ? undefined : lv.value,
      });
      S.selected = inst.id;
      closeModal();
      await loadInstances();
      busy(`launch:${inst.id}`, async () => {
        await api("POST", `/instances/${enc(inst.id)}/install`);
        toast(`${inst.name} is installed`);
        await loadInstances();
      });
    } catch (e) {
      toast((e as Error).message, "err");
      btn.disabled = false;
      btn.textContent = "Create and install";
    }
  });
  loadVersions();
}

async function settingsModal() {
  const [cfg, path, cc] = await Promise.all([
    api("GET", "/config"),
    mcpServerPath(),
    api<{ claude_path: string | null; node_path: string | null; connected: boolean }>("GET", "/claude-code").catch(() => null),
  ]);
  const ccLine = !cc
    ? ""
    : cc.connected
      ? '<p class="ok-line">✓ Connected. Claude Code can use the launcher.</p>'
      : !cc.claude_path
        ? `<p class="muted small">Claude Code isn't installed. <button class="link" data-action="open-url" data-url="https://claude.com/claude-code">Get Claude Code</button></p>`
        : !cc.node_path
          ? `<p class="muted small">The launcher's MCP server needs Node.js. <button class="link" data-action="open-url" data-url="https://nodejs.org/">Install Node.js LTS</button>, then reopen Settings.</p>`
          : `<p><button class="btn primary small" data-action="connect-claude" ${inTauri ? "" : "disabled"}>Connect automatically</button></p>`;
  const status = await api("GET", "/status");
  const cmd = `claude mcp add --scope user claude-launcher -- node "${path}"`;
  openModal(`
    <h2>Settings</h2>
    <section class="settings-sec">
      <h3>✦ Connect Claude Code</h3>
      ${ccLine}
      <p class="muted small">${cc?.connected ? "To connect it manually, run" : "Or run this once in a terminal"}. Claude Launcher needs to be open while Claude Code uses it.</p>
      <div class="copy"><code id="mcp-cmd">${esc(cmd)}</code><button class="btn small" data-copy="${esc(cmd)}">Copy</button></div>
      <p class="muted small">Then ask Claude Code something like "make a Fabric mod for my instance that adds a ruby ore".</p>
    </section>
    <section class="settings-sec">
      <h3>Microsoft sign-in</h3>
      <p class="muted small">Each launcher needs its own Azure app (client) ID, approved by Mojang for Minecraft sign-in. The README walks through registering one.</p>
      <label class="field"><span>Azure client ID</span><input id="set-cid" class="input mono" placeholder="00000000-0000-0000-0000-000000000000" value="${esc(cfg.ms_client_id ?? "")}"/></label>
    </section>
    <section class="settings-sec">
      <h3>Mod browser</h3>
      <p class="muted small">Modrinth works out of the box. CurseForge needs a free API key from <button class="link" data-action="open-url" data-url="https://console.curseforge.com/">console.curseforge.com</button>.</p>
      <label class="field"><span>CurseForge API key</span><input id="set-cf" type="password" class="input mono" placeholder="Not set" value="${esc(cfg.curseforge_api_key ?? "")}"/></label>
    </section>
    <section class="settings-sec">
      <h3>Game</h3>
      <label class="field"><span>Default memory (MB)</span><input id="set-ram" type="number" min="1024" step="512" class="input" value="${esc(cfg.default_ram_mb)}"/></label>
      <label class="field"><span>Extra Java arguments</span><input id="set-jvm" class="input mono" placeholder="-XX:+UseG1GC" value="${esc((cfg.extra_jvm_args ?? []).join(" "))}"/></label>
    </section>
    <section class="settings-sec">
      <h3>Data folder</h3>
      <button class="path" data-action="open-path" data-path="${esc(status.data_dir)}">${esc(status.data_dir)}</button>
    </section>
    <div class="modal-actions spread">
      <button class="btn ghost" data-action="run-setup">Run setup again</button>
      <span><button class="btn ghost" data-action="close-modal">Close</button> <button class="btn primary" id="set-save">Save</button></span>
    </div>`);
  $("#set-save").addEventListener("click", async () => {
    try {
      await api("PATCH", "/config", {
        ms_client_id: $<HTMLInputElement>("#set-cid").value,
        default_ram_mb: Number($<HTMLInputElement>("#set-ram").value) || 4096,
        extra_jvm_args: $<HTMLInputElement>("#set-jvm").value.split(/\s+/).filter(Boolean),
        curseforge_api_key: $<HTMLInputElement>("#set-cf").value,
      });
      closeModal();
      await loadAccounts();
      S.browse.instance = null;
      if (S.tab === "browse") runSearch(false);
      toast("Settings saved");
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
}

async function loginModal() {
  if (!S.hasClientId) {
    toast("Add your Azure client ID in Settings first", "err");
    settingsModal();
    return;
  }
  openModal(`<h2>Sign in with Microsoft</h2><p class="muted">Getting a code…</p>`);
  try {
    const code = await api("POST", "/accounts/login");
    openModal(`
      <h2>Sign in with Microsoft</h2>
      <p>Open the Microsoft sign-in page and enter this code:</p>
      <div class="code">${esc(code.user_code)}</div>
      <div class="modal-actions center">
        <button class="btn ghost" data-copy="${esc(code.user_code)}">Copy code</button>
        <button class="btn primary" data-action="open-url" data-url="${esc(code.verification_uri)}">Open sign-in page</button>
      </div>
      <p class="muted small center"><span class="spinner"></span> Waiting for you to finish signing in…</p>`);
  } catch (e) {
    closeModal();
    toast((e as Error).message, "err");
  }
}

function accountsModal() {
  openModal(`
    <h2>Accounts</h2>
    <div class="accounts">
      ${S.accounts
        .map(
          (a) => `<div class="acc-row">
            <img src="https://mc-heads.net/avatar/${esc(a.id)}/32" alt="" width="28" height="28" onerror="this.style.visibility='hidden'"/>
            <span class="grow">${esc(a.name)}${a.selected ? ' <span class="badge ok">In use</span>' : ""}</span>
            ${a.selected ? "" : `<button class="btn small ghost" data-action="use-account" data-id="${esc(a.id)}">Use</button>`}
            <button class="btn small ghost" data-action="remove-account" data-id="${esc(a.id)}">Remove</button>
          </div>`,
        )
        .join("")}
    </div>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Close</button>
      <button class="btn primary" data-action="login">Add account</button>
    </div>`);
}

async function instanceModal(inst: Instance) {
  const status = await api<{ unsupported_reason: string | null }>("GET", `/instances/${enc(inst.id)}/live`).catch(() => ({ unsupported_reason: "Couldn't check live mode support." }));
  const L: LiveSettings = inst.live ?? { hotswap: false, content_slots: false, in_memory_textures: false, unfreeze_registries: false, slot_count: 64 };
  const off = !!status.unsupported_reason;
  const toggle = (key: keyof LiveSettings, title: string, desc: string, extra = "") => `
    <label class="live-opt ${extra}">
      <span class="switch"><input type="checkbox" id="lv-${key}" ${L[key] ? "checked" : ""} ${off ? "disabled" : ""}/><span></span></span>
      <span><strong>${title}</strong><span class="muted small">${desc}</span></span>
    </label>`;
  openModal(`
    <h2>Edit instance</h2>
    <label class="field"><span>Name</span><input id="ie-name" class="input" value="${esc(inst.name)}"/></label>
    <label class="field"><span>Memory (MB, blank for default)</span><input id="ie-ram" type="number" min="1024" step="512" class="input" value="${esc(inst.ram_mb ?? "")}"/></label>
    <section class="settings-sec live-sec">
      <h3>⚡ Live mode <span class="muted small">applies the next time you press Play</span></h3>
      ${off ? `<p class="muted small">${esc(status.unsupported_reason)}</p>` : '<p class="muted small">Change the game while it runs. Singleplayer only.</p>'}
      ${toggle("hotswap", "Hotswap code", "Push Claude's code changes into the running game. Uses JetBrains Runtime, downloaded once.")}
      ${toggle("content_slots", "Content slots", "Add new blocks and items while playing, using pre-registered slots. Stable.")}
      ${toggle("in_memory_textures", "Instant textures", "Paint new textures straight into the game. Off: an automatic resource reload (a few seconds) instead.")}
      ${toggle("unfreeze_registries", "Unfreeze registries", "Experimental: register real blocks and items while playing. Can glitch or crash; back up your world first.", "danger-opt")}
      <label class="field slots ${off ? "dim" : ""}"><span>Slots (blocks and items each)</span><input id="lv-slot_count" type="number" min="1" max="256" class="input" value="${esc(L.slot_count)}" ${off ? "disabled" : ""}/></label>
    </section>
    <div class="modal-actions spread">
      <button class="btn danger ghost" id="ie-del">Delete instance</button>
      <span><button class="btn ghost" data-action="close-modal">Cancel</button> <button class="btn primary" id="ie-save">Save</button></span>
    </div>`);
  $("#ie-save").addEventListener("click", async () => {
    try {
      const ram = $<HTMLInputElement>("#ie-ram").value;
      const chk = (k: string) => $<HTMLInputElement>(`#lv-${k}`).checked;
      const live: LiveSettings | undefined = off
        ? undefined
        : {
            hotswap: chk("hotswap"),
            content_slots: chk("content_slots"),
            in_memory_textures: chk("in_memory_textures"),
            unfreeze_registries: chk("unfreeze_registries"),
            slot_count: Number($<HTMLInputElement>("#lv-slot_count").value) || 64,
          };
      if (live?.unfreeze_registries && !L.unfreeze_registries && !confirm("Unfreezing registries is experimental and can crash the game or corrupt a world. Back up worlds you care about. Turn it on?")) return;
      await api("PATCH", `/instances/${enc(inst.id)}`, { name: $<HTMLInputElement>("#ie-name").value, ram_mb: ram ? Number(ram) : undefined, live });
      if (S.running.has(`game:${inst.id}`) && live && JSON.stringify(live) !== JSON.stringify(L)) toast("Live mode changes apply the next time you launch");
      closeModal();
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
  $("#ie-del").addEventListener("click", async () => {
    if (!confirm(`Delete "${inst.name}" and everything in it (worlds, mods, settings)? This can't be undone.`)) return;
    try {
      await api("DELETE", `/instances/${enc(inst.id)}`);
      closeModal();
      toast("Instance deleted");
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
}

async function openClaude(body: { instance_id?: string; project_id?: string; request: string; crash?: boolean }, dryRun = false) {
  if (!body.request.trim() && !body.crash) {
    toast("Describe what you want first", "err");
    return;
  }
  return busy(dryRun ? "claude-copy" : "claude-open", async () => {
    const r = await api<{ opened: boolean; prompt: string; error?: string }>("POST", "/claude-code/open", { ...body, dry_run: dryRun });
    if (dryRun) {
      await navigator.clipboard.writeText(r.prompt);
      toast("Prompt copied. Paste it into Claude Code.");
      return;
    }
    if (r.opened) {
      toast("Claude Code is opening in a new window");
      if (!body.project_id) S.makeIdea = "";
      return;
    }
    openModal(`
      <h2>Couldn't open Claude Code</h2>
      <p class="muted">${esc(r.error ?? "")}</p>
      <p class="small">Paste this into Claude Code instead:</p>
      <div class="copy"><code>${esc(r.prompt)}</code></div>
      <div class="modal-actions">
        <button class="btn ghost" data-action="close-modal">Close</button>
        <button class="btn primary" data-copy="${esc(r.prompt)}">Copy prompt</button>
      </div>`);
  });
}

function askClaudeModal(p: Project) {
  openModal(`
    <h2>Ask Claude about ${esc(p.name)}</h2>
    <label class="field"><span>What should change?</span><textarea id="ask-text" class="input" rows="4" placeholder="Make the glow brighter, and add a config option for it"></textarea></label>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn ghost" id="ask-copy">Copy prompt</button>
      <button class="btn primary" id="ask-go">Open in Claude Code</button>
    </div>`, (root) => root.querySelector<HTMLTextAreaElement>("#ask-text")?.focus());
  const text = () => $<HTMLTextAreaElement>("#ask-text").value;
  $("#ask-copy").addEventListener("click", () => openClaude({ project_id: p.id, request: text() }, true));
  $("#ask-go").addEventListener("click", async () => {
    const t = text();
    if (!t.trim()) return toast("Describe what you want first", "err");
    closeModal();
    await openClaude({ project_id: p.id, request: t });
  });
}

// ---------- first-run setup ----------

const SETUP_STEPS = ["welcome", "claude", "account", "curseforge", "done"] as const;

async function setupModal(step = 0) {
  const name = SETUP_STEPS[step];
  const dots = SETUP_STEPS.map((_, i) => `<span class="dot-step ${i === step ? "on" : i < step ? "done" : ""}"></span>`).join("");
  const nav = (next: string, opts: { back?: boolean; skip?: boolean } = {}) => `
    <div class="modal-actions spread">
      <span>${opts.back !== false && step > 0 ? '<button class="btn ghost" data-setup="back">Back</button>' : '<button class="link muted" data-setup="skip-all">Skip setup</button>'}</span>
      <span>${opts.skip ? '<button class="btn ghost" data-setup="next">Skip</button> ' : ""}<button class="btn primary" id="setup-next">${next}</button></span>
    </div>`;
  let body = "";
  if (name === "welcome") {
    body = `
      <div class="setup-hero"><img src="/icon.png" alt="" width="64" height="64"/></div>
      <h2 class="center">Welcome to Claude Launcher</h2>
      <p class="center muted">Play Minecraft with Fabric or NeoForge, browse Modrinth and CurseForge, and have Claude Code build mods for you.</p>
      <p class="center muted small">Setup takes about a minute. You can change everything later in Settings.</p>
      ${nav("Get started", { back: false })}`;
  } else if (name === "claude") {
    const cc = await api<{ claude_path: string | null; node_path: string | null; connected: boolean }>("GET", "/claude-code").catch(() => null);
    const row = (ok: boolean, label: string, fix: string) =>
      `<li class="${ok ? "ok" : ""}"><span>${ok ? "✓" : "○"}</span> ${label}${ok ? "" : ` <span class="muted small">${fix}</span>`}</li>`;
    body = `
      <h2>✦ Connect Claude Code</h2>
      <p class="muted">Claude Code writes, builds and installs mods through the launcher.</p>
      <ul class="checklist">
        ${row(!!cc?.claude_path, "Claude Code installed", `<button class="link" data-action="open-url" data-url="https://claude.com/claude-code">Get it</button>`)}
        ${row(!!cc?.node_path, "Node.js installed", `<button class="link" data-action="open-url" data-url="https://nodejs.org/">Get Node.js LTS</button> (runs the launcher's tools)`)}
        ${row(!!cc?.connected, "Connected to the launcher", cc?.claude_path && cc?.node_path ? `<button class="link" data-setup="connect">Connect now</button>` : "after installing the above")}
      </ul>
      <p class="muted small">Installed something just now? <button class="link" data-setup="refresh">Check again</button></p>
      ${nav("Next", { skip: !cc?.connected })}`;
  } else if (name === "account") {
    const cfg = await api("GET", "/config");
    body = `
      <h2>Sign-in for playing</h2>
      <p class="muted">You can test mods right away without an account. To <strong>play</strong> your instances, you'll sign in with a Microsoft account that owns Minecraft, which needs a free Azure app ID:</p>
      <ol class="steps small">
        <li>At <button class="link" data-action="open-url" data-url="https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade">portal.azure.com → App registrations</button>, register an app for <em>personal Microsoft accounts only</em>.</li>
        <li>Under <em>Authentication</em>, turn on <em>Allow public client flows</em>.</li>
        <li>Paste its <em>Application (client) ID</em> below.</li>
        <li>Submit the ID to Mojang for Minecraft API access (see the README). Sign-in works once it's approved.</li>
      </ol>
      <label class="field"><span>Azure client ID</span><input id="setup-cid" class="input mono" placeholder="00000000-0000-0000-0000-000000000000" value="${esc(cfg.ms_client_id ?? "")}"/></label>
      ${nav("Next", { skip: true })}`;
  } else if (name === "curseforge") {
    const cfg = await api("GET", "/config");
    body = `
      <h2>CurseForge (optional)</h2>
      <p class="muted">Modrinth works out of the box. To also browse CurseForge, add a free API key from <button class="link" data-action="open-url" data-url="https://console.curseforge.com/">console.curseforge.com</button>.</p>
      <label class="field"><span>CurseForge API key</span><input id="setup-cf" type="password" class="input mono" placeholder="Not set" value="${esc(cfg.curseforge_api_key ?? "")}"/></label>
      ${nav("Next", { skip: true })}`;
  } else {
    body = `
      <h2>You're set</h2>
      <p class="muted">Make an instance to play and build mods for, or import a modpack.</p>
      <div class="setup-choices">
        <button class="choice" data-setup="finish-new"><strong>New instance</strong><span class="muted small">Pick a Minecraft version and loader</span></button>
        <button class="choice" data-setup="finish-import"><strong>Import a modpack</strong><span class="muted small">From a Modrinth .mrpack file</span></button>
      </div>
      ${nav("Finish")}`;
  }
  openModal(`<div class="setup"><div class="dots">${dots}</div>${body}</div>`);

  const root = document.querySelector<HTMLElement>("#modal-root .modal")!;
  const saveStep = async () => {
    if (name === "account") await api("PATCH", "/config", { ms_client_id: $<HTMLInputElement>("#setup-cid").value });
    if (name === "curseforge") await api("PATCH", "/config", { curseforge_api_key: $<HTMLInputElement>("#setup-cf").value });
  };
  const finish = async () => {
    await api("PATCH", "/config", { onboarded: true });
    await loadAccounts();
    closeModal();
  };
  root.querySelector("#setup-next")?.addEventListener("click", async () => {
    try {
      await saveStep();
      if (name === "done") await finish();
      else setupModal(step + 1);
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
  root.addEventListener("click", async (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>("[data-setup]");
    if (!el) return;
    const a = el.dataset.setup;
    if (a === "back") setupModal(step - 1);
    else if (a === "next") setupModal(step + 1);
    else if (a === "refresh") setupModal(step);
    else if (a === "skip-all") finish();
    else if (a === "connect") {
      try {
        await api("POST", "/claude-code/connect", { mcp_path: await mcpServerPath() });
        toast("Claude Code is connected");
      } catch (e) {
        toast((e as Error).message, "err");
      }
      setupModal(step);
    } else if (a === "finish-new") {
      await finish();
      newInstanceModal();
    } else if (a === "finish-import") {
      await finish();
      $<HTMLInputElement>("#pack-file").click();
    }
  });
}

function exportModal(inst: Instance) {
  const enabled = S.mods.filter((m) => m.enabled);
  const linked = enabled.filter((m) => m.online?.source === "modrinth");
  const claude = enabled.filter((m) => m.claude_project && m.online?.source !== "modrinth");
  const other = enabled.filter((m) => !linked.includes(m) && !claude.includes(m));
  openModal(`
    <h2>Export modpack</h2>
    <p class="muted small">Creates a Modrinth modpack (.mrpack) of "${esc(inst.name)}" that other launchers can import.</p>
    <ul class="export-sum">
      <li><strong>${linked.length}</strong> Modrinth mod${linked.length === 1 ? "" : "s"}, linked (not copied)</li>
      <li><strong>${claude.length}</strong> mod${claude.length === 1 ? "" : "s"} made by Claude, included in the pack</li>
      ${other.length ? `<li><strong>${other.length}</strong> other mod${other.length === 1 ? "" : "s"} (CurseForge or added by hand): ${other.map((m) => esc(m.name ?? m.file_name)).join(", ")}</li>` : ""}
    </ul>
    <label class="field"><span>Pack version</span><input id="ex-ver" class="input" value="1.0.0"/></label>
    <label class="check"><input type="checkbox" id="ex-cfg" checked/> Include mod settings (config folder)</label>
    ${other.length ? `<label class="check"><input type="checkbox" id="ex-other"/> Also include the ${other.length} other mod${other.length === 1 ? "" : "s"}. Only do this if their licences allow sharing.</label>` : ""}
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="ex-go">Export</button>
    </div>`);
  $("#ex-go").addEventListener("click", async () => {
    const btn = $<HTMLButtonElement>("#ex-go");
    btn.disabled = true;
    btn.textContent = "Exporting…";
    try {
      const r = await api<{ path: string; linked: string[]; embedded: string[]; left_out: string[] }>("POST", `/instances/${enc(inst.id)}/export`, {
        version: $<HTMLInputElement>("#ex-ver").value,
        include_config: $<HTMLInputElement>("#ex-cfg").checked,
        include_other_jars: document.querySelector<HTMLInputElement>("#ex-other")?.checked ?? false,
      });
      const folder = r.path.replace(/[\\/][^\\/]+$/, "");
      openModal(`
        <h2>Modpack exported</h2>
        <button class="path" data-action="open-path" data-path="${esc(folder)}">${esc(r.path)}</button>
        <p class="muted small">${r.linked.length} linked · ${r.embedded.length} included${r.left_out.length ? ` · ${r.left_out.length} left out` : ""}</p>
        ${r.left_out.length ? `<p class="small">Left out, so players need to add these themselves:</p><ul class="export-sum small">${r.left_out.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>` : ""}
        <div class="modal-actions">
          <button class="btn ghost" data-action="close-modal">Close</button>
          <button class="btn primary" data-action="open-path" data-path="${esc(folder)}">Show in folder</button>
        </div>`);
    } catch (e) {
      toast((e as Error).message, "err");
      btn.disabled = false;
      btn.textContent = "Export";
    }
  });
}

$<HTMLInputElement>("#pack-file").addEventListener("change", async (ev) => {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  openModal(`<h2>Importing ${esc(file.name)}</h2><p class="muted"><span class="spinner"></span> Downloading the pack's mods…</p>`);
  try {
    const r = await apiBytes<{ instance: Instance; downloaded: number; override_files: number; warnings: string[] }>("/modpacks/import", await file.arrayBuffer());
    closeModal();
    S.selected = r.instance.id;
    S.tab = "mods";
    await loadInstances();
    toast(`Imported ${r.instance.name} (${r.downloaded} downloads, ${r.override_files} included files)`);
    r.warnings.forEach((w) => toast(w, "err"));
    busy(`launch:${r.instance.id}`, async () => {
      await api("POST", `/instances/${enc(r.instance.id)}/install`);
      toast(`${r.instance.name} is installed`);
      await loadInstances();
    });
  } catch (e) {
    closeModal();
    toast((e as Error).message, "err");
  }
});

// ---------- actions ----------

document.addEventListener("click", async (ev) => {
  const t = ev.target as HTMLElement;
  const copyEl = t.closest<HTMLElement>("[data-copy]");
  if (copyEl) {
    await navigator.clipboard.writeText(copyEl.dataset.copy!);
    toast("Copied");
    return;
  }
  const el = t.closest<HTMLElement>("[data-action]");
  if (!el) return;
  const a = el.dataset.action!;
  const inst = current();
  switch (a) {
    case "select":
      S.selected = el.dataset.id!;
      S.modFilter = "";
      S.browse.instance = null;
      S.browse.hits = [];
      renderSidebar();
      await loadInstanceData();
      break;
    case "tab":
      S.tab = el.dataset.tab as Tab;
      renderMain();
      break;
    case "new-instance":
      newInstanceModal();
      break;
    case "settings":
      settingsModal();
      break;
    case "login":
      loginModal();
      break;
    case "accounts":
      accountsModal();
      break;
    case "use-account":
      await api("PATCH", "/config", { selected_account: el.dataset.id });
      await loadAccounts();
      accountsModal();
      break;
    case "remove-account":
      await api("DELETE", `/accounts/${enc(el.dataset.id!)}`);
      await loadAccounts();
      S.accounts.length ? accountsModal() : closeModal();
      break;
    case "close-modal":
      closeModal();
      break;
    case "open-path":
      openPath(el.dataset.path!).then(() => !inTauri && toast("Path copied"));
      break;
    case "open-url":
      openUrl(el.dataset.url!);
      break;
    case "instance-menu":
      if (inst) instanceModal(inst);
      break;
    case "play":
      if (!inst) break;
      if (!S.accounts.length) {
        toast("Sign in with a Microsoft account that owns Minecraft first", "err");
        loginModal();
        break;
      }
      busy(`launch:${inst.id}`, async () => {
        await api("POST", `/instances/${enc(inst.id)}/launch`, {});
      });
      break;
    case "stop":
      if (inst) await api("POST", `/instances/${enc(inst.id)}/stop`);
      break;
    case "rebuild": {
      const pid = el.dataset.project!;
      busy(`install:${pid}`, async () => {
        const r = await api("POST", `/projects/${enc(pid)}/install`, {});
        if (!r.success) {
          S.tab = "console";
          toast(`Build failed${r.errors?.[0] ? `: ${r.errors[0]}` : ""}`, "err");
        }
      });
      break;
    }
    case "run-dev": {
      const pid = el.dataset.project!;
      busy(`dev:${pid}`, async () => {
        await api("POST", `/projects/${enc(pid)}/dev`);
        S.tab = "console";
      });
      break;
    }
    case "stop-dev":
      await api("DELETE", `/projects/${enc(el.dataset.project!)}/dev`);
      break;
    case "delete-mod": {
      if (!inst) break;
      const file = el.dataset.file!;
      if (!confirm(`Delete ${file}?`)) break;
      busy(`mod:${file}`, () => api("DELETE", `/instances/${enc(inst.id)}/mods/${enc(file)}`));
      break;
    }
    case "hotswap": {
      const pid = el.dataset.project!;
      busy(`hotswap:${pid}`, async () => {
        const r = await api<{ compiled: boolean; errors: string[]; changed: string[]; agent: { redefined: string[]; deferred: string[]; errors: string[]; needs_restart: boolean } | null; notes: string[] }>("POST", `/projects/${enc(pid)}/hotswap`);
        if (!r.compiled) {
          S.tab = "console";
          toast(`Didn't compile${r.errors[0] ? `: ${r.errors[0]}` : ""}`, "err");
          return;
        }
        if (!r.agent) {
          toast(r.notes.find((n) => n.includes("swap")) ?? "Compiled");
          return;
        }
        const n = r.agent.redefined.length + r.agent.deferred.length;
        toast(n ? `Swapped ${n} class${n === 1 ? "" : "es"} into the game` : "No code changes to swap");
        r.agent.errors.forEach((e) => toast(e, "err"));
      });
      break;
    }
    case "run-setup":
      setupModal(0);
      break;
    case "crash-ask":
      if (inst) openClaude({ instance_id: inst.id, request: "", crash: true });
      break;
    case "crash-dismiss":
      if (inst) {
        await api("POST", `/instances/${enc(inst.id)}/crash/dismiss`);
        S.crash = null;
        renderMain();
      }
      break;
    case "make-mod":
      if (inst) openClaude({ instance_id: inst.id, request: S.makeIdea });
      break;
    case "copy-prompt":
      if (inst) openClaude({ instance_id: inst.id, request: S.makeIdea }, true);
      break;
    case "ask-claude": {
      const p = S.projects.find((x) => x.id === el.dataset.project);
      if (p) askClaudeModal(p);
      break;
    }
    case "connect-claude":
      busy("connect", async () => {
        await api("POST", "/claude-code/connect", { mcp_path: await mcpServerPath() });
        toast("Claude Code is connected to the launcher");
        settingsModal();
      });
      break;
    case "import-pack":
      $<HTMLInputElement>("#pack-file").click();
      break;
    case "export-pack":
      if (inst) exportModal(inst);
      break;
    case "check-updates":
      checkUpdates();
      break;
    case "update-all":
      applyUpdates();
      break;
    case "update-one":
      applyUpdates([el.dataset.file!.replace(/\.disabled$/, "")]);
      break;
    case "browse-source":
      S.browse.source = el.dataset.v as Source;
      S.browse.hits = [];
      runSearch(false);
      break;
    case "browse-more":
      runSearch(true);
      break;
    case "browse-install":
      installOnline(el.dataset.pid!);
      break;
    case "browse-versions":
      versionsModal(el.dataset.pid!, el.dataset.title!);
      break;
    case "install-version":
      closeModal();
      installOnline(el.dataset.pid!, el.dataset.vid!);
      break;
    case "clear-console":
      if (inst) [`game:${inst.id}`, ...projectsFor(inst).flatMap((p) => [`build:${p.id}`, `dev:${p.id}`])].forEach((s) => S.logs.delete(s));
      renderMain();
      break;
  }
});

let searchTimer: number | undefined;

document.addEventListener("change", (ev) => {
  const el = ev.target as HTMLInputElement;
  if (el.dataset.action === "browse-sort") {
    S.browse.sort = el.value;
    runSearch(false);
    return;
  }
  if (el.dataset.action === "toggle-mod") {
    const inst = current();
    if (!inst) return;
    const file = el.dataset.file!;
    busy(`mod:${file}`, () => api("POST", `/instances/${enc(inst.id)}/mods/${enc(file)}/enabled`, { enabled: el.checked }));
  }
});

document.addEventListener("input", (ev) => {
  const el = ev.target as HTMLInputElement;
  if (el.dataset.action === "filter-mods") {
    S.modFilter = el.value;
    refocus = "mod-filter";
    renderMain();
  }
  if (el.dataset.action === "make-idea") {
    S.makeIdea = el.value;
  }
  if (el.dataset.action === "browse-q") {
    S.browse.q = el.value;
    clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      refocus = "browse-q";
      runSearch(false);
    }, 300);
  }
});

document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape") closeModal();
});

// ---------- start ----------

(async () => {
  try {
    await connect();
    events(onEvent);
    await Promise.all([loadAccounts(), loadInstances()]);
    const cfg = await api("GET", "/config");
    if (!cfg.onboarded) setupModal(0);
  } catch (e) {
    $("#main").innerHTML = `<div class="empty"><h1>Couldn't start</h1><p>${esc((e as Error).message)}</p></div>`;
  }
})();
