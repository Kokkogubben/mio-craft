import { api, apiBytes, appVersion, canMakeShortcuts, checkUpdate, connect, createShortcut, enc, events, inTauri, installUpdate, mcpServerPath, onPlayInstance, openPath, openUrl, takeStartupPlay } from "./api";
import { cachedHead, initPlayer, loadImage, onPlayerClick, pausePlayer, showAccount, wearSkin } from "./player";
import { IdleAnimation, SkinViewer } from "skinview3d";
import { resync, slide, slideIn } from "./slide";
import { GLYPHS, ICON_COLORS, glyphSvg, inkFor, pictureToIcon } from "./icons";
import { closeMenu, menuOpen, showMenu, type MenuItem } from "./menu";

// ---------- types ----------

interface Instance {
  id: string;
  name: string;
  mc_version: string;
  loader: "vanilla" | "fabric" | "neoforge" | "forge" | "quilt" | "legacyfabric";
  loader_version: string | null;
  installed: boolean;
  running: boolean;
  game_dir: string;
  mods_dir: string;
  ram_mb: number | null;
  jvm_args?: string[] | null;
  /** Why Claude's mod templates don't cover this instance (null when they do). */
  mod_template_unsupported?: string | null;
  created_at: number;
  last_played: number | null;
  /** Seconds the game has run in this instance. */
  playtime_secs?: number;
  live?: LiveSettings;
  icon_color?: string | null;
  icon_glyph?: string | null;
  icon_image?: string | null;
  launch?: LaunchOptions;
  /** The group (folder) it's listed under in the sidebar. */
  group?: string | null;
}

/** Named selections of which mods are on ("Full", "Light"); the mods folder is always the active one. */
interface ModSetsView {
  for?: string;
  active: string | null;
  sets: { name: string; enabled: number }[];
}

/** Window size, a command before launch and environment variables (Edit only; never in modpacks). */
interface LaunchOptions {
  width?: number;
  height?: number;
  fullscreen?: boolean;
  before_launch?: string;
  env?: { name: string; value: string }[];
}

/** The env text box: one NAME=value per line. */
function parseEnv(text: string): { name: string; value: string }[] {
  return text
    .split(/\r?\n/)
    .filter((l) => l.trim())
    .map((l) => {
      const at = l.indexOf("=");
      if (at < 0) throw new Error(`"${l.trim()}" needs a value: write it as NAME=value`);
      return { name: l.slice(0, at).trim(), value: l.slice(at + 1) };
    });
}

/** An instance's icon tile: its picture, or a glyph or letter on its colour. */
function instIcon(i: Pick<Instance, "loader" | "icon_color" | "icon_glyph" | "icon_image">, big = false): string {
  const cls = `inst-icon ${esc(i.loader)}${big ? " big" : ""}`;
  if (i.icon_image) return `<span class="${cls} pic"><img src="${esc(i.icon_image)}" alt=""/></span>`;
  const style = i.icon_color ? ` style="background:${esc(i.icon_color)};color:${inkFor(i.icon_color)}"` : "";
  const inner = i.icon_glyph && GLYPHS.includes(i.icon_glyph) ? glyphSvg(i.icon_glyph, big ? 40 : 24) : esc(i.loader[0].toUpperCase());
  return `<span class="${cls}"${style}>${inner}</span>`;
}

type View = "home" | "instances";

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

type Tab = "mods" | "browse" | "packs" | "servers" | "projects" | "console";
type Server = { index: number; name: string; address: string; icon?: string };

type BrowseKind = "mod" | "shader" | "resourcepack";

interface ShaderPack {
  file_name: string;
  size: number;
  online: { source: Source; title: string; page_url: string } | null;
}

interface ShaderList {
  packs: ShaderPack[];
  has_loader: boolean;
  folder: string;
}

const NO_SHADERS: ShaderList = { packs: [], has_loader: false, folder: "" };

/** The instance's resource packs, and which ones the game has turned on. */
interface ResourcePackList {
  for?: string;
  packs: { file_name: string; description: string | null; icon: string | null; size: number; enabled: boolean; online: { source: Source; title: string; page_url: string } | null }[];
  folder: string;
}
/** Iris on Modrinth: the mod that loads shader packs. */
const IRIS_ID = "YL57xq9U";

// ---------- state ----------

