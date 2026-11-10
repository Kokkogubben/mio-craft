import { api, connect, enc, events, inTauri, mcpServerPath, openPath, openUrl } from "./api";

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
}

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

type Tab = "mods" | "projects" | "console";

// ---------- state ----------

const S = {
  instances: [] as Instance[],
  selected: null as string | null,
  tab: "mods" as Tab,
  mods: [] as ModEntry[],
  projects: [] as Project[],
  accounts: [] as Account[],
  hasClientId: false,
  running: new Set<string>(),
  busy: new Set<string>(),
  modFilter: "",
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
    const [mods, projects] = await Promise.all([
      api<ModEntry[]>("GET", `/instances/${enc(inst.id)}/mods`),
      api<Project[]>("GET", "/projects"),
    ]);
    S.mods = mods;
    S.projects = projects;
  } else {
    S.mods = [];
  }
  renderMain();
}

async function loadAccounts() {
  const [accounts, status] = await Promise.all([api<Account[]>("GET", "/accounts"), api("GET", "/status")]);
  S.accounts = accounts;
  S.hasClientId = status.has_client_id;
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
        </div>
      </div>
      <div class="head-actions">
        <button class="btn ghost" data-action="open-path" data-path="${esc(inst.game_dir)}" title="Open the game folder">Folder</button>
        <button class="btn ghost" data-action="instance-menu" title="Instance settings">Edit</button>
        <button class="btn ${running ? "danger" : "play"}" data-action="${running ? "stop" : "play"}" ${launching ? "disabled" : ""}>${playLabel}</button>
      </div>
    </header>
    <div class="tabs" role="tablist">
      ${(["mods", "projects", "console"] as Tab[])
        .map((t) => {
          const label = t === "mods" ? `Mods <span class="count">${S.mods.length}</span>` : t === "projects" ? `Claude projects <span class="count">${projectsFor(inst).length}</span>` : "Console";
          return `<button role="tab" class="tab ${S.tab === t ? "active" : ""}" data-action="tab" data-tab="${t}">${label}</button>`;
        })
        .join("")}
    </div>
    <section class="tab-body">${S.tab === "mods" ? modsTab(inst) : S.tab === "projects" ? projectsTab(inst) : consoleTab(inst)}</section>`;
  if (S.tab === "console") {
    const c = $("#console");
    c.scrollTop = c.scrollHeight;
  }
  const filter = document.querySelector<HTMLInputElement>("#mod-filter");
  if (filter && document.activeElement?.id !== "mod-filter" && S.modFilter) {
    filter.focus();
    filter.setSelectionRange(filter.value.length, filter.value.length);
  }
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
          </div>
          ${m.description ? `<div class="mod-desc">${esc(m.description)}</div>` : ""}
          <div class="mod-file">${esc(m.file_name)} · ${fmtSize(m.size)}</div>
        </div>
        <div class="mod-actions">
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
      <button class="btn ghost" data-action="open-path" data-path="${esc(inst.mods_dir)}">Open mods folder</button>
    </div>
    ${
      S.mods.length
        ? `<div class="mod-list">${rows || '<p class="muted pad">No mods match.</p>'}</div>`
        : `<div class="note">No mods yet. Drop .jar files into the mods folder, or ask Claude Code to make one, for example: <code>Make a ${loaderLabel(inst.loader)} mod for my "${esc(inst.name)}" instance that makes ores glow</code></div>`
    }`;
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
          <button class="btn small ghost" data-action="${dev ? "stop-dev" : "run-dev"}" data-project="${esc(p.id)}" title="Test with an offline development client">${dev ? "Stop test" : "Test"}</button>
        </div>
      </div>`;
    })
    .join("");
  return `
    <div class="note claude-note">
      <strong>✦ Mods from Claude Code.</strong> Ask Claude Code to make a mod for this instance; it creates the project, writes the code, builds it and installs it here.
      ${S.projects.length ? "" : `Not connected yet? <button class="link" data-action="settings">Connect Claude Code</button>`}
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
      if (e.source.startsWith("game:") && e.code && e.code !== 0) toast(`The game exited with code ${e.code}. Check the Console tab.`, "err");
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
  const [cfg, path] = await Promise.all([api("GET", "/config"), mcpServerPath()]);
  const status = await api("GET", "/status");
  const cmd = `claude mcp add --scope user claude-launcher -- node "${path}"`;
  openModal(`
    <h2>Settings</h2>
    <section class="settings-sec">
      <h3>✦ Connect Claude Code</h3>
      <p class="muted small">Run this once in a terminal. Claude Launcher needs to be open while Claude Code uses it.</p>
      <div class="copy"><code id="mcp-cmd">${esc(cmd)}</code><button class="btn small" data-copy="${esc(cmd)}">Copy</button></div>
      <p class="muted small">Then ask Claude Code something like "make a Fabric mod for my instance that adds a ruby ore".</p>
    </section>
    <section class="settings-sec">
      <h3>Microsoft sign-in</h3>
      <p class="muted small">Each launcher needs its own Azure app (client) ID, approved by Mojang for Minecraft sign-in. The README walks through registering one.</p>
      <label class="field"><span>Azure client ID</span><input id="set-cid" class="input mono" placeholder="00000000-0000-0000-0000-000000000000" value="${esc(cfg.ms_client_id ?? "")}"/></label>
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
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Close</button>
      <button class="btn primary" id="set-save">Save</button>
    </div>`);
  $("#set-save").addEventListener("click", async () => {
    try {
      await api("PATCH", "/config", {
        ms_client_id: $<HTMLInputElement>("#set-cid").value,
        default_ram_mb: Number($<HTMLInputElement>("#set-ram").value) || 4096,
        extra_jvm_args: $<HTMLInputElement>("#set-jvm").value.split(/\s+/).filter(Boolean),
      });
      closeModal();
      await loadAccounts();
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

function instanceModal(inst: Instance) {
  openModal(`
    <h2>Edit instance</h2>
    <label class="field"><span>Name</span><input id="ie-name" class="input" value="${esc(inst.name)}"/></label>
    <label class="field"><span>Memory (MB, blank for default)</span><input id="ie-ram" type="number" min="1024" step="512" class="input" value="${esc(inst.ram_mb ?? "")}"/></label>
    <div class="modal-actions spread">
      <button class="btn danger ghost" id="ie-del">Delete instance</button>
      <span><button class="btn ghost" data-action="close-modal">Cancel</button> <button class="btn primary" id="ie-save">Save</button></span>
    </div>`);
  $("#ie-save").addEventListener("click", async () => {
    try {
      const ram = $<HTMLInputElement>("#ie-ram").value;
      await api("PATCH", `/instances/${enc(inst.id)}`, { name: $<HTMLInputElement>("#ie-name").value, ram_mb: ram ? Number(ram) : undefined });
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
    case "clear-console":
      if (inst) [`game:${inst.id}`, ...projectsFor(inst).flatMap((p) => [`build:${p.id}`, `dev:${p.id}`])].forEach((s) => S.logs.delete(s));
      renderMain();
      break;
  }
});

document.addEventListener("change", (ev) => {
  const el = ev.target as HTMLInputElement;
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
    renderMain();
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
  } catch (e) {
    $("#main").innerHTML = `<div class="empty"><h1>Couldn't start</h1><p>${esc((e as Error).message)}</p></div>`;
  }
})();
