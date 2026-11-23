// Thin client for MioCraft's local API. The launcher writes its port
// and token to `api.json` in its data folder each time it starts.

import { readFile } from "node:fs/promises";
import { request } from "node:http";
import { homedir } from "node:os";
import { join } from "node:path";

export interface ApiInfo {
  port: number;
  token: string;
  pid: number;
  version: string;
}

export function dataDir(): string {
  if (process.env.MIO_CRAFT_HOME) return process.env.MIO_CRAFT_HOME;
  if (process.platform === "win32") {
    return join(process.env.APPDATA ?? join(homedir(), "AppData", "Roaming"), "MioCraft");
  }
  if (process.platform === "darwin") {
    return join(homedir(), "Library", "Application Support", "MioCraft");
  }
  return join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "MioCraft");
}

export class LauncherNotRunning extends Error {}

async function apiInfo(): Promise<ApiInfo> {
  const file = join(dataDir(), "api.json");
  try {
    return JSON.parse(await readFile(file, "utf8")) as ApiInfo;
  } catch {
    throw new LauncherNotRunning(
      `MioCraft doesn't seem to be running (no ${file}). Ask the user to open MioCraft, then try again.`,
    );
  }
}

export async function call<T = unknown>(
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
): Promise<T> {
  const info = await apiInfo();
  let res: { status: number; text: string };
  try {
    res = await send(info, method, path, body);
  } catch {
    throw new LauncherNotRunning(
      "Couldn't reach MioCraft. Ask the user to open (or restart) MioCraft, then try again.",
    );
  }
  const text = res.text;
  let data: unknown;
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (res.status < 200 || res.status >= 300) {
    const msg = (data as { error?: string }).error ?? `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return data as T;
}

/** One request to the launcher. Plain node:http rather than fetch, which gives up
 *  after 5 minutes without a response: a first Forge build takes longer than that. */
function send(info: ApiInfo, method: string, path: string, body?: unknown): Promise<{ status: number; text: string }> {
  const payload = body !== undefined ? JSON.stringify(body) : undefined;
  return new Promise((resolve, reject) => {
    const req = request(
      {
        host: "127.0.0.1",
        port: info.port,
        path: `/api${path}`,
        method,
        headers: {
          Authorization: `Bearer ${info.token}`,
          ...(payload !== undefined ? { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(payload) } : {}),
        },
      },
      (res) => {
        const chunks: Buffer[] = [];
        res.on("data", (c: Buffer) => chunks.push(c));
        res.on("end", () => resolve({ status: res.statusCode ?? 0, text: Buffer.concat(chunks).toString("utf8") }));
        res.on("error", reject);
      },
    );
    req.on("error", reject);
    if (payload !== undefined) req.write(payload);
    req.end();
  });
}

export const enc = encodeURIComponent;