const S = {
  /** The launcher's settings, as last read or saved. */
  cfg: null as any,
  /** The home screen look is on (off with Settings → Legacy → Legacy look). */
  xp: false,
  view: "home" as View,
  instances: [] as Instance[],
  selected: null as string | null,
  tab: "mods" as Tab,
  mods: [] as ModEntry[],
  projects: [] as Project[],
  shaders: NO_SHADERS,
  resourcePacks: null as null | ResourcePackList,
  accounts: [] as Account[],
  hasClientId: false,
  hasBuiltinClientId: false,
  hasCfKey: false,
  browse: {
    instance: null as string | null,
    source: "modrinth" as Source,
    kind: "mod" as BrowseKind,
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
  /** Sidebar search. */
  instQ: "",
  /** The selected instance's mod sets, once loaded. */
  modSets: null as null | ModSetsView,
  /** Problems the mod check found in the selected instance (missing, wrong loader, twice). */
  modCheck: null as null | { for: string; problems: { file: string; message: string }[] },
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
  updates: { instance: null as string | null, byFile: new Map<string, ModUpdate>(), checked: false },
  logs: new Map<string, string[]>(),
  /** Each instance's multiplayer list, once loaded. */
  servers: new Map<string, Server[]>(),
  progress: null as null | { task: string; current: number; total: number; message: string },
};

const LOG_LIMIT = 3000;

// ---------- helpers ----------

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector(sel) as T;

function esc(s: unknown): string {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

const LOADER_LABELS: Record<string, string> = {
  fabric: "Fabric",
  neoforge: "NeoForge",
  forge: "Forge",
  quilt: "Quilt",
  legacyfabric: "Legacy Fabric",
  vanilla: "Vanilla",
};

function loaderLabel(l: string): string {
  return LOADER_LABELS[l] ?? l;
}

/** Why a loader has no builds for a Minecraft version (mirrors the core's message). */
function loaderUnsupported(loader: string, mc: string): string {
  switch (loader) {
    case "neoforge":
      return "NeoForge supports Minecraft 1.20.2 and newer.";
    case "legacyfabric":
      return `Legacy Fabric supports Minecraft 1.3 to 1.13.2, not ${mc}.`;
    case "fabric":
      return `Fabric doesn't support Minecraft ${mc}. For 1.13.2 and older, use Legacy Fabric.`;
    default:
      return `${loaderLabel(loader)} has no builds for Minecraft ${mc}.`;
  }
}

/** Claude's mod templates exist for these loaders only. */

function fmtSize(b: number): string {
  if (b >= 1024 * 1024 * 1024) return `${(b / 1024 / 1024 / 1024).toFixed(1)} GB`;
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

/** "45 min", "3 h 20 min", "120 h". */
function fmtPlaytime(secs: number): string {
  const m = Math.floor(secs / 60);
  if (m < 1) return "under a minute";
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  return h >= 100 || m % 60 === 0 ? `${h} h` : `${h} h ${m % 60} min`;
}

const totalPlaytime = () => S.instances.reduce((n, i) => n + (i.playtime_secs ?? 0), 0);

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

/** Most recently played first; never-played ones after them, newest first. */
const byRecent = (list: Instance[]) =>
  [...list].sort((a, b) => (b.last_played ?? 0) - (a.last_played ?? 0) || b.created_at - a.created_at);

// ---------- data ----------

async function loadInstances() {
  S.instances = await api<Instance[]>("GET", "/instances");
  S.instances.forEach((i) => (i.running ? S.running.add(`game:${i.id}`) : S.running.delete(`game:${i.id}`)));
  if (!S.selected || !S.instances.some((i) => i.id === S.selected)) S.selected = byRecent(S.instances)[0]?.id ?? null;
  renderSidebar();
  await loadInstanceData();
}

async function loadInstanceData() {
  const inst = current();
  if (inst) {
    const [mods, projects, crash, sets] = await Promise.all([
      api<ModEntry[]>("GET", `/instances/${enc(inst.id)}/mods`),
      api<Project[]>("GET", "/projects"),
      api("GET", `/instances/${enc(inst.id)}/crash`).catch(() => null),
      api<ModSetsView>("GET", `/instances/${enc(inst.id)}/mod-sets`).catch(() => null),
    ]);
    S.mods = mods;
    S.modSets = sets && { ...sets, for: inst.id };
    api<ResourcePackList>("GET", `/instances/${enc(inst.id)}/resourcepacks`)
      .then((r) => {
        S.resourcePacks = { ...r, for: inst.id };
        if (S.selected === inst.id && (S.tab === "packs" || S.tab === "browse")) renderMain();
      })
      .catch(() => {});
    // Opens every jar, so it comes in after the list instead of holding it up.
    if (inst.loader !== "vanilla")
      api<{ problems: { file: string; message: string }[] }>("GET", `/instances/${enc(inst.id)}/mod-check`)
        .then((r) => {
          S.modCheck = { for: inst.id, problems: r.problems };
          if (S.selected === inst.id && S.tab === "mods") renderMain();
        })
        .catch(() => {});
    S.projects = projects;
    S.crash = crash;
    S.shaders =
      inst.loader === "vanilla"
        ? NO_SHADERS
        : await api<ShaderList>("GET", `/instances/${enc(inst.id)}/shaders`).catch(() => NO_SHADERS);
  } else {
    S.mods = [];
    S.shaders = NO_SHADERS;
  }
  renderMain();
}

async function loadAccounts() {
  const [accounts, status] = await Promise.all([api<Account[]>("GET", "/accounts"), api("GET", "/status")]);
  S.accounts = accounts;
  S.hasClientId = status.has_client_id;
  S.hasBuiltinClientId = !!status.has_builtin_client_id;
  S.hasCfKey = status.has_curseforge_key;
  (status.running as string[]).forEach((r) => S.running.add(r));
  renderSidebar();
  renderTopbar();
  refreshPlayer();
}

// ---------- sidebar ----------

// ---------- sidebar groups and search ----------

/** Groups folded shut in the sidebar (this browser only). */
const foldedGroups: Set<string> = (() => {
  try {
    return new Set<string>(JSON.parse(localStorage.getItem("mc.foldedGroups") ?? "[]"));
  } catch {
    return new Set<string>();
  }
})();

function toggleGroup(g: string) {
  if (!foldedGroups.delete(g)) foldedGroups.add(g);
  try {
    localStorage.setItem("mc.foldedGroups", JSON.stringify([...foldedGroups]));
  } catch {
    /* only a convenience */
  }
  renderSidebar();
}

const groupNames = () => [...new Set(S.instances.map((i) => i.group).filter((g): g is string => !!g))].sort((a, b) => a.localeCompare(b));

function instMatches(i: Instance, q: string): boolean {
  return `${i.name} ${i.mc_version} ${loaderLabel(i.loader)} ${i.group ?? ""}`.toLowerCase().includes(q);
}

function instRow(i: Instance, inGroup: boolean): string {
  return `
      <button class="inst ${inGroup ? "in-group" : ""} ${i.id === S.selected ? "active" : ""}" data-action="select" data-id="${esc(i.id)}">
        ${instIcon(i)}
        <span class="inst-text">
          <span class="inst-name">${esc(i.name)}</span>
          <span class="inst-meta">${esc(i.mc_version)} · ${loaderLabel(i.loader)}</span>
        </span>
        ${S.running.has(`game:${i.id}`) ? '<span class="dot" title="Running"></span>' : ""}
      </button>`;
}

function sidebarList(): string {
  if (!S.instances.length) return `<p class="side-empty">No instances yet.</p>`;
  const q = S.instQ.trim().toLowerCase();
  const shown = q ? S.instances.filter((i) => instMatches(i, q)) : S.instances;
  if (!shown.length) return `<p class="side-empty">No instances match.</p>`;
  let html = shown.filter((i) => !i.group).map((i) => instRow(i, false)).join("");
  for (const g of groupNames()) {
    const members = shown.filter((i) => i.group === g);
    if (!members.length) continue;
    // While searching, every group with a match is open.
    const open = !!q || !foldedGroups.has(g);
    const running = members.some((i) => S.running.has(`game:${i.id}`));
    html += `<button class="side-group ${open ? "open" : ""}" data-action="toggle-group" data-group="${esc(g)}" aria-expanded="${open}">
        <svg class="chev" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M3 2l4 3-4 3" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>
        <span class="side-group-name">${esc(g)}</span>
        ${!open && running ? '<span class="dot" title="Running"></span>' : ""}
        <span class="side-group-count">${members.length}</span>
      </button>`;
    if (open) html += members.map((i) => instRow(i, true)).join("");
  }
  return html;
}

function groupModal(inst: Instance) {
  const groups = groupNames();
  openModal(`
    <h2>Move to group</h2>
    <p class="muted small">Groups are folders in the instance list. Type a new name or pick one you already have.</p>
    <label class="field"><span>Group</span><input id="grp-name" class="input" maxlength="40" list="grp-list" value="${esc(inst.group ?? "")}" placeholder="e.g. Modpacks"/></label>
    <datalist id="grp-list">${groups.map((g) => `<option value="${esc(g)}"></option>`).join("")}</datalist>
    ${groups.length ? `<div class="grp-picks">${groups.map((g) => `<button type="button" class="btn small ghost" data-grp="${esc(g)}">${esc(g)}</button>`).join("")}</div>` : ""}
    <div class="modal-actions spread">
      <span>${inst.group ? '<button class="btn ghost" id="grp-clear">Take out of group</button>' : ""}</span>
      <span><button class="btn ghost" data-action="close-modal">Cancel</button> <button class="btn primary" id="grp-ok">Move</button></span>
    </div>`);
  const input = $<HTMLInputElement>("#grp-name");
  input.focus();
  input.select();
  const save = async (group: string) => {
    closeModal();
    try {
      await api("PATCH", `/instances/${enc(inst.id)}`, { group });
      if (group) {
        foldedGroups.delete(group);
        try {
          localStorage.setItem("mc.foldedGroups", JSON.stringify([...foldedGroups]));
        } catch {
          /* only a convenience */
        }
      }
    } catch (e) {
      toast((e as Error).message, "err");
    }
  };
  $("#grp-ok").addEventListener("click", () => save(input.value.trim()));
  input.addEventListener("keydown", (e) => e.key === "Enter" && save(input.value.trim()));
  document.querySelectorAll<HTMLElement>("[data-grp]").forEach((b) => b.addEventListener("click", () => save(b.dataset.grp!)));
  document.querySelector("#grp-clear")?.addEventListener("click", () => save(""));
}

function renderSidebar() {
  const list = $("#instance-list");
  $("#side-search").hidden = S.instances.length < 2 && !S.instQ;
  const html = sidebarList();
  if (html !== list.dataset.html) {
    list.innerHTML = html;
    list.dataset.html = html;
  }
  slide("inst", list, list.querySelector(".inst.active"));

  // Classic layout: account and settings at the bottom of the sidebar.
  const acc = selectedAccount();
  $("#side-foot").innerHTML = `
    ${
      acc
        ? `<button class="account" data-action="accounts">${headImg(acc, 24)}<span>${esc(acc.name)}</span></button>`
        : `<button class="account signin" data-action="login">Sign in with Microsoft</button>`
    }
    <button class="icon-btn" data-action="settings" title="Settings" aria-label="Settings">${GEAR_ICON}</button>`;
}

// ---------- top bar & home ----------

const GEAR_ICON = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>`;

const selectedAccount = () => S.accounts.find((a) => a.selected) ?? S.accounts[0] ?? null;

/** The account's head from its skin, or a plain tile until the skin has loaded. */
function headImg(a: Account, px: number): string {
  const head = cachedHead(a.id);
  return head
    ? `<img class="head" src="${head}" alt="" width="${px}" height="${px}"/>`
    : `<span class="head blank" style="width:${px}px;height:${px}px"></span>`;
}

function renderTopbar() {
  document.querySelectorAll<HTMLElement>(".navtab").forEach((t) => t.classList.toggle("active", t.dataset.view === S.view));
  slide("nav", S.xp ? $(".topnav") : null, $(".navtab.active"));
  const acc = selectedAccount();
  $("#top-right").innerHTML = `
    ${
      acc
        ? `<button class="account" data-action="accounts" title="Accounts">${headImg(acc, 24)}<span>${esc(acc.name)}</span></button>`
        : `<button class="btn" data-action="login">Sign in</button>`
    }
    <button class="btn square" data-action="settings" title="Settings" aria-label="Settings">
      ${GEAR_ICON}
    </button>`;
}

/** Load the selected account's skin into the player and the top bar. */
async function refreshPlayer() {
  const acc = selectedAccount();
  await showAccount(acc?.id ?? null);
  renderSidebar();
  renderTopbar();
  renderHome();
}

/** Show a view. `animate` slides it in, for switches the user made. */
function setView(v: View, animate = false) {
  const before = S.view;
  S.view = v;
  const home = S.xp && v === "home";
  document.body.classList.toggle("v-home", home);
  pausePlayer(!home);
  resync();
  renderTopbar();
  renderHome();
  if (animate && S.xp && before !== v) slideIn(home ? $("#home") : $("#app"), home ? -48 : 48);
}

let playerReady = false;

const LEGACY_KEY = "miocraft.legacyColors";
const LEGACY_LOOK_KEY = "miocraft.legacyLook";
const PLAIN_KEY = "miocraft.plain";

/** Switch between the copper colours and the original ones (Settings → Legacy → Legacy colors). */
function applyColors(legacy: boolean) {
  document.body.classList.toggle("legacy", legacy);
  // Remembered here too, so the next start paints the right colours before the config has loaded.
  try {
    localStorage.setItem(LEGACY_KEY, legacy ? "1" : "0");
  } catch {}
}

/** The pixel font and pixel-art controls, or the original font and plain controls in the new layout
 *  (Settings → Legacy → Stylized fonts and buttons). */
function applyStyle(stylized: boolean) {
  document.body.classList.toggle("plain", !stylized);
  try {
    localStorage.setItem(PLAIN_KEY, stylized ? "0" : "1");
  } catch {}
}

// Paint the remembered colours and layout before the config has loaded, so neither flashes.
try {
  if (localStorage.getItem(LEGACY_KEY) === "1") document.body.classList.add("legacy");
  if (localStorage.getItem(LEGACY_LOOK_KEY) !== "1") document.body.classList.add("xp", "v-home");
  if (localStorage.getItem(PLAIN_KEY) === "1") document.body.classList.add("plain");
} catch {
  document.body.classList.add("xp", "v-home");
}

/** Switch between the home screen look and the original layout (Settings → Legacy → Legacy look). */
function applyLook(xp: boolean) {
  S.xp = xp;
  document.body.classList.toggle("xp", xp);
  try {
    localStorage.setItem(LEGACY_LOOK_KEY, xp ? "0" : "1");
  } catch {}
  if (xp && !playerReady) {
    initPlayer($<HTMLCanvasElement>("#skin"), $("#player-label"));
    onPlayerClick(() => {
      const acc = selectedAccount();
      if (acc) outfitModal(acc);
    });
    playerReady = true;
    refreshPlayer();
  }
  setView(xp ? "home" : "instances");
}

const RECENT_LIMIT = 6;
// Drawn on a pixel grid so they match the pixel font.
const PLAY_ICON = `<svg class="pix-icon" width="18" height="18" viewBox="0 0 9 9" shape-rendering="crispEdges" fill="currentColor"><path d="M2 1h2v1h1v1h1v1h1v1h-1v1h-1v1h-1v1h-2z"/></svg>`;
const STOP_ICON = `<svg class="pix-icon" width="18" height="18" viewBox="0 0 9 9" shape-rendering="crispEdges" fill="currentColor"><path d="M2 2h5v5h-5z"/></svg>`;

function instCard(i: Instance, action: string): string {
  const running = S.running.has(`game:${i.id}`);
  const when = running ? '<span class="dot" title="Running"></span>' : `<span class="muted small">${i.last_played ? ago(i.last_played) : "Never played"}</span>`;
  return `
    <button class="icard ${i.id === S.selected ? "active" : ""}" data-action="${action}" data-id="${esc(i.id)}" title="${[i.playtime_secs ? `Played ${fmtPlaytime(i.playtime_secs)}` : "", action === "pick-instance" ? "Double-click to open" : ""].filter(Boolean).join(". ")}">
      ${instIcon(i)}
      <span class="inst-text">
        <span class="inst-name">${esc(i.name)}</span>
        <span class="inst-meta">${esc(i.mc_version)} · ${loaderLabel(i.loader)}</span>
      </span>
      ${when}
    </button>`;
}

function renderHome() {
  if (!S.xp || S.view !== "home") return;
  const acc = selectedAccount();
  $("#player-label").innerHTML = acc
    ? `<button class="nameplate" data-action="accounts">${esc(acc.name)}</button>`
    : `<button class="nameplate" data-action="login">Sign in</button>`;

  const inst = current();
  if (!inst) {
    $("#home-play").innerHTML = `<button class="btn play big" data-action="new-instance"><span class="play-main">New instance</span><span class="play-sub">Pick a version and a loader</span></button>`;
  } else {
    const running = S.running.has(`game:${inst.id}`);
    const launching = S.busy.has(`launch:${inst.id}`);
    const main = running ? "Stop" : launching ? "Starting…" : inst.installed ? "Play" : "Install & play";
    $("#home-play").innerHTML = `
      <button class="btn ${running ? "danger" : "play"} big" data-action="${running ? "stop" : "play"}" ${launching ? "disabled" : ""}>
        <span class="play-main">${running ? STOP_ICON : PLAY_ICON} ${main}</span>
        <span class="play-sub">${esc(inst.name)} · ${esc(inst.mc_version)}${inst.loader === "vanilla" ? "" : ` ${loaderLabel(inst.loader)}`}</span>
      </button>
      <button class="btn square big-square" data-action="choose-instance" title="Choose instance" aria-label="Choose instance">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="square"><path d="M4 6h2M10 6h10M4 12h2M10 12h10M4 18h2M10 18h10"/></svg>
      </button>`;
  }

  const recent = byRecent(S.instances).slice(0, RECENT_LIMIT);
  const box = $("#home-recent");
  const html = `
    <div class="recent">
      <div class="recent-head">
        <span class="recent-title">Recently played</span>
        ${totalPlaytime() >= 60 ? `<span class="muted small" title="All instances together">${fmtPlaytime(totalPlaytime())} played in total</span>` : ""}
        <span class="grow"></span>
        <button class="btn small" data-action="new-instance">+ New</button>
        <button class="btn small" data-action="view" data-view="instances">View all</button>
      </div>
      ${
        recent.length
          ? `<div class="recent-grid">${recent.map((i) => instCard(i, "pick-instance")).join("")}</div>`
          : `<p class="muted">No instances yet. Make one and it shows up here.</p>`
      }
    </div>`;
  // Only re-render when something changed, so a sliding highlight isn't cut short.
  if (html !== box.dataset.html) {
    box.innerHTML = html;
    box.dataset.html = html;
  }
  const grid = box.querySelector<HTMLElement>(".recent-grid");
  slide("card", grid, grid?.querySelector(".icard.active") ?? null);
}

function chooseInstanceModal() {
  openModal(`
    <h2>Choose instance</h2>
    <div class="pick-list">${byRecent(S.instances).map((i) => instCard(i, "choose-pick")).join("")}</div>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Close</button>
    </div>`);
}

// Which way the instance page slides in when it next renders: 1 from below, -1 from above.
let mainSlide = 0;

async function selectInstance(id: string) {
  const order = S.instances.map((i) => i.id);
  if (S.selected && S.selected !== id) mainSlide = order.indexOf(id) > order.indexOf(S.selected) ? 1 : -1;
  S.selected = id;
  S.modFilter = "";
  if (C.filesFor !== id) resetConsole();
  S.browse.instance = null;
  S.browse.hits = [];
  renderSidebar();
  renderHome();
  await loadInstanceData();
}

// ---------- main panel ----------

function renderMain() {
  renderHome();
  const main = $("#main");
  const inst = current();
  if (!inst) {
    main.innerHTML = `
      <div class="empty">
        <img src="/icon.png" alt="" width="64" height="64"/>
        <h1>Make your first instance</h1>
        <p>Pick a Minecraft version and a mod loader. Then ask Claude Code to make a mod for it, and it'll show up here.</p>
        <button class="btn primary" data-action="new-instance">New instance</button>
      </div>`;
    slide("tab", null, null);
    return;
  }
  const running = S.running.has(`game:${inst.id}`);
  const launching = S.busy.has(`launch:${inst.id}`);
  const playLabel = running ? "Stop" : launching ? "Starting…" : inst.installed ? "Play" : "Install & play";
  main.innerHTML = `
    <header class="inst-head">
      ${instIcon(inst, true)}
      <div class="inst-head-text">
        <h1 title="${esc(inst.name)}">${esc(inst.name)}</h1>
        <div class="chips">
          <span class="chip">Minecraft ${esc(inst.mc_version)}</span>
          <span class="chip ${esc(inst.loader)}">${loaderLabel(inst.loader)}${inst.loader_version ? ` ${esc(inst.loader_version)}` : ""}</span>
          ${inst.installed ? "" : '<span class="chip warn">Not installed</span>'}
          ${inst.playtime_secs ? `<span class="chip" title="${esc(`${fmtPlaytime(totalPlaytime())} across all instances`)}">Played ${fmtPlaytime(inst.playtime_secs)}</span>` : ""}
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
      ${(["mods", "browse", "packs", "servers", "projects", "console"] as Tab[])
        .map((t) => {
          const rp = S.resourcePacks?.for === inst.id ? S.resourcePacks : null;
          const label =
            t === "mods" ? `Mods <span class="count">${S.mods.length}</span>`
            : t === "browse" ? "Browse"
            : t === "packs" ? `Resource packs${rp ? ` <span class="count">${rp.packs.length}</span>` : ""}`
            : t === "servers" ? `Servers${S.servers.has(inst.id) ? ` <span class="count">${S.servers.get(inst.id)!.length}</span>` : ""}`
            : t === "projects" ? `Claude projects <span class="count">${projectsFor(inst).length}</span>`
            : "Console";
          return `<button role="tab" class="tab ${S.tab === t ? "active" : ""}" data-action="tab" data-tab="${t}">${label}</button>`;
        })
        .join("")}
    </div>
    <section class="tab-body">${S.tab === "mods" ? modsTab(inst) : S.tab === "browse" ? browseTab(inst) : S.tab === "packs" ? packsTab(inst) : S.tab === "servers" ? serversTab(inst) : S.tab === "projects" ? projectsTab(inst) : consoleTab(inst)}</section>`;
  slide("tab", main.querySelector(".tabs"), main.querySelector(".tab.active"));
  if (mainSlide) {
    slideIn(main, 0, mainSlide * 28);
    mainSlide = 0;
  }
  if (S.tab === "browse" && S.browse.instance !== inst.id && !S.browse.loading) runSearch(false);
  if (S.tab === "servers" && !S.servers.has(inst.id)) loadServers(inst.id);
  if (S.tab === "console") consoleMounted(inst);
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
        ${c.report_path ? `<button class="btn ghost small" data-action="show-crash" data-name="${esc(c.report_path.replace(/^.*[\\/]/, ""))}">Show report</button><button class="btn ghost small" data-action="open-path" data-path="${esc(c.report_path.replace(/[\\/][^\\/]+$/, ""))}">Crash reports</button>` : `<button class="btn ghost small" data-action="tab" data-tab="console">Console</button>`}
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
  if (n > 0) return `<button class="btn primary" data-action="review-updates" title="See what's new, then update">Update all (${n})</button>`;
  if (S.updates.instance === inst.id && S.updates.checked) return '<button class="btn ghost" data-action="check-updates" title="Check again">Up to date ✓</button>';
  return '<button class="btn ghost" data-action="check-updates">Check for updates</button>';
}

async function checkUpdates() {
  const inst = current();
  if (!inst) return;
  await busy("check-updates", async () => {
    const r = await api<{ updates: ModUpdate[]; warnings: string[] }>("GET", `/instances/${enc(inst.id)}/updates`);
    S.updates = { instance: inst.id, byFile: new Map(r.updates.map((u) => [u.file_name, u])), checked: true };
    r.warnings.forEach((w) => toast(w, "err"));
    if (!r.updates.length) toast("All browser-installed mods are up to date");
  });
}


type Change = { version_id: string; version_number: string; date: string; text: string | null };
type ModUpdate = { file_name: string; title: string; source: Source; project_id: string; latest: ModVersion; changes: Change[] };

/** CurseForge changelogs already fetched, by "project/file". */
const cfChangelogs = new Map<string, string>();

function changeHtml(u: ModUpdate, c: Change): string {
  const text = c.text ?? cfChangelogs.get(`${u.project_id}/${c.version_id}`);
  const date = c.date ? new Date(c.date).toLocaleDateString() : "";
  return `
    <div class="cl-ver">
      <div class="cl-head"><strong>${esc(c.version_number)}</strong>${date ? `<span class="muted small">${esc(date)}</span>` : ""}</div>
      ${
        text === undefined
          ? `<div class="cl-text" data-cf="${esc(u.project_id)}/${esc(c.version_id)}"><span class="skel-line" style="width:70%"></span><span class="skel-line" style="width:45%"></span></div>`
          : text
            ? `<pre class="cl-text">${esc(text)}</pre>`
            : '<p class="muted small cl-text">No changelog for this version.</p>'
      }
    </div>`;
}

/** Show what an update brings before installing it. With one file, just that mod; otherwise all of them. */
function updatesModal(file?: string) {
  const all = [...S.updates.byFile.values()] as ModUpdate[];
  const list = file ? all.filter((u) => u.file_name === file) : all;
  if (!list.length) return;
  const files = list.map((u) => u.file_name);
  openModal(
    `
    <h2>${file ? `What's new in ${esc(list[0].title)}` : `${list.length} mod update${list.length === 1 ? "" : "s"}`}</h2>
    <div class="cl-list">
      ${list
        .map(
          (u) => `
        <section class="cl-mod">
          ${file ? "" : `<h3>${esc(u.title)} <span class="muted small">→ ${esc(u.latest.version_number)}</span></h3>`}
          ${u.changes.length ? u.changes.map((c) => changeHtml(u, c)).join("") : '<p class="muted small">No changelog for this update.</p>'}
          ${u.changes.length >= 8 ? '<p class="muted small">Older versions are left out.</p>' : ""}
        </section>`,
        )
        .join("")}
    </div>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="cl-go">${file ? `Update to ${esc(shortVer(list[0].latest.version_number))}` : `Update all (${list.length})`}</button>
    </div>`,
    (root) => {
      root.classList.add("wide");
      root.querySelector("#cl-go")!.addEventListener("click", () => {
        closeModal();
        applyUpdates(file ? files : undefined);
      });
      // CurseForge sends changelogs one at a time: fetch them as they scroll into view, a few at once.
      const queue: HTMLElement[] = [];
      let running = 0;
      let closed = false;
      const next = () => {
        while (!closed && running < 3 && queue.length) {
          const el = queue.shift()!;
          const key = el.dataset.cf!;
          const [mod, fileId] = key.split("/");
          running++;
          api<{ text: string }>("GET", `/changelog/curseforge/${enc(mod)}/${enc(fileId)}`)
            .then((r) => {
              cfChangelogs.set(key, r.text);
              el.outerHTML = r.text ? `<pre class="cl-text">${esc(r.text)}</pre>` : '<p class="muted small cl-text">No changelog for this version.</p>';
            })
            .catch((e) => (el.outerHTML = `<p class="muted small cl-text">Couldn't load the changelog: ${esc((e as Error).message)}</p>`))
            .finally(() => {
              running--;
              next();
            });
        }
      };
      const seen = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            if (!e.isIntersecting) continue;
            seen.unobserve(e.target);
            queue.push(e.target as HTMLElement);
          }
          next();
        },
        { root: root.querySelector(".cl-list"), rootMargin: "150px" },
      );
      root.querySelectorAll<HTMLElement>(".cl-text[data-cf]").forEach((el) => seen.observe(el));
      modalCleanup = () => {
        closed = true;
        seen.disconnect();
      };
    },
  );
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
    return `<div class="note">Vanilla instances can't load mods. Create an instance with a mod loader to use mods.</div>`;
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
          ${updateFor(inst, m) ? `<button class="btn ghost small" data-action="whats-new" data-file="${esc(m.file_name)}">What's new</button>` : ""}
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
    <div class="toolbar wrap mods-bar">
      <input id="mod-filter" class="input" placeholder="Search mods" value="${esc(S.modFilter)}" data-action="filter-mods"/>
      <span class="muted mods-count">${S.mods.filter((m) => m.enabled).length} enabled · ${claudeCount} by Claude</span>
      ${modSetPicker(inst)}
      ${updatesButton(inst)}
      <button class="btn ghost" data-action="open-path" data-path="${esc(inst.mods_dir)}">Open mods folder</button>
    </div>
    ${modProblems(inst)}
    ${
      S.mods.length
        ? `<div class="mod-list">${rows || '<p class="muted pad">No mods match.</p>'}</div>`
        : `<div class="note">No mods yet. Drop .jar files into the mods folder, or ask Claude Code to make one, for example: <code>Make a ${loaderLabel(inst.loader)} mod for my "${esc(inst.name)}" instance that makes ores glow</code></div>`
    }`;
}

function modProblems(inst: Instance): string {
  const c = S.modCheck;
  if (!c || c.for !== inst.id || !c.problems.length) return "";
  const n = c.problems.length;
  return `<div class="note err mod-problems">
      <strong>${n === 1 ? "1 problem" : `${n} problems`} with these mods</strong>
      <ul>${c.problems.map((p) => `<li>${esc(p.message)}</li>`).join("")}</ul>
    </div>`;
}

// ---------- mod sets ----------

function modSetPicker(inst: Instance): string {
  const v = S.modSets;
  if (!v || v.for !== inst.id || !S.mods.length) return "";
  const running = S.running.has(`game:${inst.id}`);
  const working = S.busy.has(`modset:${inst.id}`);
  const title = running
    ? "Close the game to change mod sets"
    : "Mod sets choose which mods are on. Turning a mod on or off changes the set you're in.";
  return `<label class="set-pick" title="${esc(title)}">
      <span class="muted">Mod set</span>
      <select class="input" data-action="mod-set" ${running || working ? "disabled" : ""}>
        ${v.active ? "" : `<option value="" selected>${v.sets.length ? "None" : "No sets yet"}</option>`}
        ${v.sets.map((s) => `<option value="set:${esc(s.name)}" ${s.name === v.active ? "selected" : ""}>${esc(s.name)} (${s.enabled} on)</option>`).join("")}
        <option value="new">+ New set from the mods on now…</option>
        ${v.active ? `<option value="rename">Rename "${esc(v.active)}"…</option><option value="remove">Delete "${esc(v.active)}"…</option>` : ""}
      </select>
    </label>`;
}

async function modSetCall(inst: Instance, path: string, body: object) {
  await busy(`modset:${inst.id}`, async () => {
    const v = await api<ModSetsView>("POST", `/instances/${enc(inst.id)}/mod-sets${path}`, body);
    if (S.selected === inst.id) S.modSets = { ...v, for: inst.id };
  });
}

function modSetNameModal(inst: Instance, renaming: string | null) {
  openModal(`
    <h2>${renaming ? "Rename mod set" : "New mod set"}</h2>
    ${renaming ? "" : '<p class="muted small">It starts with the mods that are on now. While a set is chosen, turning mods on or off changes that set; switch sets to turn a whole group on or off at once. A mod you add joins the set you\'re in, and is off in the others.</p>'}
    <label class="field"><span>Name</span><input id="ms-name" class="input" maxlength="40" placeholder="${renaming ? "" : S.modSets?.sets.length ? "Light" : "Full"}" value="${esc(renaming ?? "")}"/></label>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="ms-ok">${renaming ? "Rename" : "Create"}</button>
    </div>`);
  const input = $<HTMLInputElement>("#ms-name");
  input.focus();
  input.select();
  const go = async () => {
    const name = input.value.trim();
    if (!name) return input.focus();
    closeModal();
    await modSetCall(inst, renaming ? "/rename" : "", renaming ? { name: renaming, new_name: name } : { name });
  };
  $("#ms-ok").addEventListener("click", go);
  input.addEventListener("keydown", (e) => e.key === "Enter" && go());
}

async function removeModSet(inst: Instance, name: string) {
  const ok = await ask({
    title: `Delete the set "${name}"?`,
    body: "The mods stay as they are now; only the set is forgotten.",
    ok: "Delete set",
    danger: true,
  });
  if (ok) await modSetCall(inst, "/remove", { name });
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
  const B = S.browse;
  const vanilla = inst.loader === "vanilla";
  // Vanilla runs resource packs but no mods or shaders.
  if (vanilla && B.kind !== "resourcepack") B.kind = "resourcepack";
  const needsKey = B.source === "curseforge" && !S.hasCfKey;
  const shader = B.kind === "shader";
  const rpack = B.kind === "resourcepack";
  const noun = shader ? "shader packs" : rpack ? "resource packs" : "mods";
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
        ${vanilla ? "" : `<button data-action="browse-kind" data-v="mod" class="${B.kind === "mod" ? "on" : ""}">Mods</button>
        <button data-action="browse-kind" data-v="shader" class="${shader ? "on" : ""}">Shaders</button>`}
        <button data-action="browse-kind" data-v="resourcepack" class="${rpack ? "on" : ""}">Resource packs</button>
      </div>
      <div class="seg small-seg">
        <button data-action="browse-source" data-v="modrinth" class="${B.source === "modrinth" ? "on" : ""}">Modrinth</button>
        <button data-action="browse-source" data-v="curseforge" class="${B.source === "curseforge" ? "on" : ""}">CurseForge</button>
      </div>
      <input id="browse-q" class="input" placeholder="Search ${sourceLabel(B.source)} ${noun} for ${esc(inst.mc_version)}${B.kind === "mod" ? " " + loaderLabel(inst.loader) : ""}" value="${esc(B.q)}" data-action="browse-q" ${needsKey ? "disabled" : ""}/>
      <select id="browse-sort" class="input sort" data-action="browse-sort" ${needsKey ? "disabled" : ""}>
        ${[["relevance", "Relevance"], ["downloads", "Downloads"], ["updated", "Recently updated"], ["newest", "Newest"]]
          .map(([v, l]) => `<option value="${v}" ${B.sort === v ? "selected" : ""}>${l}</option>`)
          .join("")}
      </select>
    </div>
    ${shader ? shaderPanel() : ""}
    ${
      needsKey
        ? `<div class="note">CurseForge needs a free API key. Get one at <button class="link" data-action="open-url" data-url="https://console.curseforge.com/">console.curseforge.com</button>, then add it in <button class="link" data-action="settings">Settings</button>.</div>`
        : B.error
          ? `<div class="note err">${esc(B.error)}</div>`
          : B.hits.length
            ? `<p class="muted small">${rpack ? `Showing resource packs for Minecraft ${esc(inst.mc_version)}. Installed packs show in the Resource packs tab, where you turn them on.` : shader ? `Showing shader packs for Minecraft ${esc(inst.mc_version)}.` : `Showing mods for Minecraft ${esc(inst.mc_version)} with ${loaderLabel(inst.loader)}. Required dependencies install automatically.`}</p>
               <div class="mod-list">${results}</div>
               ${B.hits.length < B.total ? `<button class="btn ghost more" data-action="browse-more" ${B.loading ? "disabled" : ""}>${B.loading ? "Loading…" : "Load more"}</button>` : ""}`
            : `<p class="muted pad">${B.loading ? "Searching…" : `No ${noun} found.`}</p>`
    }`;
}

