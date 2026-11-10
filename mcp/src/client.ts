// Thin client for Claude Launcher's local API. The launcher writes its port
// and token to `api.json` in its data folder each time it starts.

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

export interface ApiInfo {
  port: number;
  token: string;
  pid: number;
  version: string;
}

export function dataDir(): string {
  if (process.env.CLAUDE_LAUNCHER_HOME) return process.env.CLAUDE_LAUNCHER_HOME;
  if (process.platform === "win32") {
    return join(process.env.APPDATA ?? join(homedir(), "AppData", "Roaming"), "ClaudeLauncher");
  }
  if (process.platform === "darwin") {
    return join(homedir(), "Library", "Application Support", "ClaudeLauncher");
  }
  return join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "ClaudeLauncher");
}

export class LauncherNotRunning extends Error {}

async function apiInfo(): Promise<ApiInfo> {
  const file = join(dataDir(), "api.json");
  try {
    return JSON.parse(await readFile(file, "utf8")) as ApiInfo;
  } catch {
    throw new LauncherNotRunning(
      `Claude Launcher doesn't seem to be running (no ${file}). Ask the user to open Claude Launcher, then try again.`,
    );
  }
}

export async function call<T = unknown>(
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
): Promise<T> {
  const info = await apiInfo();
  let res: Response;
  try {
    res = await fetch(`http://127.0.0.1:${info.port}/api${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${info.token}`,
        ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new LauncherNotRunning(
      "Couldn't reach Claude Launcher. Ask the user to open (or restart) Claude Launcher, then try again.",
    );
  }
  const text = await res.text();
  let data: unknown;
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (!res.ok) {
    const msg = (data as { error?: string }).error ?? `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return data as T;
}

export const enc = encodeURIComponent;
