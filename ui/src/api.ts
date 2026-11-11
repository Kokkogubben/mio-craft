// Talks to the launcher core's local API. Inside the desktop app the address
// comes from Tauri; in a plain browser (development against
// `launcher-headless`) pass ?port=…&token=… in the URL.

export interface ApiInfo {
  port: number;
  token: string;
}

export const inTauri = "__TAURI_INTERNALS__" in window;

let info: ApiInfo | null = null;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function connect(): Promise<ApiInfo> {
  if (info) return info;
  if (inTauri) {
    info = await invoke<ApiInfo>("get_api");
  } else {
    const q = new URLSearchParams(location.search);
    const port = Number(q.get("port"));
    const token = q.get("token") ?? "";
    if (!port || !token) throw new Error("Open with ?port=<port>&token=<token> from api.json");
    info = { port, token };
  }
  return info;
}

function base(): string {
  return `http://127.0.0.1:${info!.port}/api`;
}

export async function api<T = any>(method: string, path: string, body?: unknown): Promise<T> {
  await connect();
  const res = await fetch(base() + path, {
    method,
    headers: {
      Authorization: `Bearer ${info!.token}`,
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : {};
  if (!res.ok) throw new Error(data.error ?? `HTTP ${res.status}`);
  return data as T;
}

export async function apiBytes<T = any>(path: string, data: ArrayBuffer): Promise<T> {
  await connect();
  const res = await fetch(base() + path, {
    method: "POST",
    headers: { Authorization: `Bearer ${info!.token}`, "Content-Type": "application/octet-stream" },
    body: data,
  });
  const text = await res.text();
  const body = text ? JSON.parse(text) : {};
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`);
  return body as T;
}

export function events(onEvent: (e: any) => void): EventSource {
  const es = new EventSource(`${base()}/events?token=${encodeURIComponent(info!.token)}`);
  es.onmessage = (m) => {
    try {
      onEvent(JSON.parse(m.data));
    } catch {
      /* ignore */
    }
  };
  return es;
}

export async function openPath(path: string): Promise<void> {
  if (inTauri) await invoke("open_path", { path });
  else await navigator.clipboard?.writeText(path);
}

export async function openUrl(url: string): Promise<void> {
  if (inTauri) await invoke("open_url", { url });
  else window.open(url, "_blank");
}

export interface AppUpdate {
  version: string;
  current_version: string;
  notes: string | null;
}

/** A newer release, or null when up to date (always null outside the installed app). */
export async function checkUpdate(): Promise<AppUpdate | null> {
  return inTauri ? invoke<AppUpdate | null>("check_update") : null;
}

/** Downloads and installs the newest release, then restarts the app. */
export async function installUpdate(): Promise<void> {
  if (inTauri) await invoke("install_update");
}

export async function appVersion(): Promise<string | null> {
  return inTauri ? invoke<string>("app_version") : null;
}

export async function mcpServerPath(): Promise<string> {
  if (inTauri) return invoke<string>("mcp_server_path");
  return "<path to>/mio-craft-mcp.mjs";
}

export const enc = encodeURIComponent;