function packsTab(inst: Instance): string {
  const rp = S.resourcePacks?.for === inst.id ? S.resourcePacks : null;
  if (!rp) return `<div class="mod-list">${'<div class="mod"><span class="pack-icon skel-box"></span><div class="mod-text"><span class="skel-line" style="width:40%"></span><span class="skel-line" style="width:65%"></span></div></div>'.repeat(3)}</div>`;
  const running = S.running.has(`game:${inst.id}`);
  const on = rp.packs.filter((p) => p.enabled).length;
  const rows = rp.packs
    .map((p) => {
      const key = `rpack:${p.file_name}`;
      return `
      <div class="mod ${p.enabled ? "" : "disabled"}">
        <label class="switch" title="${running ? "Close the game to change which packs are on" : p.enabled ? "Turn off" : "Turn on"}">
          <input type="checkbox" ${p.enabled ? "checked" : ""} data-action="toggle-rpack" data-file="${esc(p.file_name)}" ${running || S.busy.has(key) ? "disabled" : ""}/>
          <span></span>
        </label>
        ${p.icon ? `<img class="pack-icon" src="${esc(p.icon)}" alt=""/>` : '<div class="pack-icon blank"></div>'}
        <div class="mod-text">
          <div class="mod-title">
            <span class="mod-name">${esc(p.online?.title ?? p.file_name.replace(/\.zip$/i, ""))}</span>
            ${p.online ? `<button class="badge src ${p.online.source}" data-action="open-url" data-url="${esc(p.online.page_url)}" title="Open on ${sourceLabel(p.online.source)}">${sourceLabel(p.online.source)}</button>` : ""}
          </div>
          ${p.description ? `<div class="mod-desc">${esc(p.description)}</div>` : ""}
          <div class="mod-file">${esc(p.file_name)}${p.size ? ` · ${fmtSize(p.size)}` : ""}</div>
        </div>
        <div class="mod-actions">
          <button class="icon-btn" data-action="delete-rpack" data-file="${esc(p.file_name)}" title="Delete" aria-label="Delete resource pack" ${S.busy.has(key) ? "disabled" : ""}>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/></svg>
          </button>
        </div>
      </div>`;
    })
    .join("");
  return `
    <div class="toolbar">
      <span class="muted">${on} on · ${rp.packs.length} installed</span>
      <button class="btn primary" data-action="find-rpacks">Find resource packs</button>
      <button class="btn ghost" data-action="open-path" data-path="${esc(rp.folder)}">Open resourcepacks folder</button>
    </div>
    ${running ? '<div class="note">The game is running. Close it to turn packs on or off here, or change them in game under Options → Resource Packs.</div>' : ""}
    ${
      rp.packs.length
        ? `<div class="mod-list">${rows}</div><p class="muted small">Packs you turn on here are on the next time the game starts. Packs lower in the game's list are drawn on top; a pack you turn on here goes on top.</p>`
        : `<div class="note">No resource packs yet. Find some with <button class="link" data-action="find-rpacks">Find resource packs</button>, or drop .zip files onto the launcher.</div>`
    }`;
}

function shaderPanel(): string {
  const sh = S.shaders;
  const iris = S.busy.has(`get:modrinth:${IRIS_ID}`);
  const packs = sh.packs
    .map(
      (p) => `
      <div class="mod">
        <div class="mod-text">
          <div class="mod-title">
            <span class="mod-name">${esc(p.online?.title ?? p.file_name.replace(/\.zip$/i, ""))}</span>
            ${p.online ? `<button class="badge src ${p.online.source}" data-action="open-url" data-url="${esc(p.online.page_url)}" title="Open on ${sourceLabel(p.online.source)}">${sourceLabel(p.online.source)}</button>` : ""}
          </div>
          <div class="mod-file">${esc(p.file_name)}${p.size ? ` · ${fmtSize(p.size)}` : ""}</div>
        </div>
        <div class="mod-actions">
          <button class="icon-btn" data-action="delete-shader" data-file="${esc(p.file_name)}" title="Delete" aria-label="Delete shader pack">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/></svg>
          </button>
        </div>
      </div>`,
    )
    .join("");
  return `
    ${
      sh.has_loader
        ? ""
        : `<div class="note">Shader packs need the <strong>Iris</strong> mod to work. <button class="btn small primary" data-action="install-iris" ${iris ? "disabled" : ""}>${iris ? "Installing…" : "Install Iris"}</button></div>`
    }
    ${
      sh.packs.length
        ? `<div class="toolbar">
             <strong>Installed shader packs</strong><span class="muted">${sh.packs.length}</span>
             <span class="grow"></span>
             <button class="btn ghost" data-action="open-path" data-path="${esc(sh.folder)}">Open shaderpacks folder</button>
           </div>
           <div class="mod-list">${packs}</div>
           <p class="muted small">Turn a pack on in the game under Options → Video Settings → Shader Packs.</p>`
        : ""
    }`;
}

let searchSeq = 0;
async function runSearch(more: boolean) {
  const inst = current();
  const B = S.browse;
  if (inst?.loader === "vanilla") B.kind = "resourcepack";
  if (!inst) return;
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
      `/browse/${B.source}/search?instance_id=${enc(inst.id)}&q=${enc(B.q)}&sort=${B.sort}&offset=${more ? B.hits.length : 0}&limit=20&kind=${B.kind}`,
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
  const shader = S.browse.kind === "shader";
  const rpack = S.browse.kind === "resourcepack";
  await busy(`get:${source}:${pid}`, async () => {
    const r = await api<{ installed: string[]; already_installed: string[]; warnings: string[] }>(
      "POST",
      `/instances/${enc(inst.id)}/${shader ? "shaders" : rpack ? "resourcepacks" : "online-mods"}`,
      { source, project_id: pid, version_id: versionId },
    );
    const hit = S.browse.hits.find((h) => h.project_id === pid);
    if (hit) hit.installed = true;
    const deps = r.installed.length - 1;
    if (r.installed.length) toast(`Installed ${hit?.title ?? r.installed[0]}${deps > 0 ? ` and ${deps} dependenc${deps === 1 ? "y" : "ies"}` : ""}`);
    else toast(`${hit?.title ?? (shader ? "That shader pack" : rpack ? "That resource pack" : "That mod")} is already installed`);
    r.warnings.forEach((w) => toast(w, "err"));
  });
}

async function versionsModal(pid: string, title: string) {
  const inst = current();
  if (!inst) return;
  const source = S.browse.source;
  openModal(`<h2>${esc(title)}</h2><p class="muted">Loading versions…</p>`);
  try {
    const vs = await api<ModVersion[]>("GET", `/browse/${source}/projects/${enc(pid)}/versions?instance_id=${enc(inst.id)}&kind=${S.browse.kind}`);
    openModal(`
      <h2>${esc(title)}</h2>
      <p class="muted small">Versions for Minecraft ${esc(inst.mc_version)}${S.browse.kind === "mod" ? ` with ${loaderLabel(inst.loader)}` : ""}</p>
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
  if (inst.mod_template_unsupported) return `<div class="note">${esc(inst.mod_template_unsupported)}</div>`;
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

// ---------- servers (the game's own multiplayer list, servers.dat) ----------

let serversLoading = new Set<string>();

function loadServers(id: string) {
  if (serversLoading.has(id)) return;
  serversLoading.add(id);
  api<Server[]>("GET", `/instances/${enc(id)}/servers`)
    .then((list) => S.servers.set(id, list))
    .catch((e) => {
      S.servers.set(id, []);
      toast((e as Error).message, "err");
    })
    .finally(() => {
      serversLoading.delete(id);
      if (S.selected === id && S.tab === "servers") renderMain();
    });
}

const SERVER_ICON = `<svg class="pix-icon" width="22" height="22" viewBox="0 0 11 11" shape-rendering="crispEdges" fill="currentColor"><path d="M1 1h9v4h-9zM1 6h9v4h-9z" opacity=".35"/><path d="M2 2h1v2h-1zM2 7h1v2h-1zM8 2h1v1h-1zM8 7h1v1h-1z"/></svg>`;

function serversTab(inst: Instance): string {
  const list = S.servers.get(inst.id);
  const running = S.running.has(`game:${inst.id}`);
  const launching = S.busy.has(`launch:${inst.id}`);
  const rows = (list ?? [])
    .map(
      (s) => `
      <div class="mod srv">
        <span class="srv-icon">${s.icon ? `<img src="data:image/png;base64,${s.icon}" alt="" width="40" height="40"/>` : SERVER_ICON}</span>
        <div class="mod-text">
          <div class="mod-title"><span class="mod-name">${esc(s.name)}</span></div>
          <div class="mod-file mono">${esc(s.address)}</div>
        </div>
        <div class="mod-actions">
          <button class="btn small play" data-action="join-server" data-address="${esc(s.address)}" ${running || launching ? "disabled" : ""} title="Start the game and join ${esc(s.name)}">${PLAY_ICON} Join</button>
          <button class="btn small ghost" data-action="edit-server" data-index="${s.index}" ${running ? "disabled" : ""}>Edit</button>
          <button class="icon-btn" data-action="remove-server" data-index="${s.index}" ${running ? "disabled" : ""} title="Remove" aria-label="Remove server">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/></svg>
          </button>
        </div>
      </div>`,
    )
    .join("");
  return `
    <div class="toolbar">
      <span class="muted grow">${running ? "The game is running. Close it to change the list." : "The same list as Multiplayer in the game."}</span>
      <button class="btn" data-action="add-server" ${running ? "disabled" : ""}>Add server</button>
    </div>
    ${
      !list
        ? '<div class="mod-list">' + '<div class="mod srv"><span class="srv-icon skel-box"></span><div class="mod-text"><span class="skel-line" style="width:40%"></span><span class="skel-line" style="width:25%"></span></div></div>'.repeat(3) + "</div>"
        : list.length
          ? `<div class="mod-list">${rows}</div>`
          : `<div class="note">No servers yet. Add one and it also shows up under Multiplayer in the game.</div>`
    }`;
}

/** Show an instance on the Instances page with one of its tabs open. */
async function openTab(inst: Instance, tab: Tab) {
  if (settingsOpen) closeSettings();
  if (S.view !== "instances") setView("instances", true);
  if (S.selected !== inst.id) await selectInstance(inst.id);
  S.tab = tab;
  renderMain();
}

function serverModal(inst: Instance, s?: Server) {
  openModal(`
    <h2>${s ? "Edit server" : "Add server"}</h2>
    <label class="field"><span>Name</span><input id="sv-name" class="input" placeholder="Minecraft Server" value="${esc(s?.name ?? "")}" maxlength="64"/></label>
    <label class="field"><span>Address</span><input id="sv-addr" class="input mono" placeholder="play.example.net" value="${esc(s?.address ?? "")}" maxlength="255" spellcheck="false" autocapitalize="off"/></label>
    <p class="muted small">Add <code>:port</code> if the server doesn't use the usual one, like <code>play.example.net:25570</code>.</p>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="sv-save">${s ? "Save" : "Add server"}</button>
    </div>`);
  const go = async () => {
    const btn = $<HTMLButtonElement>("#sv-save");
    const body = { name: $<HTMLInputElement>("#sv-name").value, address: $<HTMLInputElement>("#sv-addr").value };
    btn.disabled = true;
    try {
      const list = s
        ? await api<Server[]>("PATCH", `/instances/${enc(inst.id)}/servers/${s.index}`, body)
        : await api<Server[]>("POST", `/instances/${enc(inst.id)}/servers`, body);
      S.servers.set(inst.id, list);
      closeModal();
      renderMain();
    } catch (e) {
      btn.disabled = false;
      toast((e as Error).message, "err");
    }
  };
  $("#sv-save").addEventListener("click", go);
  for (const id of ["#sv-name", "#sv-addr"]) $(id).addEventListener("keydown", (e) => (e as KeyboardEvent).key === "Enter" && go());
  $<HTMLInputElement>(s ? "#sv-name" : "#sv-addr").focus();
}

// ---------- console: live output, older logs and crash reports ----------

type LogFile = { kind: "log" | "crash" | "launcher"; name: string; size: number; modified: number };
type ConOnly = "all" | "game" | "build" | "dev";

/** What the Console tab shows. */
const C = {
  /** "" for live output, otherwise "kind/name" of a log file. */
  file: "",
  files: null as LogFile[] | null,
  filesFor: "",
  /** The open file's lines (null while loading). */
  text: null as string[] | null,
  truncated: false,
  folder: "",
  q: "",
  only: "all" as ConOnly,
  /** Which error the arrows last jumped to. */
  err: -1,
};

/** At most this many lines are drawn; searching still covers the whole file. */
const CON_DRAW = 20000;

function resetConsole() {
  Object.assign(C, { file: "", files: null, filesFor: "", text: null, truncated: false, folder: "", q: "", only: "all", err: -1 });
}

function lineKind(line: string): "err" | "warn" | "" {
  return /\b(ERROR|FATAL|Exception|error:|FAILURE)\b/.test(line) ? "err" : /\bWARN\b/.test(line) ? "warn" : "";
}

function showSource(source: string): boolean {
  return C.only === "all" || source.startsWith(`${C.only}:`);
}

function matches(line: string): boolean {
  return !C.q || line.toLowerCase().includes(C.q.toLowerCase());
}

/** Escape a line and mark what the search found. */
function marked(line: string): string {
  if (!C.q) return esc(line);
  const lower = line.toLowerCase();
  const q = C.q.toLowerCase();
  let out = "";
  let at = 0;
  for (let i = lower.indexOf(q); i !== -1; i = lower.indexOf(q, at)) {
    out += esc(line.slice(at, i)) + `<mark>${esc(line.slice(i, i + q.length))}</mark>`;
    at = i + q.length;
  }
  return out + esc(line.slice(at));
}

function logLine(source: string, line: string): string {
  const tag = source ? `<span class="src">${esc(source.split(":")[0])}</span>` : "";
  return `<span class="ln ${lineKind(line)}">${tag}${marked(line)}\n</span>`;
}

const fileLabel = (f: LogFile) =>
  f.kind === "launcher" ? "Last launch (launcher output)" : f.name === "latest.log" ? "latest.log (newest)" : f.name;

function consoleTab(inst: Instance): string {
  if (C.filesFor !== inst.id) {
    resetConsole();
    C.filesFor = inst.id;
  }
  if (!C.files) loadLogFiles(inst);
  const live = !C.file;
  const group = (kind: LogFile["kind"], label: string) => {
    const fs = (C.files ?? []).filter((f) => f.kind === kind);
    return fs.length
      ? `<optgroup label="${label}">${fs.map((f) => `<option value="${esc(`${f.kind}/${f.name}`)}" ${C.file === `${f.kind}/${f.name}` ? "selected" : ""}>${esc(fileLabel(f))}</option>`).join("")}</optgroup>`
      : "";
  };
  const hasProjects = projectsFor(inst).length > 0;
  const chips: [ConOnly, string][] = [["all", "All"], ["game", "Game"], ...(hasProjects ? ([["build", "Builds"], ["dev", "Test client"]] as [ConOnly, string][]) : [])];
  return `
    <div class="toolbar console-bar">
      <select id="con-file" class="input con-file" aria-label="Which log to show">
        <option value="" ${live ? "selected" : ""}>Live output</option>
        ${C.files ? group("log", "Game logs") + group("crash", "Crash reports") + group("launcher", "Launcher") : '<option disabled>Looking for older logs…</option>'}
      </select>
      ${live && chips.length > 2 ? `<div class="seg con-only">${chips.map(([v, l]) => `<button data-action="con-only" data-v="${v}" class="${C.only === v ? "on" : ""}">${l}</button>`).join("")}</div>` : ""}
      <input id="con-q" class="input con-q" placeholder="Search the log" value="${esc(C.q)}" spellcheck="false"/>
      <span class="con-errs" id="con-errs"></span>
      ${live ? '<button class="btn ghost small" data-action="clear-console">Clear</button>' : C.folder ? `<button class="btn ghost small" data-action="open-path" data-path="${esc(C.folder)}">Open folder</button>` : ""}
    </div>
    <pre id="console" class="console"></pre>`;
}

function loadLogFiles(inst: Instance) {
  const id = inst.id;
  api<LogFile[]>("GET", `/instances/${enc(id)}/log-files`)
    .then((files) => {
      if (C.filesFor !== id) return;
      C.files = files;
      if (S.tab === "console" && current()?.id === id) {
        const sel = document.querySelector<HTMLSelectElement>("#con-file");
        // Refresh only the picker, so typing in the search box isn't interrupted.
        if (sel) {
          const fresh = document.createElement("div");
          fresh.innerHTML = consoleTab(inst);
          sel.replaceWith(fresh.querySelector("#con-file")!);
          bindPicker(inst);
        }
      }
    })
    .catch(() => (C.files = []));
}

async function openLogFile(inst: Instance, key: string) {
  if (C.filesFor !== inst.id) {
    resetConsole();
    C.filesFor = inst.id;
  }
  // A file the picker doesn't know yet (a fresh crash report): list them again.
  if (key && C.files && !C.files.some((f) => `${f.kind}/${f.name}` === key)) C.files = null;
  C.file = key;
  C.text = null;
  C.err = -1;
  C.folder = "";
  C.truncated = false;
  if (S.view !== "instances") setView("instances");
  renderMain();
  if (!key) return;
  const [kind, ...rest] = key.split("/");
  try {
    const r = await api<{ text: string; truncated: boolean; folder: string }>("GET", `/instances/${enc(inst.id)}/log-files/${enc(kind)}/${enc(rest.join("/"))}`);
    if (C.file !== key) return;
    C.text = r.text.split(/\r?\n/);
    if (C.text[C.text.length - 1] === "") C.text.pop();
    C.truncated = r.truncated;
    C.folder = r.folder;
  } catch (e) {
    if (C.file !== key) return;
    C.text = [];
    toast((e as Error).message, "err");
  }
  if (S.tab === "console") renderMain();
}

/** Fill the console with the lines that pass the source filter and the search. */
function drawConsole(inst: Instance, scroll: "end" | "keep" = "end") {
  const c = document.querySelector<HTMLElement>("#console");
  if (!c) return;
  let lines: { s: string; l: string }[];
  let empty: string;
  if (!C.file) {
    const sources = [`game:${inst.id}`, ...projectsFor(inst).flatMap((p) => [`build:${p.id}`, `dev:${p.id}`])].filter(showSource);
    lines = sources.flatMap((s) => (S.logs.get(s) ?? []).map((l) => ({ s, l })));
    empty = "Nothing yet. Press Play or build a mod. Older logs and crash reports are in the menu at the top left.";
  } else if (!C.text) {
    c.innerHTML = '<span class="con-empty"><span class="skel-line" style="width:60%"></span><span class="skel-line" style="width:40%"></span><span class="skel-line" style="width:52%"></span></span>';
    updateErrors();
    return;
  } else {
    lines = C.text.map((l) => ({ s: "", l }));
    empty = "This log is empty.";
  }
  const found = C.q ? lines.filter(({ l }) => matches(l)) : lines;
  const drawn = found.length > CON_DRAW ? found.slice(-CON_DRAW) : found;
  const notes = [
    C.truncated ? "Only the end of this log is shown, since it's very long." : "",
    found.length > CON_DRAW ? `Showing the last ${CON_DRAW.toLocaleString()} of ${found.length.toLocaleString()} lines. Search to narrow it down.` : "",
  ].filter(Boolean);
  c.innerHTML = found.length
    ? notes.map((n) => `<span class="con-note">${esc(n)}</span>`).join("") + drawn.map(({ s, l }) => logLine(s, l)).join("")
    : `<span class="con-empty muted">${C.q ? `No lines contain “${esc(C.q)}”.` : esc(empty)}</span>`;
  C.err = -1;
  updateErrors();
  if (scroll === "end") c.scrollTop = c.scrollHeight;
}

function updateErrors() {
  const box = document.querySelector<HTMLElement>("#con-errs");
  if (!box) return;
  const n = document.querySelectorAll("#console .ln.err").length;
  box.innerHTML = n
    ? `<span class="err-n small">${n} error${n === 1 ? "" : "s"}</span>
       <button class="icon-btn" data-action="con-err" data-dir="up" title="Previous error" aria-label="Previous error"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="m6 15 6-6 6 6"/></svg></button>
       <button class="icon-btn" data-action="con-err" data-dir="down" title="Next error (from the top)" aria-label="Next error"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="m6 9 6 6 6-6"/></svg></button>`
    : '<span class="muted small">No errors</span>';
}

/** Scroll to the next or previous error line. The first press goes to the first error. */
function jumpError(dir: 1 | -1) {
  const errs = [...document.querySelectorAll<HTMLElement>("#console .ln.err")];
  if (!errs.length) return;
  C.err = C.err < 0 ? (dir === 1 ? 0 : errs.length - 1) : (C.err + dir + errs.length) % errs.length;
  const el = errs[C.err];
  document.querySelectorAll("#console .ln.flash").forEach((x) => x.classList.remove("flash"));
  el.classList.add("flash");
  el.scrollIntoView({ block: "center" });
}

let conSearchTimer: number | undefined;

function bindPicker(inst: Instance) {
  document.querySelector<HTMLSelectElement>("#con-file")?.addEventListener("change", (ev) => {
    openLogFile(inst, (ev.target as HTMLSelectElement).value);
  });
}

function bindConsole(inst: Instance) {
  bindPicker(inst);
  const q = document.querySelector<HTMLInputElement>("#con-q");
  q?.addEventListener("input", () => {
    C.q = q.value;
    clearTimeout(conSearchTimer);
    conSearchTimer = window.setTimeout(() => drawConsole(inst), C.file ? 120 : 0);
  });
  q?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") jumpError(e.shiftKey ? -1 : 1);
    if (e.key === "Escape" && q.value) {
      e.stopPropagation();
      q.value = C.q = "";
      drawConsole(inst);
    }
  });
}

/** After the Console tab is drawn: fill it and hook up its controls. */
function consoleMounted(inst: Instance | null) {
  if (!inst) return;
  bindConsole(inst);
  drawConsole(inst);
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
      if (c && inst && !C.file && (e.source === `game:${inst.id}` || projectsFor(inst).some((p) => e.source.endsWith(`:${p.id}`)))) {
        if (!showSource(e.source) || !matches(e.line)) break;
        const stick = c.scrollTop + c.clientHeight >= c.scrollHeight - 30;
        if (c.querySelector(".con-empty")) c.innerHTML = "";
        c.insertAdjacentHTML("beforeend", logLine(e.source, e.line));
        while (c.childElementCount > LOG_LIMIT) c.firstElementChild?.remove();
        if (stick) c.scrollTop = c.scrollHeight;
        if (lineKind(e.line) === "err") updateErrors();
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
      if (e.source.startsWith("game:")) {
        S.servers.delete(e.source.slice(5));
        // The game wrote a new log (and maybe a crash report).
        if (C.filesFor === e.source.slice(5)) C.files = null;
      }
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

/**
 * Ask a yes/no question in the launcher's own style, on top of whatever is
 * open (so an Edit dialog behind it keeps its changes). `body` is HTML.
 * Resolves true for yes; Esc, Cancel and clicking outside are no.
 */
function ask(o: { title: string; body: string; ok: string; danger?: boolean }): Promise<boolean> {
  return new Promise((done) => {
    const box = document.createElement("div");
    box.className = "ask-root";
    box.innerHTML = `<div class="backdrop ask-backdrop"></div>
      <div class="modal ask" role="alertdialog" aria-modal="true" aria-labelledby="ask-title" aria-describedby="ask-body">
        <h2 id="ask-title">${esc(o.title)}</h2>
        <p id="ask-body">${o.body}</p>
        <div class="modal-actions">
          <button class="btn ghost" data-ask="no">Cancel</button>
          <button class="btn ${o.danger ? "danger" : "primary"}" data-ask="yes">${esc(o.ok)}</button>
        </div>
      </div>`;
    document.body.appendChild(box);
    const before = document.activeElement as HTMLElement | null;
    const buttons = [...box.querySelectorAll<HTMLButtonElement>("[data-ask]")];
    const finish = (yes: boolean) => {
      document.removeEventListener("keydown", onKey, true);
      box.remove();
      before?.focus?.();
      done(yes);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        finish(false);
      } else if (e.key === "Tab") {
        // Keep the focus on the two buttons.
        e.preventDefault();
        buttons[(buttons.indexOf(document.activeElement as HTMLButtonElement) + 1) % buttons.length].focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    box.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      const a = t.closest<HTMLElement>("[data-ask]")?.dataset.ask;
      if (a) finish(a === "yes");
      else if (t.classList.contains("ask-backdrop")) finish(false);
    });
    // For something that can't be undone, Enter shouldn't do it by accident.
    buttons[o.danger ? 0 : 1].focus();
  });
}

/** Run when the open modal closes or is replaced (to stop a 3D preview, say). */
let modalCleanup: (() => void) | null = null;

function openModal(html: string, onMount?: (root: HTMLElement) => void) {
  modalCleanup?.();
  modalCleanup = null;
  const root = $("#modal-root");
  root.innerHTML = `<div class="backdrop" data-action="close-modal"></div><div class="modal" role="dialog" aria-modal="true">${html}</div>`;
  root.classList.add("show");
  onMount?.(root.querySelector(".modal") as HTMLElement);
}

function closeModal() {
  modalCleanup?.();
  modalCleanup = null;
  const root = $("#modal-root");
  root.classList.remove("show");
  root.innerHTML = "";
}

/** Describe a pack; Claude Code creates the instance, finds the mods, checks them and test-launches it. */
function modpackModal() {
  openModal(`
    <h2>✦ Build a modpack with Claude</h2>
    <p class="muted small">Claude Code makes a new instance, finds the mods on Modrinth or CurseForge, checks that nothing is missing, and starts the game once to make sure it loads.</p>
    <label class="field"><span>What should the pack be?</span><textarea id="mp-idea" class="input" rows="4" placeholder="e.g. A cozy farming pack with better animals, cooking and seasons, and good performance"></textarea></label>
    <label class="field"><span>Name</span><input id="mp-name" class="input" maxlength="60" placeholder="Claude picks one"/></label>
    <div class="row">
      <label class="field grow"><span>Minecraft version</span><select id="mp-mc" class="input"><option value="">Claude picks</option></select></label>
      <label class="field grow"><span>Mod loader</span><select id="mp-loader" class="input"><option value="">Claude picks</option><option value="fabric">Fabric</option><option value="neoforge">NeoForge</option><option value="forge">Forge</option><option value="quilt">Quilt</option></select></label>
    </div>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn ghost" id="mp-copy">Copy prompt</button>
      <button class="btn primary" id="mp-open">Open in Claude Code</button>
    </div>`);
  $<HTMLTextAreaElement>("#mp-idea").focus();
  api<{ versions: { id: string }[] }>("GET", "/versions")
    .then((vs) => {
      const sel = document.querySelector<HTMLSelectElement>("#mp-mc");
      if (sel) sel.insertAdjacentHTML("beforeend", vs.versions.slice(0, 60).map((v) => `<option>${esc(v.id)}</option>`).join(""));
    })
    .catch(() => {});
  const go = async (dryRun: boolean) => {
    const request = $<HTMLTextAreaElement>("#mp-idea").value;
    if (!request.trim()) {
      toast("Describe the pack first", "err");
      return $<HTMLTextAreaElement>("#mp-idea").focus();
    }
    const modpack = {
      name: $<HTMLInputElement>("#mp-name").value.trim() || null,
      mc_version: $<HTMLSelectElement>("#mp-mc").value || null,
      loader: $<HTMLSelectElement>("#mp-loader").value || null,
    };
    if (!dryRun) closeModal();
    await openClaude({ request, modpack }, dryRun);
  };
  $("#mp-open").addEventListener("click", () => go(false));
  $("#mp-copy").addEventListener("click", () => go(true));
}

async function newInstanceModal() {
  openModal(`
    <h2>New instance</h2>
    <button type="button" class="claude-pack" id="ni-claude">✦ Or describe a modpack and let Claude build it…</button>
    <label class="field"><span>Name</span><input id="ni-name" class="input" placeholder="My modpack"/></label>
    <div class="field"><span>Mod loader</span>
      <div class="seg wrap" id="ni-loader">
        <button data-v="fabric" class="on">Fabric</button><button data-v="neoforge">NeoForge</button><button data-v="forge">Forge</button><button data-v="quilt">Quilt</button><button data-v="legacyfabric" title="Fabric for Minecraft 1.3 to 1.13.2">Legacy Fabric</button><button data-v="vanilla">Vanilla</button>
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

  $("#ni-claude").addEventListener("click", modpackModal);
  const mc = $<HTMLSelectElement>("#ni-mc");
  const lv = $<HTMLSelectElement>("#ni-lv");
  const hint = $("#ni-hint");
  let loader = "fabric";
  // Answers can arrive out of order when the loader or version changes quickly.
  let loadSeq = 0;

  const loadLoaders = async () => {
    const seq = ++loadSeq;
    lv.innerHTML = "";
    hint.textContent = "";
    if (loader === "vanilla") {
      lv.disabled = true;
      return;
    }
    lv.disabled = true;
    lv.innerHTML = "<option>Loading…</option>";
    try {
      const v = await api<{ version: string; stable: boolean; recommended: boolean }[]>("GET", `/loaders/${loader}/${enc(mc.value)}`);
      if (seq !== loadSeq) return;
      if (!v.length) {
        lv.innerHTML = "<option value=''>Not available</option>";
        hint.textContent = loaderUnsupported(loader, mc.value);
        return;
      }
      const pick = (v.find((x) => x.recommended) ?? v.find((x) => x.stable) ?? v[0]).version;
      const label = (x: { version: string; stable: boolean; recommended: boolean }) => (x.recommended ? " (recommended)" : x.stable ? "" : " (beta)");
      // Keep the picked version in the list even when it's older than the newest 40.
      const shown = v.slice(0, 40);
      if (!shown.some((x) => x.version === pick)) shown.push(v.find((x) => x.version === pick)!);
      lv.innerHTML = shown.map((x) => `<option value="${esc(x.version)}" ${x.version === pick ? "selected" : ""}>${esc(x.version)}${label(x)}</option>`).join("");
      lv.disabled = false;
      const support = await api<{ unsupported_reason: string | null }>("GET", `/mod-templates/${loader}/${enc(mc.value)}`).catch(() => null);
      if (seq !== loadSeq) return;
      if (support?.unsupported_reason) hint.textContent = support.unsupported_reason;
    } catch (e) {
      if (seq !== loadSeq) return;
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

type ClaudeCodeStatus = { claude_path: string | null; node_path: string | null; connected: boolean };

/** The last Claude Code check (undefined = not done yet). */
let lastCc: ClaudeCodeStatus | null | undefined;
let ccCheck: Promise<void> | null = null;

/** Check Claude Code (runs it, so it takes a few seconds) and update Settings if it's open. */
function refreshClaudeCode(): Promise<void> {
  ccCheck ??= api<ClaudeCodeStatus>("GET", "/claude-code")
    .catch(() => null)
    .then((cc) => {
      lastCc = cc;
      const line = document.querySelector("#cc-line");
      if (line) line.innerHTML = claudeCodeBlock(cc);
    })
    .finally(() => (ccCheck = null));
  return ccCheck;
}

function manualLine(cc: ClaudeCodeStatus | null): string {
  return `<p class="muted small">${cc?.connected ? "To connect it manually, run this in a terminal:" : "Or run this once in a terminal:"}</p>`;
}

function claudeCodeLine(cc: ClaudeCodeStatus | null): string {
  return !cc
    ? ""
    : cc.connected
      ? '<p class="ok-line">✓ Connected. Claude Code can use the launcher.</p>'
      : !cc.claude_path
        ? `<p class="muted small">Claude Code isn't installed. <button class="link" data-action="open-url" data-url="https://claude.com/claude-code">Get Claude Code</button></p>`
        : !cc.node_path
          ? `<p class="muted small">The launcher's MCP server needs Node.js. <button class="link" data-action="open-url" data-url="https://nodejs.org/">Install Node.js LTS</button>, then reopen Settings.</p>`
          : `<p><button class="btn primary small" data-action="connect-claude" ${inTauri ? "" : "disabled"}>Connect automatically</button></p>`;
}

/** The Claude Code status, or placeholder lines while it's being checked. */
function claudeCodeBlock(cc: ClaudeCodeStatus | null | undefined): string {
  if (cc === undefined) return `<span class="skel-line" style="width:64%"></span><span class="skel-line" style="width:40%"></span>`;
  return claudeCodeLine(cc) + manualLine(cc);
}

/** What Settings shows that doesn't change while the app runs, loaded once. */
let settingsInfo: { path: string; version: string | null; status: any } | null = null;
let settingsInfoLoad: Promise<void> | null = null;

function loadSettingsInfo(): Promise<void> {
  settingsInfoLoad ??= Promise.all([mcpServerPath(), appVersion().catch(() => null), api("GET", "/status")])
    .then(([path, version, status]) => {
      settingsInfo = { path, version, status };
    })
    .catch((e) => {
      settingsInfoLoad = null;
      throw e;
    });
  return settingsInfoLoad;
}

// ---------- storage (a settings category) ----------
// Each part is scanned on its own, so the quick ones show up while the big folders are still counted.

type StorItem = { key: string; label: string; size: number; detail: string; removable: boolean; path?: string };
type StorPart = { total: number; items: StorItem[]; note?: string };

const STOR_PARTS: [string, string, string][] = [
  ["instances", "Instances", "Worlds, mods and settings. Delete an instance from its own page."],
  ["versions", "Minecraft versions", "Game files and mod loaders. Removed ones are downloaded again if an instance needs them."],
  ["assets", "Game assets", "Sounds, languages and textures, shared by every instance."],
  ["libraries", "Libraries", ""],
  ["java", "Java", "Downloaded again when a game or a build needs it."],
  ["gradle", "Mod project builds", "Gradle and what it downloads to build mod projects."],
  ["other", "Other", ""],
];
const stor = new Map<string, StorPart>();
let storBusy = false;

function storageHtml(dataDir: string): string {
  return `
    <h2>Storage</h2>
    <p class="muted small">Everything MioCraft keeps is in <button class="link" data-action="open-path" data-path="${esc(dataDir)}">its data folder</button>. Things nothing uses any more can be removed here.</p>
    <div class="stor-total" id="stor-total"><span class="skel-line" style="width:180px"></span></div>
    ${STOR_PARTS.map(
      ([id, title, desc]) => `
      <section class="settings-sec stor" id="stor-${id}">
        <h3><span>${title}</span><span class="stor-size"></span></h3>
        ${desc ? `<p class="muted small">${desc}</p>` : ""}
        <div class="stor-items"><span class="skel-line" style="width:65%"></span><span class="skel-line" style="width:40%"></span></div>
      </section>`,
    ).join("")}`;
}

function loadStorage(only?: string) {
  for (const [id] of STOR_PARTS) {
    if (only && id !== only) continue;
    api<StorPart>("GET", `/storage/${id}`)
      .then((p) => {
        stor.set(id, p);
        drawStorPart(id);
      })
      .catch((e) => {
        const box = document.querySelector(`#stor-${id} .stor-items`);
        if (box) box.innerHTML = `<p class="muted small">Couldn't look: ${esc((e as Error).message)}</p>`;
      });
  }
}

function drawStorPart(id: string) {
  const sec = document.querySelector<HTMLElement>(`#stor-${id}`);
  const p = stor.get(id);
  if (!sec || !p) return;
  sec.querySelector(".stor-size")!.textContent = fmtSize(p.total);
  const items = id === "instances" || id === "other" ? p.items : [...p.items].sort((a, b) => Number(b.removable) - Number(a.removable) || b.size - a.size);
  const unused = items.filter((i) => i.removable);
  sec.querySelector(".stor-items")!.innerHTML =
    (p.note ? `<p class="note">${esc(p.note)}</p>` : "") +
    (unused.length > 1 && id === "versions"
      ? `<div class="stor-all"><span class="muted small">${unused.length} not used by any instance, ${fmtSize(unused.reduce((n, i) => n + i.size, 0))}</span><button class="btn small" data-action="stor-clean-all" data-part="${id}" ${storBusy ? "disabled" : ""}>Remove all unused</button></div>`
      : "") +
    (items.length
      ? items
          .map(
            (i) => `
        <div class="stor-row${i.removable ? " unused" : ""}">
          <div class="stor-what"><strong>${esc(i.label)}</strong>${i.detail ? `<span class="muted small">${esc(i.detail)}</span>` : ""}</div>
          <span class="stor-n">${fmtSize(i.size)}</span>
          ${i.path ? `<button class="btn ghost small" data-action="open-path" data-path="${esc(i.path)}">Open</button>` : ""}
          ${i.removable ? `<button class="btn ghost small danger" data-action="stor-clean" data-part="${id}" data-key="${esc(i.key)}" ${storBusy ? "disabled" : ""}>Remove</button>` : ""}
        </div>`,
          )
          .join("")
      : '<p class="muted small">Nothing here yet.</p>');
  drawStorTotal();
}

function drawStorTotal() {
  const box = document.querySelector<HTMLElement>("#stor-total");
  if (!box) return;
  const done = STOR_PARTS.filter(([id]) => stor.has(id));
  const total = done.reduce((n, [id]) => n + stor.get(id)!.total, 0);
  const free = done.reduce((n, [id]) => n + stor.get(id)!.items.filter((i) => i.removable).reduce((m, i) => m + i.size, 0), 0);
  const counting = done.length < STOR_PARTS.length;
  box.innerHTML = `<strong>${fmtSize(total)}</strong><span class="muted">${counting ? " so far, still counting…" : " in total"}</span>${free ? `<span class="stor-free">${fmtSize(free)} can be removed</span>` : ""}`;
}

/** Remove items one after another. Returns how many bytes were freed. */
async function storClean(part: string, items: StorItem[]): Promise<number> {
  storBusy = true;
  STOR_PARTS.forEach(([id]) => drawStorPart(id));
  let freed = 0;
  try {
    for (const i of items) {
      const row = document.querySelector<HTMLElement>(`[data-action="stor-clean"][data-part="${part}"][data-key="${CSS.escape(i.key)}"]`);
      if (row) row.textContent = "Removing…";
      const r = await api<{ freed: number }>("POST", "/storage/clean", { part, key: i.key });
      freed += r.freed;
    }
  } catch (e) {
    toast((e as Error).message, "err");
  } finally {
    storBusy = false;
    STOR_PARTS.forEach(([id]) => drawStorPart(id));
    loadStorage(part);
  }
  if (freed) toast(`Freed ${fmtSize(freed)}`);
  return freed;
}

async function askStorClean(part: string, key: string) {
  const i = stor.get(part)?.items.find((x) => x.key === key);
  if (!i || storBusy) return;
  const again: Record<string, string> = {
    versions: "If an instance needs it later, it's downloaded again.",
    assets: "If an instance needs them later, they're downloaded again.",
    java: "If a game or a build needs it later, it's downloaded again.",
    gradle: key === "home" ? "The next mod build downloads what it needs again, which can take several minutes." : "If a mod project needs it later, it's downloaded again.",
    other: "",
  };
  const ok = await ask({
    title: `Remove ${i.label}?`,
    body: `<p>This frees <strong>${fmtSize(i.size)}</strong>. ${again[part] ?? ""}</p>`,
    ok: "Remove",
    danger: true,
  });
  if (ok) await storClean(part, [i]);
}

async function askStorCleanAll(part: string) {
  const items = stor.get(part)?.items.filter((i) => i.removable) ?? [];
  if (!items.length || storBusy) return;
  const size = items.reduce((n, i) => n + i.size, 0);
  const ok = await ask({
    title: `Remove ${items.length} unused versions?`,
    body: `<p>This frees <strong>${fmtSize(size)}</strong>: ${items.map((i) => esc(i.label)).join(", ")}.</p><p class="muted small">If an instance needs one later, it's downloaded again.</p>`,
    ok: "Remove all",
    danger: true,
  });
  if (ok) await storClean(part, items);
}

// ---------- settings page ----------
// A full-window page that slides in over everything, with categories down the side. Every change
// saves straight away.

type SetCat = "claude" | "mods" | "playing" | "discord" | "legacy" | "storage" | "about" | "dev";
const SET_CATS: [SetCat, string][] = [
  ["claude", "✦ Claude Code"],
  ["mods", "Mod browser"],
  ["playing", "While playing"],
  ["discord", "Discord"],
  ["legacy", "Legacy"],
  ["storage", "Storage"],
  ["about", "About"],
  ["dev", "Dev"],
];
let setCat: SetCat = "claude";
let settingsOpen = false;
let savedTimer: number | undefined;

function openSettings(cat?: SetCat) {
  if (cat) setCat = cat;
  refreshClaudeCode();
  const page = $("#settings-page");
  page.innerHTML = `
    <aside class="set-nav">
      <button class="btn small set-back" data-action="close-settings">← Back</button>
      <h2>Settings</h2>
      <nav id="set-cats">
        ${SET_CATS.map(([id, label]) => `<button class="set-cat" data-action="set-cat" data-cat="${id}">${label}</button>`).join("")}
      </nav>
    </aside>
    <div class="set-body">
      <div id="set-pane"></div>
      <span id="set-saved" class="set-saved" aria-live="polite"></span>
    </div>`;
  page.hidden = false;
  void page.offsetWidth;
  page.classList.add("open");
  settingsOpen = true;
  showSetCat(setCat, false);
}

function closeSettings() {
  if (!settingsOpen) return;
  settingsOpen = false;
  const page = $("#settings-page");
  page.classList.remove("open");
  const done = () => {
    if (!settingsOpen) {
      page.hidden = true;
      page.innerHTML = "";
    }
  };
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) done();
  else page.addEventListener("transitionend", done, { once: true });
}

/** Show a category; its data is filled in as soon as it's loaded, with placeholders until then. */
async function showSetCat(cat: SetCat, animate = true) {
  setCat = cat;
  document.querySelectorAll<HTMLElement>(".set-cat").forEach((b) => b.classList.toggle("active", b.dataset.cat === cat));
  slide("setcat", $("#set-cats"), document.querySelector<HTMLElement>(".set-cat.active"));
  const pane = $("#set-pane");
  if (!S.cfg || !settingsInfo) {
    pane.innerHTML = `<h2>${SET_CATS.find(([id]) => id === cat)![1]}</h2><span class="skel-line" style="width:70%"></span><span class="skel-line" style="width:45%"></span><span class="skel-box"></span>`;
    try {
      const [cfg] = await Promise.all([S.cfg ?? api("GET", "/config"), loadSettingsInfo()]);
      S.cfg = cfg;
    } catch (e) {
      toast((e as Error).message, "err");
      return;
    }
    // Closed or moved on while loading.
    if (!settingsOpen || setCat !== cat) return;
  }
  pane.innerHTML = setCatHtml(cat, S.cfg, settingsInfo!);
  if (cat === "storage") {
    stor.clear();
    loadStorage();
  }
  if (animate) slideIn(pane, 0, 14);
}

const setSwitch = (id: string, on: boolean, title: string, desc: string, disabled = false) => `
  <label class="live-opt">
    <span class="switch"><input type="checkbox" id="${id}" ${on ? "checked" : ""} ${disabled ? "disabled" : ""}/><span></span></span>
    <span><strong>${title}</strong><span class="muted small">${desc}</span></span>
  </label>`;

function setCatHtml(cat: SetCat, cfg: any, info: { path: string; version: string | null; status: any }): string {
  const { path, version, status } = info;
  switch (cat) {
    case "claude": {
      const cmd = `claude mcp add --scope user mio-craft -- node "${path}"`;
      return `
        <h2>✦ Claude Code</h2>
        <section class="settings-sec">
          <h3>Connect Claude Code</h3>
          <div id="cc-line">${claudeCodeBlock(lastCc)}</div>
          <div class="copy"><code id="mcp-cmd">${esc(cmd)}</code><button class="btn small" data-copy="${esc(cmd)}">Copy</button></div>
          <p class="muted small">Then ask Claude Code something like "make a Fabric mod for my instance that adds a ruby ore". MioCraft needs to be open while Claude Code uses it.</p>
        </section>`;
    }
    case "mods":
      return `
        <h2>Mod browser</h2>
        <section class="settings-sec">
          <h3>CurseForge</h3>
          <p class="muted small">Modrinth works out of the box. CurseForge needs a free API key from <button class="link" data-action="open-url" data-url="https://console.curseforge.com/">console.curseforge.com</button>.</p>
          <label class="field"><span>CurseForge API key</span><input id="set-cf" type="password" class="input mono" placeholder="Not set" value="${esc(cfg.curseforge_api_key ?? "")}"/></label>
        </section>`;
    case "playing":
      return `
        <h2>While playing</h2>
        <section class="settings-sec">
          ${setSwitch("set-minimize", !!cfg.minimize_while_playing, "Minimize MioCraft while I play", "The launcher gets out of the way when a game starts and comes back when the game closes or crashes.")}
        </section>`;
    case "discord":
      return `
        <h2>Discord</h2>
        <section class="settings-sec">
          ${setSwitch(
            "set-discord",
            cfg.discord_rpc && status.has_discord_app_id,
            "Show what I'm playing",
            status.has_discord_app_id
              ? "Shows the instance name, Minecraft version and mod loader on your Discord profile while a game or test client is running. Discord has to be open."
              : "This build of MioCraft has no Discord app ID, so it can't show a status on Discord.",
            !status.has_discord_app_id,
          )}
        </section>`;
    case "legacy":
      return `
        <h2>Legacy</h2>
        <section class="settings-sec">
          ${setSwitch("set-stylized", cfg.stylized !== false, "Stylized fonts and buttons", "The pixel font and pixel-art buttons and panels. Turn it off to keep the home screen layout with MioCraft's original font and buttons.")}
          ${setSwitch("set-legacy-look", !!cfg.legacy_look, "Legacy look", "Use MioCraft's original layout, with your instances in a sidebar, instead of the home screen with your player.")}
          ${setSwitch("set-legacy", !!cfg.legacy_colors, "Legacy colors", "Use MioCraft's original green and grey instead of the copper colors.")}
        </section>`;
    case "storage":
      return storageHtml(status.data_dir);
    case "about":
      return `
        <h2>About</h2>
        <section class="settings-sec">
          <h3>Updates</h3>
          <p class="muted small">MioCraft ${esc(version ?? status.version)}. It checks GitHub for a new version when it starts and asks before installing.</p>
          <p><button class="btn small" data-action="check-update" ${inTauri ? "" : "disabled"}>Check for updates</button></p>
          ${setSwitch("set-stable", !!cfg.stable_updates, "Stable versions only", "Update only to versions that have been out a while without known problems. Off, you get every new version as soon as it's released. If you're already on a newer version, you keep it until the stable one passes it.")}
        </section>
        <section class="settings-sec">
          <h3>Data folder</h3>
          <button class="path" data-action="open-path" data-path="${esc(status.data_dir)}">${esc(status.data_dir)}</button>
        </section>
        <section class="settings-sec">
          <h3>Setup</h3>
          <p class="muted small">Go through the first-run setup again.</p>
          <p><button class="btn small" data-action="run-setup">Run setup again</button></p>
        </section>`;
    case "dev":
      return `
        <h2>Dev</h2>
        <section class="settings-sec">
          <h3>Microsoft sign-in</h3>
          <p class="muted small">${S.hasBuiltinClientId ? "MioCraft signs you in with its own Azure app. Leave this empty unless you're developing with your own app ID." : "This build of MioCraft has no Microsoft app ID, so sign-in isn't available. Enter an Azure app ID that Mojang has approved here."}</p>
          <label class="field"><span>Azure client ID (optional)</span><input id="set-cid" class="input mono" placeholder="00000000-0000-0000-0000-000000000000" value="${esc(cfg.ms_client_id ?? "")}"/></label>
        </section>`;
  }
}

/** Apply the look settings in `cfg`. */
function applyLookSettings(cfg: any) {
  applyColors(!!cfg.legacy_colors);
  applyStyle(cfg.stylized !== false);
  if (!cfg.legacy_look !== S.xp) applyLook(!cfg.legacy_look);
}

/** Save one change to the config. Look changes show straight away and go back if saving fails. */
async function saveSetting(patch: Record<string, unknown>) {
  const before = S.cfg;
  const look = "legacy_colors" in patch || "stylized" in patch || "legacy_look" in patch;
  if (look) applyLookSettings({ ...before, ...patch });
  try {
    S.cfg = await api("PATCH", "/config", patch);
    if ("ms_client_id" in patch) await loadAccounts();
    if ("curseforge_api_key" in patch) {
      S.browse.instance = null;
      if (S.tab === "browse") runSearch(false);
    }
    const saved = document.querySelector<HTMLElement>("#set-saved");
    if (saved) {
      saved.textContent = "✓ Saved";
      saved.classList.add("show");
      clearTimeout(savedTimer);
      savedTimer = window.setTimeout(() => saved.classList.remove("show"), 1400);
    }
  } catch (e) {
    if (look && before) {
      applyLookSettings(before);
      if (settingsOpen && setCat === "legacy") showSetCat("legacy", false);
    }
    toast((e as Error).message, "err");
  }
}

// Switches save when flipped, text fields when you leave them or press Enter.
document.addEventListener("change", (ev) => {
  const t = ev.target as HTMLInputElement;
  if (!t.closest("#settings-page")) return;
  switch (t.id) {
    case "set-discord":
      return void saveSetting({ discord_rpc: t.checked });
    case "set-minimize":
      return void saveSetting({ minimize_while_playing: t.checked });
    case "set-stable":
      return void saveSetting({ stable_updates: t.checked });
    case "set-stylized":
      return void saveSetting({ stylized: t.checked });
    case "set-legacy-look":
      return void saveSetting({ legacy_look: t.checked });
    case "set-legacy":
      return void saveSetting({ legacy_colors: t.checked });
    case "set-cf":
      return void saveSetting({ curseforge_api_key: t.value });
    case "set-cid":
      return void saveSetting({ ms_client_id: t.value });
  }
});

async function loginModal() {
  if (!S.hasClientId) {
    toast("This build of MioCraft has no Microsoft app ID: add one in Settings to sign in", "err");
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

// ---------- outfit: skin and cape ----------

type Outfit = { skin: string; slim: boolean; capes: { id: string; alias: string; active: boolean; png: string }[] };
type SavedSkin = { id: string; name: string; slim: boolean; png: string };

const png64 = (b64: string) => `data:image/png;base64,${b64}`;

/** The front of a skin, flat, as a data URL (for the skin tiles). */
function skinFront(img: HTMLImageElement, slim: boolean): string {
  const k = img.width / 64;
  const legacy = img.height * 2 === img.width;
  const c = document.createElement("canvas");
  c.width = 16 * 3;
  c.height = 32 * 3;
  const g = c.getContext("2d")!;
  g.imageSmoothingEnabled = false;
  g.scale(3, 3);
  const aw = slim ? 3 : 4;
  const part = (sx: number, sy: number, w: number, h: number, dx: number, dy: number, mirror = false) => {
    g.save();
    if (mirror) {
      g.translate(dx * 2 + w, 0);
      g.scale(-1, 1);
    }
    g.drawImage(img, sx * k, sy * k, w * k, h * k, dx, dy, w, h);
    g.restore();
  };
  // The base layer, then (64×64 skins only) the overlay on top.
  for (const over of legacy ? [false] : [false, true]) {
    part(over ? 40 : 8, 8, 8, 8, 4, 0); // head
    part(20, over ? 36 : 20, 8, 12, 4, 8); // body
    part(44, over ? 36 : 20, aw, 12, 4 - aw, 8); // right arm, on the viewer's left
    if (legacy) part(44, 20, aw, 12, 12, 8, true);
    else part(over ? 52 : 36, 52, aw, 12, 12, 8); // left arm
    part(4, over ? 36 : 20, 4, 12, 4, 20); // right leg
    if (legacy) part(4, 20, 4, 12, 8, 20, true);
    else part(over ? 4 : 20, 52, 4, 12, 8, 20); // left leg
  }
  return c.toDataURL();
}

/** The outside of a cape, flat, as a data URL. */
function capeFront(img: HTMLImageElement): string {
  const k = img.width / 64;
  const c = document.createElement("canvas");
  c.width = 10 * 4;
  c.height = 16 * 4;
  const g = c.getContext("2d")!;
  g.imageSmoothingEnabled = false;
  g.drawImage(img, 1 * k, 1 * k, 10 * k, 16 * k, 0, 0, c.width, c.height);
  return c.toDataURL();
}

/** Guess the thin-armed model: the outer columns of the right arm are empty. */
function looksSlim(img: HTMLImageElement): boolean {
  if (img.height !== img.width) return false;
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const g = c.getContext("2d", { willReadFrequently: true })!;
  g.drawImage(img, 0, 0, 64, 64);
  const d = g.getImageData(54, 20, 2, 12).data;
  for (let i = 3; i < d.length; i += 4) if (d[i] !== 0) return false;
  return true;
}

/** Change the account's skin and cape, with a 3D preview and a library of saved skins. */
function outfitModal(acc: Account) {
  type Pick = { png: string; slim: boolean; key: string };
  let orig: { png: string; slim: boolean; cape: string | null } | null = null;
  let pick: Pick | null = null;
  let cape: string | null = null;
  let capes: Outfit["capes"] = [];
  let library: SavedSkin[] = [];
  let upload: Pick | null = null;
  let busy = false;
  const thumbs = new Map<string, string>();

  openModal(
    `
    <h2>Change outfit <span class="muted small">${esc(acc.name)}</span></h2>
    <div class="outfit">
      <div class="of-left">
        <div class="of-view"><canvas id="of-canvas" class="loading"></canvas><span class="of-hint muted small">Drag to turn</span></div>
        <div class="seg of-model" id="of-model"><button data-v="classic">Classic arms</button><button data-v="slim">Slim arms</button></div>
        <button class="btn small ghost" id="of-keep" hidden>Save to library</button>
      </div>
      <div class="of-right">
        <h3>Skin</h3>
        <div class="of-grid" id="of-skins">${'<span class="of-tile skel"></span>'.repeat(4)}</div>
        <input type="file" id="of-file" accept="image/png" hidden/>
        <h3>Cape</h3>
        <div class="of-grid capes" id="of-capes">${'<span class="of-tile cape skel"></span>'.repeat(3)}</div>
        <p class="muted small">When you change skin, the old one stays in this list. In game, the new outfit shows the next time you start Minecraft.</p>
      </div>
    </div>
    <div class="modal-actions">
      <button class="btn ghost small" id="of-reset" title="Wear one of Minecraft's default skins" disabled>Use a default skin</button>
      <span class="grow"></span>
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="of-save" disabled>Save outfit</button>
    </div>`,
    (m) => m.classList.add("outfit-modal"),
  );

  // ---- 3D preview ----
  const canvas = $<HTMLCanvasElement>("#of-canvas");
  const box = canvas.parentElement!;
  const viewer = new SkinViewer({ canvas, width: box.clientWidth || 220, height: box.clientHeight || 300, fov: 40, zoom: 0.85 });
  viewer.controls.enableZoom = false;
  viewer.controls.enablePan = false;
  viewer.animation = new IdleAnimation();
  // Turn towards the front for skins and the back for capes.
  let turnTo = 0.5;
  let raf = 0;
  const turn = () => {
    const r = viewer.playerObject.rotation;
    r.y += (turnTo - r.y) * 0.12;
    raf = Math.abs(turnTo - r.y) > 0.002 ? requestAnimationFrame(turn) : 0;
  };
  const face = (y: number) => {
    turnTo = y;
    if (!raf) raf = requestAnimationFrame(turn);
  };
  viewer.playerObject.rotation.y = 0.5;
  modalCleanup = () => {
    cancelAnimationFrame(raf);
    viewer.dispose();
  };

  const capePng = (id: string | null) => capes.find((c) => c.id === id)?.png ?? "";

  const show = () => {
    if (!pick) return;
    viewer.loadSkin(pick.png, { model: pick.slim ? "slim" : "default" });
    const cp = capePng(cape);
    if (cp) viewer.loadCape(png64(cp));
    else viewer.loadCape(null);
    canvas.classList.remove("loading");
  };

  const changed = () => !!orig && !!pick && (pick.png !== orig.png || pick.slim !== orig.slim || cape !== orig.cape);

  const render = () => {
    if (!pick || !orig) return;
    const p = pick;
    document.querySelectorAll<HTMLButtonElement>("#of-model button").forEach((b) => b.classList.toggle("on", (b.dataset.v === "slim") === p.slim));
    const tile = (key: string, slim: boolean, name: string, extra = "") => {
      const t = thumbs.get(`${key}:${slim}`) ?? "";
      return `<button class="of-tile ${p.key === key ? "on" : ""}" data-skin="${esc(key)}" title="${esc(name)}">
        ${t ? `<img src="${t}" alt=""/>` : '<span class="of-blank"></span>'}
        <span class="of-name">${esc(name)}</span>${extra}
      </button>`;
    };
    // The skin being worn is already the first tile.
    const wearing = orig.png.slice("data:image/png;base64,".length);
    const lib = library
      .filter((l) => l.png !== wearing)
      .map((l) => tile(l.id, l.slim, l.name, `<span class="of-del" data-del="${esc(l.id)}" title="Delete from the library" role="button">×</span>`));
    $("#of-skins").innerHTML =
      tile("current", orig.slim, "Wearing now") +
      (upload ? tile("upload", upload.slim, "New skin") : "") +
      lib.join("") +
      `<button class="of-tile add" id="of-upload" title="Choose a 64×64 PNG skin"><span class="of-plus">+</span><span class="of-name">Upload…</span></button>`;
    $("#of-capes").innerHTML =
      `<button class="of-tile cape ${cape === null ? "on" : ""}" data-cape=""><span class="of-blank none"></span><span class="of-name">No cape</span></button>` +
      capes
        .map((c) => {
          const t = thumbs.get(`cape:${c.id}`);
          return `<button class="of-tile cape ${cape === c.id ? "on" : ""}" data-cape="${esc(c.id)}" title="${esc(c.alias)}">${t ? `<img src="${t}" alt=""/>` : '<span class="of-blank"></span>'}<span class="of-name">${esc(c.alias)}</span></button>`;
        })
        .join("") +
      (capes.length ? "" : '<span class="muted small of-nocapes">This account has no capes.</span>');
    // Offer to keep a new or worn skin that isn't in the library yet.
    const saved = p.key !== "upload" && p.key !== "current" ? true : library.some((l) => png64(l.png) === p.png);
    $<HTMLButtonElement>("#of-keep").hidden = saved;
    $<HTMLButtonElement>("#of-save").disabled = busy || !changed();
    $<HTMLButtonElement>("#of-reset").disabled = busy;
  };

  /** Flat pictures for the tiles, made once per skin and model. */
  const makeThumbs = async () => {
    const want: [string, string, boolean][] = [];
    if (orig) want.push(["current", orig.png, orig.slim]);
    if (upload) want.push(["upload", upload.png, upload.slim]);
    library.forEach((l) => want.push([l.id, png64(l.png), l.slim]));
    if (pick) want.push([pick.key, pick.png, pick.slim]);
    for (const [key, src, slim] of want) {
      if (thumbs.has(`${key}:${slim}`)) continue;
      try {
        thumbs.set(`${key}:${slim}`, skinFront(await loadImage(src), slim));
      } catch {}
    }
    for (const c of capes) {
      if (thumbs.has(`cape:${c.id}`) || !c.png) continue;
      try {
        thumbs.set(`cape:${c.id}`, capeFront(await loadImage(png64(c.png))));
      } catch {}
    }
    if (canvas.isConnected) render();
  };

  const apply = (o: Outfit) => {
    capes = o.capes;
    const active = o.capes.find((c) => c.active)?.id ?? null;
    orig = { png: png64(o.skin), slim: o.slim, cape: active };
    pick = { png: orig.png, slim: orig.slim, key: "current" };
    cape = active;
    upload = null;
    thumbs.delete("current:true");
    thumbs.delete("current:false");
  };

  const loadLibrary = () =>
    api<SavedSkin[]>("GET", "/skins")
      .then((l) => (library = l))
      .catch(() => (library = []));

  Promise.all([api<Outfit>("GET", `/accounts/${enc(acc.id)}/outfit`), loadLibrary()])
    .then(([o]) => {
      if (!canvas.isConnected) return;
      apply(o);
      show();
      render();
      void makeThumbs();
    })
    .catch((e) => {
      if (!canvas.isConnected) return;
      $("#of-skins").innerHTML = `<p class="err-line small">${esc((e as Error).message)}</p>`;
      $("#of-capes").innerHTML = "";
    });

  const pickSkin = (p: Pick) => {
    pick = p;
    face(0.5);
    show();
    render();
    void makeThumbs();
  };

  const m = $(".outfit-modal");
  m.addEventListener("click", async (ev) => {
    const t = ev.target as HTMLElement;
    const del = t.closest<HTMLElement>("[data-del]");
    if (del) {
      ev.stopPropagation();
      // The first click asks, the second deletes.
      if (del.dataset.armed !== "1") {
        del.dataset.armed = "1";
        del.textContent = "Delete?";
        del.classList.add("armed");
        return;
      }
      const id = del.dataset.del!;
      try {
        await api("DELETE", `/skins/${enc(id)}`);
        library = library.filter((l) => l.id !== id);
        if (pick?.key === id && orig) pick = { png: orig.png, slim: orig.slim, key: "current" };
        show();
        render();
      } catch (e) {
        toast((e as Error).message, "err");
      }
      return;
    }
    const skin = t.closest<HTMLElement>("[data-skin]");
    if (skin && orig) {
      const k = skin.dataset.skin!;
      if (k === "current") pickSkin({ png: orig.png, slim: orig.slim, key: k });
      else if (k === "upload" && upload) pickSkin(upload);
      else {
        const l = library.find((x) => x.id === k);
        if (l) pickSkin({ png: png64(l.png), slim: l.slim, key: l.id });
      }
      return;
    }
    const cp = t.closest<HTMLElement>("[data-cape]");
    if (cp && pick) {
      cape = cp.dataset.cape || null;
      face(cape ? Math.PI + 0.5 : 0.5);
      show();
      render();
      return;
    }
    if (t.closest("#of-upload")) $<HTMLInputElement>("#of-file").click();
  });

  $("#of-model").addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!b || !pick) return;
    pick = { ...pick, slim: b.dataset.v === "slim" };
    if (pick.key === "upload" && upload) upload = { ...upload, slim: pick.slim };
    face(0.5);
    show();
    render();
    void makeThumbs();
  });

  $<HTMLInputElement>("#of-file").addEventListener("change", async (ev) => {
    const input = ev.target as HTMLInputElement;
    const f = input.files?.[0];
    input.value = "";
    if (!f) return;
    const url = await new Promise<string>((ok, fail) => {
      const r = new FileReader();
      r.onload = () => ok(r.result as string);
      r.onerror = () => fail(r.error);
      r.readAsDataURL(f);
    });
    if (!url.startsWith("data:image/png")) return toast("A skin has to be a PNG image", "err");
    let img: HTMLImageElement;
    try {
      img = await loadImage(url);
    } catch {
      return toast("That file isn't a PNG image", "err");
    }
    if (img.width !== 64 || (img.height !== 64 && img.height !== 32)) {
      return toast(`A skin has to be 64×64 pixels (or the old 64×32), but this image is ${img.width}×${img.height}`, "err");
    }
    upload = { png: url, slim: looksSlim(img), key: "upload" };
    thumbs.delete("upload:true");
    thumbs.delete("upload:false");
    pickSkin(upload);
  });

  const bytes = async (dataUrl: string) => (await fetch(dataUrl)).arrayBuffer();

  $("#of-keep").addEventListener("click", async () => {
    if (!pick) return;
    const name = pick.key === "upload" ? "New skin" : "Saved skin";
    try {
      const saved = await apiBytes<SavedSkin>(`/skins?model=${pick.slim ? "slim" : "classic"}&name=${enc(name)}`, await bytes(pick.png));
      await loadLibrary();
      if (pick.key === "upload") {
        pick = { ...pick, key: saved.id };
        upload = null;
      }
      toast("Saved to your skin library");
      render();
      void makeThumbs();
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });

  /** After a change: show it on the home screen and in the top bar. */
  const finish = async (o: Outfit, msg: string) => {
    apply(o);
    await loadLibrary();
    const worn = o.capes.find((c) => c.active && c.png);
    await wearSkin(acc.id, png64(o.skin), o.slim, worn ? png64(worn.png) : null);
    renderTopbar();
    renderSidebar();
    if (msg) toast(msg);
  };

  $("#of-save").addEventListener("click", async () => {
    if (!pick || !orig || busy) return;
    const save = $<HTMLButtonElement>("#of-save");
    busy = true;
    save.textContent = "Saving…";
    render();
    const wantCape = cape;
    const capeChanges = wantCape !== orig.cape;
    try {
      if (pick.png !== orig.png || pick.slim !== orig.slim) {
        // Show the new skin straight away, so a failed cape change below doesn't hide it.
        await finish(await apiBytes<Outfit>(`/accounts/${enc(acc.id)}/skin?model=${pick.slim ? "slim" : "classic"}`, await bytes(pick.png)), capeChanges ? "" : "Outfit changed");
        cape = wantCape;
      }
      if (capeChanges) await finish(await api<Outfit>("PUT", `/accounts/${enc(acc.id)}/cape`, { cape_id: wantCape }), "Outfit changed");
      closeModal();
    } catch (e) {
      busy = false;
      save.textContent = "Save outfit";
      show();
      render();
      void makeThumbs();
      toast((e as Error).message, "err");
    }
  });

  $("#of-reset").addEventListener("click", async () => {
    if (busy) return;
    busy = true;
    render();
    try {
      const o = await api<Outfit>("DELETE", `/accounts/${enc(acc.id)}/skin`);
      await finish(o, "Now wearing a default skin. Your old one is in the list.");
      busy = false;
      show();
      render();
      void makeThumbs();
    } catch (e) {
      busy = false;
      render();
      toast((e as Error).message, "err");
    }
  });
}

function accountsModal() {
  openModal(`
    <h2>Accounts</h2>
    <div class="accounts">
      ${S.accounts
        .map(
          (a) => `<div class="acc-row">
            ${headImg(a, 28)}
            <span class="grow">${esc(a.name)}${a.selected ? ' <span class="badge ok">In use</span>' : ""}</span>
            <button class="btn small ghost" data-action="outfit" data-id="${esc(a.id)}" title="Change skin and cape">Outfit</button>
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
  const LO: LaunchOptions = inst.launch ?? {};
  const toggle = (key: keyof LiveSettings, title: string, desc: string, extra = "") => `
    <label class="live-opt ${extra}">
      <span class="switch"><input type="checkbox" id="lv-${key}" ${L[key] ? "checked" : ""} ${off ? "disabled" : ""}/><span></span></span>
      <span><strong>${title}</strong><span class="muted small">${desc}</span></span>
    </label>`;
  openModal(`
    <h2>Edit instance</h2>
    <label class="field"><span>Name</span><input id="ie-name" class="input" value="${esc(inst.name)}"/></label>
    <div class="field"><span>Icon</span>
      <div class="icon-edit">
        <span id="ie-icon-preview"></span>
        <div class="icon-opts">
          <div class="swatches" role="radiogroup" aria-label="Icon colour">
            <button type="button" class="swatch default ${esc(inst.loader)}" data-color="" title="Loader colour" aria-label="Loader colour"></button>
            ${ICON_COLORS.map((c) => `<button type="button" class="swatch" data-color="${c}" style="background:${c}" title="${c}" aria-label="Colour ${c}"></button>`).join("")}
          </div>
          <div class="glyph-picks" role="radiogroup" aria-label="Icon symbol">
            <button type="button" class="glyph-pick" data-glyph="" title="Letter">${esc(inst.loader[0].toUpperCase())}</button>
            ${GLYPHS.map((g) => `<button type="button" class="glyph-pick" data-glyph="${g}" title="${g}" aria-label="${g}">${glyphSvg(g, 16)}</button>`).join("")}
          </div>
          <div class="pic-row">
            <button type="button" class="btn small ghost" id="ie-pic">Use a picture…</button>
            <button type="button" class="btn small ghost" id="ie-pic-clear">Remove picture</button>
            <input type="file" id="ie-pic-file" accept="image/png,image/jpeg,image/gif,image/webp" hidden/>
          </div>
        </div>
      </div>
    </div>
    <label class="field"><span>Memory (MB)</span><input id="ie-ram" type="number" min="1024" step="512" class="input" value="${esc(inst.ram_mb ?? S.cfg?.default_ram_mb ?? 4096)}"/></label>
    <label class="field"><span>Java arguments</span><input id="ie-jvm" class="input mono" placeholder="-XX:+UseG1GC" value="${esc((inst.jvm_args ?? S.cfg?.extra_jvm_args ?? []).join(" "))}"/></label>
    <div class="field"><span>Window</span>
      <div class="win-row">
        <select id="ie-win" class="input">
          <option value="default" ${!LO.fullscreen && !LO.width ? "selected" : ""}>Game default</option>
          <option value="size" ${!LO.fullscreen && LO.width ? "selected" : ""}>Size</option>
          <option value="full" ${LO.fullscreen ? "selected" : ""}>Fullscreen</option>
        </select>
        <span class="win-size" ${LO.fullscreen || !LO.width ? "hidden" : ""}>
          <input id="ie-w" type="number" min="320" max="16384" class="input" aria-label="Width" value="${esc(LO.width ?? 1280)}"/>
          <span class="muted">×</span>
          <input id="ie-h" type="number" min="240" max="16384" class="input" aria-label="Height" value="${esc(LO.height ?? 720)}"/>
        </span>
      </div>
    </div>
    <details class="adv-launch" ${LO.before_launch || LO.env?.length ? "open" : ""}>
      <summary>Advanced launch options</summary>
      <label class="field"><span>Command before launch</span><input id="ie-pre" class="input mono" placeholder="e.g. a backup script" value="${esc(LO.before_launch ?? "")}"/></label>
      <p class="muted small">Runs in the game folder each time you press Play; the game starts only if it succeeds. Its output shows in the Console. It can use INST_ID, INST_NAME, INST_DIR, INST_MC_DIR and INST_JAVA.</p>
      <label class="field"><span>Environment variables</span><textarea id="ie-env" class="input mono" rows="3" spellcheck="false" placeholder="NAME=value, one per line">${esc((LO.env ?? []).map((e) => `${e.name}=${e.value}`).join("\n"))}</textarea></label>
      <p class="muted small">Set for the game and the command. These stay on this computer: modpack exports leave them out, and imports never set them.</p>
    </details>
    <div class="field"><span>Game settings</span>
      <div class="row-btn"><button type="button" class="btn small ghost" id="ie-copyset">Copy settings from…</button><span class="muted small">Keybinds, FOV, sound, accessibility and more, from another instance or Minecraft.</span></div>
    </div>
    <section class="settings-sec live-sec">
      <h3>⚡ Live mode <span class="muted small">applies the next time you press Play</span></h3>
      ${off ? `<p class="muted small">${esc(status.unsupported_reason)}</p>` : '<p class="muted small">Experimental: change the game while it runs. Singleplayer only; back up your world first.</p>'}
      ${toggle("hotswap", "Hotswap code", "Push Claude's code changes into the running game. Uses JetBrains Runtime, downloaded once.")}
      ${toggle("content_slots", "Content slots", "Add new blocks and items while playing, using pre-registered slots.")}
      ${toggle("in_memory_textures", "Instant textures", "Paint new textures straight into the game. Off: an automatic resource reload (a few seconds) instead.")}
      ${toggle("unfreeze_registries", "Unfreeze registries", "Experimental: register real blocks and items while playing. Can glitch or crash; back up your world first.", "danger-opt")}
      <label class="field slots ${off ? "dim" : ""}"><span>Slots (blocks and items each)</span><input id="lv-slot_count" type="number" min="1" max="256" class="input" value="${esc(L.slot_count)}" ${off ? "disabled" : ""}/></label>
    </section>
    <div class="modal-actions spread">
      <span><button class="btn danger ghost" id="ie-del">Delete instance</button> <button class="btn ghost" id="ie-dup" title="Copy this instance with its worlds, mods and settings">Duplicate</button></span>
      <span><button class="btn ghost" data-action="close-modal">Cancel</button> <button class="btn primary" id="ie-save">Save</button></span>
    </div>`);
  // Icon draft, saved with the rest.
  const icon = { loader: inst.loader, icon_color: inst.icon_color ?? "", icon_glyph: inst.icon_glyph ?? "", icon_image: inst.icon_image ?? "" };
  const drawIcon = () => {
    $("#ie-icon-preview").innerHTML = instIcon(icon, true);
    document.querySelectorAll<HTMLElement>(".swatch").forEach((b) => b.setAttribute("aria-checked", String(b.dataset.color === icon.icon_color)));
    document.querySelectorAll<HTMLElement>(".glyph-pick").forEach((b) => b.setAttribute("aria-checked", String(b.dataset.glyph === icon.icon_glyph)));
    $("#ie-pic-clear").hidden = !icon.icon_image;
    document.querySelector(".icon-edit")!.classList.toggle("has-pic", !!icon.icon_image);
  };
  drawIcon();
  document.querySelectorAll<HTMLElement>(".swatch").forEach((b) =>
    b.addEventListener("click", () => {
      icon.icon_color = b.dataset.color!;
      drawIcon();
    }),
  );
  document.querySelectorAll<HTMLElement>(".glyph-pick").forEach((b) =>
    b.addEventListener("click", () => {
      icon.icon_glyph = b.dataset.glyph!;
      drawIcon();
    }),
  );
  $("#ie-pic").addEventListener("click", () => $<HTMLInputElement>("#ie-pic-file").click());
  $("#ie-pic-clear").addEventListener("click", () => {
    icon.icon_image = "";
    drawIcon();
  });
  $<HTMLInputElement>("#ie-pic-file").addEventListener("change", async (ev) => {
    const input = ev.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      icon.icon_image = await pictureToIcon(file);
      drawIcon();
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
  $<HTMLSelectElement>("#ie-win").addEventListener("change", (e) => {
    $(".win-size").hidden = (e.target as HTMLSelectElement).value !== "size";
  });
  if (S.busy.has(`dup:${inst.id}`)) ($("#ie-dup") as HTMLButtonElement).disabled = true;
  $("#ie-copyset").addEventListener("click", () => copySettingsModal(inst));
  $("#ie-dup").addEventListener("click", () => {
    closeModal();
    duplicateInstance(inst);
  });
  $("#ie-save").addEventListener("click", async () => {
    try {
      const ram = $<HTMLInputElement>("#ie-ram").value;
      const chk = (k: string) => $<HTMLInputElement>(`#lv-${k}`).checked;
      const win = $<HTMLSelectElement>("#ie-win").value;
      const num = (sel: string) => Math.round(Number($<HTMLInputElement>(sel).value)) || undefined;
      const launch: LaunchOptions = {
        fullscreen: win === "full",
        width: win === "size" ? num("#ie-w") : undefined,
        height: win === "size" ? num("#ie-h") : undefined,
        before_launch: $<HTMLInputElement>("#ie-pre").value,
        env: parseEnv($<HTMLTextAreaElement>("#ie-env").value),
      };
      if (win === "size" && (!launch.width || !launch.height)) throw new Error("Give the window a width and a height");
      const live: LiveSettings | undefined = off
        ? undefined
        : {
            hotswap: chk("hotswap"),
            content_slots: chk("content_slots"),
            in_memory_textures: chk("in_memory_textures"),
            unfreeze_registries: chk("unfreeze_registries"),
            slot_count: Number($<HTMLInputElement>("#lv-slot_count").value) || 64,
          };
      if (
        live?.unfreeze_registries &&
        !L.unfreeze_registries &&
        !(await ask({
          title: "Unfreeze registries?",
          body: "This is experimental and can crash the game or corrupt a world. Back up worlds you care about first.",
          ok: "Turn it on",
          danger: true,
        }))
      )
        return;
      await api("PATCH", `/instances/${enc(inst.id)}`, {
        name: $<HTMLInputElement>("#ie-name").value,
        ram_mb: ram ? Number(ram) : undefined,
        jvm_args: $<HTMLInputElement>("#ie-jvm").value.split(/\s+/).filter(Boolean),
        live,
        launch,
        icon_color: icon.icon_color,
        icon_glyph: icon.icon_glyph,
        icon_image: icon.icon_image,
      });
      if (S.running.has(`game:${inst.id}`) && live && JSON.stringify(live) !== JSON.stringify(L)) toast("Live mode changes apply the next time you launch");
      closeModal();
    } catch (e) {
      toast((e as Error).message, "err");
    }
  });
  $("#ie-del").addEventListener("click", () => deleteInstance(inst));
}

// ---------- copying game settings (options.txt) ----------

const OPTION_CATS: [string, string, string][] = [
  ["controls", "Controls and keybinds", "Mods' keybinds too, and mouse settings"],
  ["video", "Video", "FOV, render distance, brightness, GUI scale…"],
  ["sound", "Sound", "Volume sliders and subtitles"],
  ["accessibility", "Accessibility", "Narrator, text background, screen effects…"],
  ["chat", "Chat", "Size, opacity and visibility"],
  ["skin", "Skin parts", "Which skin layers show, and your main hand"],
  ["language", "Language", ""],
];

/** Copy settings from another instance, the Minecraft Launcher or a file into `target` (and others). */
function copySettingsModal(target: Instance) {
  let fileText: string | null = null;
  openModal(`
    <h2>Copy game settings</h2>
    <label class="field"><span>From</span>
      <select id="cs-from" class="input" disabled><option>Looking for settings…</option></select>
    </label>
    <input type="file" id="cs-file" accept=".txt,text/plain" hidden/>
    <div class="field"><span>Settings to copy</span>
      <div class="cs-grid">
        ${OPTION_CATS.map(([k, label, hint]) => `<label class="check"><input type="checkbox" class="cs-cat" value="${k}" checked/> <span>${label}${hint ? ` <span class="muted small">· ${hint}</span>` : ""}</span></label>`).join("")}
      </div>
    </div>
    <div class="field"><span>Copy into</span>
      <div class="cs-to">
        ${S.instances
          .map((i) => {
            const running = S.running.has(`game:${i.id}`);
            return `<label class="check"><input type="checkbox" class="cs-inst" value="${esc(i.id)}" ${i.id === target.id ? "checked" : ""} ${running ? "disabled" : ""}/> ${instIcon(i)} <span>${esc(i.name)} <span class="muted small">${esc(i.mc_version)}${running ? " · running" : ""}</span></span></label>`;
          })
          .join("")}
      </div>
    </div>
    <p class="muted small">Each instance keeps its other settings, and its old ones are saved as <code>options.txt.bak</code>. A few settings don't carry over between very different Minecraft versions, and mods like Sodium, Iris and OptiFine keep their video settings in their own files.</p>
    <div class="modal-actions">
      <button class="btn ghost" data-action="close-modal">Cancel</button>
      <button class="btn primary" id="cs-go" disabled>Copy settings</button>
    </div>`);
  const from = $<HTMLSelectElement>("#cs-from");
  const syncTargets = () => {
    // An instance can't copy into itself.
    document.querySelectorAll<HTMLInputElement>(".cs-inst").forEach((c) => {
      const self = c.value === from.value;
      const running = S.running.has(`game:${c.value}`);
      c.disabled = self || running;
      if (self) c.checked = false;
    });
  };
  api<{ id: string; name: string; mc_version: string | null }[]>("GET", "/game-options/sources")
    .then((list) => {
      if (!from.isConnected) return;
      const others = list.filter((src) => src.id !== target.id);
      from.innerHTML =
        others.map((src) => `<option value="${esc(src.id)}">${esc(src.name)}${src.mc_version ? ` (${esc(src.mc_version)})` : ""}</option>`).join("") +
        '<option value="__file">An options.txt file…</option>';
      from.disabled = false;
      $<HTMLButtonElement>("#cs-go").disabled = false;
      syncTargets();
    })
    .catch((e) => toast((e as Error).message, "err"));
  from.addEventListener("change", () => {
    syncTargets();
    if (from.value === "__file") $<HTMLInputElement>("#cs-file").click();
  });
  $<HTMLInputElement>("#cs-file").addEventListener("change", async (ev) => {
    const f = (ev.target as HTMLInputElement).files?.[0];
    if (!f) return;
    fileText = await f.text();
    from.querySelector<HTMLOptionElement>('option[value="__file"]')!.textContent = f.name;
  });
  $("#cs-go").addEventListener("click", async () => {
    const categories = [...document.querySelectorAll<HTMLInputElement>(".cs-cat:checked")].map((c) => c.value);
    const to = [...document.querySelectorAll<HTMLInputElement>(".cs-inst:checked")].map((c) => c.value);
    if (!categories.length) return toast("Pick at least one kind of setting", "err");
    if (!to.length) return toast("Pick an instance to copy into", "err");
    const body = from.value === "__file" ? { text: fileText, to, categories } : { from: from.value, to, categories };
    if (from.value === "__file" && fileText == null) return toast("Choose the options.txt file first", "err");
    const go = $<HTMLButtonElement>("#cs-go");
    go.disabled = true;
    go.textContent = "Copying…";
    try {
      const res = await api<{ instance_id: string; copied: number; error: string | null }[]>("POST", "/game-options/copy", body);
      closeModal();
      const name = (id: string) => S.instances.find((i) => i.id === id)?.name ?? id;
      const ok = res.filter((r) => !r.error);
      if (ok.length) toast(ok.length === 1 ? `Copied ${ok[0].copied} settings into ${name(ok[0].instance_id)}` : `Copied settings into ${ok.length} instances`);
      res.filter((r) => r.error).forEach((r) => toast(`${name(r.instance_id)}: ${r.error}`, "err"));
    } catch (e) {
      go.disabled = false;
      go.textContent = "Copy settings";
      toast((e as Error).message, "err");
    }
  });
}

async function deleteInstance(inst: Instance) {
  const sure = await ask({
    title: `Delete "${inst.name}"?`,
    body: "Its worlds, mods and settings are deleted too. This can't be undone.",
    ok: "Delete instance",
    danger: true,
  });
  if (!sure) return;
  try {
    await api("DELETE", `/instances/${enc(inst.id)}`);
    closeModal();
    toast("Instance deleted");
  } catch (e) {
    toast((e as Error).message, "err");
  }
}

async function duplicateInstance(inst: Instance) {
  const key = `dup:${inst.id}`;
  if (S.busy.has(key)) return;
  S.busy.add(key);
  toast(`Copying ${inst.name}… (worlds can take a while)`);
  try {
    const copy = await api<Instance>("POST", `/instances/${enc(inst.id)}/duplicate`, {});
    await loadInstances();
    await selectInstance(copy.id);
    toast(`Made ${copy.name}`);
  } catch (e) {
    toast((e as Error).message, "err");
  } finally {
    S.busy.delete(key);
  }
}

/** Start the game; with `server`, it joins that server straight away. */
function playInstance(inst: Instance, server?: string) {
  if (!S.accounts.length) {
    toast("Sign in with a Microsoft account that owns Minecraft first", "err");
    loginModal();
    return;
  }
  busy(`launch:${inst.id}`, async () => {
    await api("POST", `/instances/${enc(inst.id)}/launch`, server ? { server } : {});
  });
}


// ---------- desktop shortcuts ----------

/** The instance's tile drawn at one size, as PNG bytes. */
async function tilePng(inst: Instance, size: number): Promise<Uint8Array> {
  const c = document.createElement("canvas");
  c.width = c.height = size;
  const g = c.getContext("2d")!;
  // Draw it the way the app shows it: read the tile's colours from a hidden copy.
  const probe = document.createElement("div");
  probe.style.cssText = "position:fixed;left:-9999px;top:0";
  probe.innerHTML = instIcon(inst, true);
  document.body.appendChild(probe);
  const tile = probe.firstElementChild as HTMLElement;
  const cs = getComputedStyle(tile);
  const [bg, ink, font] = [cs.backgroundColor, cs.color, cs.fontFamily];
  probe.remove();
  const r = Math.round(size * 0.16);
  g.beginPath();
  g.roundRect(0, 0, size, size, r);
  g.clip();
  const load = (src: string) =>
    new Promise<HTMLImageElement>((ok, fail) => {
      const im = new Image();
      im.onload = () => ok(im);
      im.onerror = fail;
      im.src = src;
    });
  if (inst.icon_image) {
    g.imageSmoothingEnabled = false;
    g.drawImage(await load(inst.icon_image), 0, 0, size, size);
  } else {
    g.fillStyle = bg;
    g.fillRect(0, 0, size, size);
    if (inst.icon_glyph && GLYPHS.includes(inst.icon_glyph)) {
      // As a picture of its own, the SVG needs its namespace and a real colour.
      const svg = glyphSvg(inst.icon_glyph, size).replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" ').replace('fill="currentColor"', `fill="${ink}"`);
      g.imageSmoothingEnabled = false;
      const inset = Math.round(size * 0.18);
      g.drawImage(await load(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`), inset, inset, size - 2 * inset, size - 2 * inset);
    } else {
      g.fillStyle = ink;
      g.font = `700 ${Math.round(size * 0.55)}px ${font}`;
      g.textAlign = "center";
      g.textBaseline = "middle";
      g.fillText(inst.loader[0].toUpperCase(), size / 2, size / 2 + size * 0.03);
    }
  }
  const blob = await new Promise<Blob>((ok) => c.toBlob((b) => ok(b!), "image/png"));
  return new Uint8Array(await blob.arrayBuffer());
}

/** An .ico file holding PNG images (Windows reads PNG inside .ico since Vista). */
async function shortcutIcon(inst: Instance): Promise<Uint8Array> {
  const sizes = [256, 48, 32, 16];
  const pngs = await Promise.all(sizes.map((s) => tilePng(inst, s)));
  const head = 6 + 16 * sizes.length;
  const out = new Uint8Array(head + pngs.reduce((n, p) => n + p.length, 0));
  const v = new DataView(out.buffer);
  v.setUint16(2, 1, true);
  v.setUint16(4, sizes.length, true);
  let at = head;
  sizes.forEach((s, i) => {
    const e = 6 + 16 * i;
    out[e] = out[e + 1] = s >= 256 ? 0 : s;
    v.setUint16(e + 4, 1, true);
    v.setUint16(e + 6, 32, true);
    v.setUint32(e + 8, pngs[i].length, true);
    v.setUint32(e + 12, at, true);
    out.set(pngs[i], at);
    at += pngs[i].length;
  });
  return out;
}

async function makeShortcut(inst: Instance) {
  try {
    const icon = await shortcutIcon(inst);
    let r = await createShortcut(inst.id, inst.name, icon, false);
    if (r.status === "exists") {
      const ok = await ask({
        title: "Replace the shortcut?",
        body: `<p>There's already a shortcut called <strong>${esc(r.path.replace(/^.*[\\/]/, "").replace(/\.lnk$/i, ""))}</strong> on your desktop.</p>`,
        ok: "Replace",
      });
      if (!ok) return;
      r = await createShortcut(inst.id, inst.name, icon, true);
    }
    toast(`Added a shortcut to ${inst.name} on your desktop`);
  } catch (e) {
    toast(String((e as Error).message ?? e), "err");
  }
}

/** A desktop shortcut asked to play an instance: show it and play it the usual way. */
function playFromShortcut(id: string) {
  const inst = S.instances.find((i) => i.id === id);
  if (!inst) {
    toast("The instance that shortcut plays no longer exists", "err");
    return;
  }
  if (S.view !== "instances") setView("instances");
  selectInstance(inst.id);
  if (S.running.has(`game:${inst.id}`)) {
    toast(`${inst.name} is already running`);
    return;
  }
  if (S.busy.has(`launch:${inst.id}`)) return;
  playInstance(inst);
}

// ---------- right-click menu on instances ----------

document.addEventListener("contextmenu", (ev) => {
  const card = (ev.target as HTMLElement).closest<HTMLElement>(".inst[data-id], .icard[data-id]");
  const inst = card && S.instances.find((i) => i.id === card.dataset.id);
  if (!inst) return;
  ev.preventDefault();
  const running = S.running.has(`game:${inst.id}`);
  const items: MenuItem[] = [
    running
      ? { label: "Stop", run: () => api("POST", `/instances/${enc(inst.id)}/stop`).catch((e) => toast(e.message, "err")) }
      : { label: inst.installed ? "Play" : "Install & play", run: () => playInstance(inst), disabled: S.busy.has(`launch:${inst.id}`) },
    { label: "Open folder", run: () => openPath(inst.game_dir).then(() => !inTauri && toast("Path copied")) },
    "sep",
    { label: "Edit…", run: () => instanceModal(inst) },
    { label: "Copy game settings…", run: () => copySettingsModal(inst) },
    { label: "Servers…", run: () => openTab(inst, "servers") },
    { label: inst.group ? "Move to another group…" : "Move to group…", run: () => groupModal(inst) },
    ...(canMakeShortcuts ? [{ label: "Create desktop shortcut", run: () => makeShortcut(inst) }] : []),
    S.busy.has(`dup:${inst.id}`)
      ? { label: "Duplicating…", run: () => {}, disabled: true }
      : { label: "Duplicate", run: () => duplicateInstance(inst), disabled: running },
    ...(inst.loader !== "vanilla" ? [{ label: "Export modpack…", run: () => exportModal(inst) }] : []),
    "sep",
    { label: "Delete…", run: () => deleteInstance(inst), danger: true, disabled: running },
  ];
  showMenu(ev.clientX, ev.clientY, items, `${inst.name} actions`);
});

async function openClaude(
  body: { instance_id?: string; project_id?: string; request: string; crash?: boolean; modpack?: { name: string | null; mc_version: string | null; loader: string | null } },
  dryRun = false,
) {
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
      <h2 class="center">Welcome to MioCraft</h2>
      <p class="center muted">Play Minecraft with Fabric, NeoForge, Forge or Quilt, browse Modrinth and CurseForge, and have Claude Code build mods for you.</p>
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
      <p class="muted">You can test mods right away without an account. To <strong>play</strong> your instances, you sign in with a Microsoft account that owns Minecraft: Java Edition.</p>
      <p class="muted small">${S.hasBuiltinClientId ? "Press <strong>Sign in with Microsoft</strong> in the bottom-left corner whenever you're ready." : "This build of MioCraft has no Microsoft app ID, so sign-in isn't available. Everything else works without it."}</p>
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
    if (name === "curseforge") S.cfg = await api("PATCH", "/config", { curseforge_api_key: $<HTMLInputElement>("#setup-cf").value });
  };
  const finish = async () => {
    S.cfg = await api("PATCH", "/config", { onboarded: true });
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
  // CurseForge files may only come from CurseForge, so they are never copied into a pack.
  const curseforge = enabled.filter((m) => m.online?.source === "curseforge" && !claude.includes(m));
  const other = enabled.filter((m) => !linked.includes(m) && !claude.includes(m) && !curseforge.includes(m));
  openModal(`
    <h2>Export modpack</h2>
    <p class="muted small">Creates a Modrinth modpack (.mrpack) of "${esc(inst.name)}" that other launchers can import.</p>
    <ul class="export-sum">
      <li><strong>${linked.length}</strong> Modrinth mod${linked.length === 1 ? "" : "s"}, linked (not copied)</li>
      <li><strong>${claude.length}</strong> mod${claude.length === 1 ? "" : "s"} made by Claude, included in the pack</li>
      ${curseforge.length ? `<li><strong>${curseforge.length}</strong> CurseForge mod${curseforge.length === 1 ? "" : "s"}, left out (CurseForge files can't be shared in packs, so players install ${curseforge.length === 1 ? "it" : "them"} from CurseForge): ${curseforge.map((m) => esc(m.name ?? m.file_name)).join(", ")}</li>` : ""}
      ${other.length ? `<li><strong>${other.length}</strong> other mod${other.length === 1 ? "" : "s"} (added by hand): ${other.map((m) => esc(m.name ?? m.file_name)).join(", ")}</li>` : ""}
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

$<HTMLInputElement>("#pack-file").addEventListener("change", (ev) => {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (file) importPack(file);
});

async function importPack(file: File) {
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
}

// ---------- drag and drop ----------

const DROP_KINDS: Record<string, string> = { mod: "mods", shader: "shader packs", resource_pack: "resource packs" };
let dragDepth = 0;

// Drags that start inside the page (an image, say) can also report "Files"; only outside files count.
let innerDrag = false;
document.addEventListener("dragstart", () => (innerDrag = true));
document.addEventListener("dragend", () => (innerDrag = false));
const hasFiles = (ev: DragEvent) => !innerDrag && !!ev.dataTransfer && [...ev.dataTransfer.types].includes("Files");

/** The instance a drop would go to: the card under the pointer, or the selected one. */
function dropTarget(ev: DragEvent): Instance | undefined {
  const card = (ev.target as HTMLElement | null)?.closest?.<HTMLElement>(".inst[data-id], .icard[data-id]");
  return S.instances.find((i) => i.id === card?.dataset.id) ?? current() ?? undefined;
}

function showDropOverlay(ev: DragEvent) {
  let o = document.getElementById("drop-overlay");
  if (!o) {
    o = document.createElement("div");
    o.id = "drop-overlay";
    o.innerHTML = '<div class="drop-box"><strong></strong><span class="muted small">Mods (.jar), shader and resource packs (.zip), or a modpack (.mrpack)</span></div>';
    document.body.appendChild(o);
  }
  const t = dropTarget(ev);
  o.querySelector("strong")!.textContent = t ? `Drop to add to ${t.name}` : "Drop a modpack to import it";
  document.querySelectorAll(".drop-hover").forEach((e) => e.classList.remove("drop-hover"));
  (ev.target as HTMLElement | null)?.closest?.(".inst[data-id], .icard[data-id]")?.classList.add("drop-hover");
}

function hideDropOverlay() {
  dragDepth = 0;
  document.getElementById("drop-overlay")?.remove();
  document.querySelectorAll(".drop-hover").forEach((e) => e.classList.remove("drop-hover"));
}

document.addEventListener("dragenter", (ev) => {
  if (!hasFiles(ev)) return;
  ev.preventDefault();
  dragDepth++;
  showDropOverlay(ev);
});
document.addEventListener("dragover", (ev) => {
  if (!hasFiles(ev)) return;
  ev.preventDefault();
  ev.dataTransfer!.dropEffect = "copy";
  showDropOverlay(ev);
});
document.addEventListener("dragleave", (ev) => {
  if (!hasFiles(ev)) return;
  if (--dragDepth <= 0) hideDropOverlay();
});
document.addEventListener("drop", (ev) => {
  if (!hasFiles(ev)) return;
  ev.preventDefault();
  const target = dropTarget(ev);
  hideDropOverlay();
  addDroppedFiles([...(ev.dataTransfer?.files ?? [])], target);
});

async function addDroppedFiles(files: File[], target: Instance | undefined) {
  const packs = files.filter((f) => f.name.toLowerCase().endsWith(".mrpack"));
  const rest = files.filter((f) => !packs.includes(f));
  if (rest.length && !target) toast("Make an instance first, then drop mods onto it", "err");
  if (rest.length && target) {
    const added: Record<string, number> = {};
    for (const f of rest) {
      try {
        const r = await apiBytes<{ file_name: string; kind: string; warning: string | null }>(`/instances/${enc(target.id)}/files/${enc(f.name)}`, await f.arrayBuffer());
        added[r.kind] = (added[r.kind] ?? 0) + 1;
        if (r.warning) toast(r.warning, "err");
      } catch (e) {
        toast((e as Error).message, "err");
      }
    }
    const parts = Object.entries(added).map(([k, n]) => (n === 1 ? `1 ${DROP_KINDS[k].replace(/s$/, "")}` : `${n} ${DROP_KINDS[k]}`));
    if (parts.length) toast(`Added ${parts.join(", ")} to ${target.name}`);
    if (target.id === S.selected && added.resource_pack) loadInstanceData();
  }
  // One modpack at a time: each opens its own import dialog.
  for (const f of packs) await importPack(f);
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
      await selectInstance(el.dataset.id!);
      break;
    case "view":
      setView(el.dataset.view as View, true);
      break;
    case "pick-instance":
      if (el.dataset.id !== S.selected) await selectInstance(el.dataset.id!);
      break;
    case "choose-instance":
      chooseInstanceModal();
      break;
    case "choose-pick":
      closeModal();
      await selectInstance(el.dataset.id!);
      break;
    case "tab":
      S.tab = el.dataset.tab as Tab;
      renderMain();
      break;
    case "new-instance":
      newInstanceModal();
      break;
    case "toggle-group":
      toggleGroup(el.dataset.group!);
      break;
    case "settings":
      openSettings();
      break;
    case "close-settings":
      closeSettings();
      break;
    case "stor-clean":
      askStorClean(el.dataset.part!, el.dataset.key!);
      break;
    case "stor-clean-all":
      askStorCleanAll(el.dataset.part!);
      break;
    case "set-cat":
      showSetCat(el.dataset.cat as SetCat);
      break;
    case "login":
      loginModal();
      break;
    case "accounts":
      accountsModal();
      break;
    case "outfit": {
      const acc = S.accounts.find((a) => a.id === el.dataset.id);
      if (acc) outfitModal(acc);
      break;
    }
    case "use-account":
      S.cfg = await api("PATCH", "/config", { selected_account: el.dataset.id });
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
    case "join-server":
      if (inst) playInstance(inst, el.dataset.address!);
      break;
    case "add-server":
      if (inst) serverModal(inst);
      break;
    case "edit-server": {
      const sv = inst && S.servers.get(inst.id)?.find((x) => x.index === Number(el.dataset.index));
      if (inst && sv) serverModal(inst, sv);
      break;
    }
    case "remove-server": {
      const sv = inst && S.servers.get(inst.id)?.find((x) => x.index === Number(el.dataset.index));
      if (!inst || !sv) break;
      if (!(await ask({ title: `Remove "${sv.name}"?`, body: `<code>${esc(sv.address)}</code> is removed from this instance's server list, in the game too.`, ok: "Remove", danger: true }))) break;
      try {
        S.servers.set(inst.id, await api<Server[]>("DELETE", `/instances/${enc(inst.id)}/servers/${sv.index}`));
        renderMain();
      } catch (e) {
        toast((e as Error).message, "err");
      }
      break;
    }
    case "play":
      if (inst) playInstance(inst);
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
      if (!(await ask({ title: "Delete this mod?", body: `<code>${esc(file)}</code> is removed from the instance's mods folder.`, ok: "Delete", danger: true }))) break;
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
      closeSettings();
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
    case "check-update":
      closeModal();
      lookForUpdate(true);
      break;
    case "install-update":
      el.setAttribute("disabled", "");
      el.textContent = "Updating…";
      installUpdate().catch((e) => {
        closeModal();
        toast(String((e as Error).message ?? e), "err");
      });
      break;
    case "connect-claude":
      busy("connect", async () => {
        await api("POST", "/claude-code/connect", { mcp_path: await mcpServerPath() });
        toast("Claude Code is connected to the launcher");
        lastCc = undefined;
        if (settingsOpen) showSetCat("claude", false);
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
    case "review-updates":
      updatesModal();
      break;
    case "whats-new":
      updatesModal(el.dataset.file!.replace(/\.disabled$/, ""));
      break;
    case "update-one":
      applyUpdates([el.dataset.file!.replace(/\.disabled$/, "")]);
      break;
    case "browse-source":
      S.browse.source = el.dataset.v as Source;
      S.browse.hits = [];
      runSearch(false);
      break;
    case "browse-kind":
      S.browse.kind = el.dataset.v as BrowseKind;
      S.browse.hits = [];
      runSearch(false);
      break;
    case "find-rpacks":
      S.browse.kind = "resourcepack";
      S.browse.hits = [];
      S.browse.instance = null;
      S.tab = "browse";
      renderMain();
      break;
    case "delete-rpack": {
      if (!inst) break;
      const file = el.dataset.file!;
      if (!(await ask({ title: "Delete this resource pack?", body: `<code>${esc(file)}</code> is removed from the instance's resourcepacks folder.`, ok: "Delete", danger: true }))) break;
      busy(`rpack:${file}`, async () => {
        await api("DELETE", `/instances/${enc(inst.id)}/resourcepacks/${enc(file)}`);
        S.browse.instance = null; // refresh the "Installed" badges
      });
      break;
    }
    case "delete-shader": {
      if (!inst) break;
      const file = el.dataset.file!;
      if (!(await ask({ title: "Delete this shader pack?", body: `<code>${esc(file)}</code> is removed from the instance's shaderpacks folder.`, ok: "Delete", danger: true }))) break;
      busy(`shader:${file}`, async () => {
        await api("DELETE", `/instances/${enc(inst.id)}/shaders/${enc(file)}`);
        S.browse.instance = null; // refresh the "Installed" badges
      });
      break;
    }
    case "install-iris": {
      if (!inst) break;
      busy(`get:modrinth:${IRIS_ID}`, async () => {
        const r = await api<{ installed: string[]; warnings: string[] }>("POST", `/instances/${enc(inst.id)}/online-mods`, {
          source: "modrinth",
          project_id: IRIS_ID,
        });
        toast(r.installed.length ? `Installed Iris${r.installed.length > 1 ? ` and ${r.installed.length - 1} dependenc${r.installed.length === 2 ? "y" : "ies"}` : ""}` : "Iris is already installed");
        r.warnings.forEach((w) => toast(w, "err"));
      });
      break;
    }
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
    case "con-only":
      C.only = el.dataset.v as ConOnly;
      renderMain();
      break;
    case "con-err":
      jumpError(el.dataset.dir === "up" ? -1 : 1);
      break;
    case "show-crash":
      if (inst) {
        S.tab = "console";
        openLogFile(inst, `crash/${el.dataset.name}`);
      }
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
  if (el.dataset.action === "toggle-rpack") {
    const inst = current();
    if (!inst) return;
    const file = el.dataset.file!;
    busy(`rpack:${file}`, () => api("POST", `/instances/${enc(inst.id)}/resourcepacks/${enc(file)}/enabled`, { enabled: el.checked }));
    return;
  }
  if (el.dataset.action === "mod-set") {
    const inst = current();
    const v = S.modSets;
    if (!inst || !v) return;
    const pick = el.value;
    // Show the set the folder is in until something actually changes.
    el.value = v.active ? `set:${v.active}` : "";
    if (pick.startsWith("set:") && pick.slice(4) !== v.active) modSetCall(inst, "/switch", { name: pick.slice(4) });
    else if (pick === "new") modSetNameModal(inst, null);
    else if (pick === "rename" && v.active) modSetNameModal(inst, v.active);
    else if (pick === "remove" && v.active) removeModSet(inst, v.active);
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
  if (el.dataset.action === "filter-instances") {
    S.instQ = el.value;
    renderSidebar();
    return;
  }
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
  if ((ev.ctrlKey || ev.metaKey) && !ev.altKey && ev.key.toLowerCase() === "f") {
    ev.preventDefault();
    focusSearch();
    return;
  }
  if (ev.key !== "Escape") return;
  if ($("#modal-root").classList.contains("show")) closeModal();
  else closeSettings();
});

/** Ctrl+F: the search box of the current tab (Browse, or the mod list). */
function focusSearch() {
  if (menuOpen()) closeMenu();
  if ($("#modal-root").classList.contains("show") || !current()) return;
  if (settingsOpen) closeSettings();
  if (S.view !== "instances") setView("instances", true);
  if (S.tab !== "browse" && S.tab !== "mods") {
    S.tab = "mods";
    renderMain();
  }
  const box = document.querySelector<HTMLInputElement>(S.tab === "browse" ? "#browse-q" : "#mod-filter");
  box?.focus();
  box?.select();
}

// Double-clicking an instance card on the home screen opens its page.
document.addEventListener("dblclick", (ev) => {
  const card = (ev.target as HTMLElement).closest<HTMLElement>('[data-action="pick-instance"]');
  if (card) setView("instances", true);
});

// ---------- app updates ----------

/** Ask the shell for a newer release and offer it. `manual` reports "up to date" and errors too. */
async function lookForUpdate(manual: boolean) {
  try {
    const u = await checkUpdate();
    if (!u) {
      if (manual) toast("MioCraft is up to date");
      return;
    }
    openModal(`
      <h2>Update available</h2>
      <p>MioCraft <strong>${esc(u.version)}</strong> is available. You have ${esc(u.current_version)}.</p>
      ${u.notes ? `<p class="muted small" style="white-space:pre-wrap">${esc(u.notes)}</p>` : ""}
      <p class="muted small">The update is downloaded from GitHub, its signature is checked, and MioCraft restarts.</p>
      <div class="modal-actions">
        <button class="btn ghost" data-action="close-modal">Later</button>
        <button class="btn primary" data-action="install-update">Update and restart</button>
      </div>`);
  } catch (e) {
    if (manual) toast(`Couldn't check for updates: ${(e as Error).message ?? e}`, "err");
  }
}

// ---------- start ----------

(async () => {
  try {
    await connect();
    const cfg = await api("GET", "/config");
    S.cfg = cfg;
    applyColors(!!cfg.legacy_colors);
    applyStyle(cfg.stylized !== false);
    applyLook(!cfg.legacy_look);
    events(onEvent);
    await Promise.all([loadAccounts(), loadInstances()]);
    // Desktop shortcuts: the one that started MioCraft, and later ones while it's open.
    onPlayInstance(playFromShortcut).catch(() => {});
    takeStartupPlay()
      .then((id) => id && playFromShortcut(id))
      .catch(() => {});
    // Get Settings ready in the background so it opens instantly.
    loadSettingsInfo().catch(() => {});
    refreshClaudeCode();
    if (!cfg.onboarded) setupModal(0);
    else lookForUpdate(false);
  } catch (e) {
    setView("instances");
    $("#main").innerHTML = `<div class="empty"><h1>Couldn't start</h1><p>${esc((e as Error).message)}</p></div>`;
  }
})();
