#!/usr/bin/env node
// MioCraft MCP server (stdio). Add it to Claude Code with:
//   claude mcp add mio-craft -- node "<path>/mio-craft-mcp.mjs"

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

import { call, enc } from "./client.js";

const server = new McpServer(
  { name: "mio-craft", version: "0.2.0" },
  {
    instructions: [
      "Tools for making Minecraft mods with MioCraft.",
      "Typical flow: list_instances (or create_instance) → create_mod → edit the files in the returned path",
      "(read its CLAUDE.md first) → build_mod and fix any errors → install_mod so it appears in the launcher's",
      "mod list → run_dev_client to test offline, read_dev_log to check output → stop_dev_client.",
      "To add third-party mods or libraries use search_online_mods then install_online_mod; the same two tools find and install shader packs with kind 'shader' (they need the Iris mod).",
      "For changes while the game runs, check get_live_status: hotswap_mod swaps code, define_live_block / define_live_item add content without restarting.",
      "Use launch_game only when the user wants to play their real instance; read_game_log with kind 'crash' after a crash.",
      "Don't run Gradle directly: these tools use the launcher's bundled Java and Gradle.",
    ].join(" "),
  },
);

type Json = Record<string, unknown>;

function text(value: unknown) {
  return { content: [{ type: "text" as const, text: typeof value === "string" ? value : JSON.stringify(value, null, 2) }] };
}

async function run(fn: () => Promise<unknown>) {
  try {
    return text(await fn());
  } catch (e) {
    return { ...text(`Error: ${(e as Error).message}`), isError: true };
  }
}

interface BuildResult {
  success: boolean;
  jar: string | null;
  errors: string[];
  output_tail: string;
  installed_as: string | null;
}

function formatBuild(r: BuildResult, verb: string): string {
  if (r.success) {
    let s = `${verb} succeeded. Jar: ${r.jar}`;
    if (r.installed_as) s += `\nInstalled into the instance as ${r.installed_as}; it now shows in the launcher's mod list, tagged as made by Claude.`;
    return s;
  }
  const errs = r.errors.length ? `Errors:\n${r.errors.join("\n")}\n\n` : "";
  return `${verb} FAILED.\n\n${errs}Last Gradle output:\n${r.output_tail}`;
}

// ---------- launcher & instances ----------

server.registerTool(
  "launcher_status",
  {
    description: "Check that MioCraft is running; shows its data folder, running processes and sign-in state.",
    inputSchema: {},
  },
  async () => run(() => call("GET", "/status")),
);

server.registerTool(
  "list_instances",
  {
    description: "List Minecraft instances (Minecraft version, mod loader, install state, mods folder).",
    inputSchema: {},
  },
  async () =>
    run(async () => {
      const list = await call<Json[]>("GET", "/instances");
      if (!list.length) return "No instances yet. Create one with create_instance.";
      return list.map((i) => ({
        id: i.id,
        name: i.name,
        minecraft: i.mc_version,
        loader: i.loader,
        loader_version: i.loader_version,
        installed: i.installed,
        running: i.running,
        mods_dir: i.mods_dir,
      }));
    }),
);

server.registerTool(
  "list_minecraft_versions",
  {
    description: "List Minecraft release versions (newest first). Set snapshots to include snapshots.",
    inputSchema: { snapshots: z.boolean().optional(), limit: z.number().int().positive().optional() },
  },
  async ({ snapshots, limit }) =>
    run(async () => {
      const r = await call<{ latest: Json; versions: { id: string; type: string }[] }>(
        "GET",
        `/versions?snapshots=${snapshots ? "true" : "false"}`,
      );
      return { latest: r.latest, versions: r.versions.slice(0, limit ?? 40).map((v) => v.id) };
    }),
);

server.registerTool(
  "list_loader_versions",
  {
    description: "List Fabric loader or NeoForge versions available for a Minecraft version.",
    inputSchema: { loader: z.enum(["fabric", "neoforge"]), mc_version: z.string() },
  },
  async ({ loader, mc_version }) =>
    run(async () => {
      const v = await call<{ version: string; stable: boolean }[]>("GET", `/loaders/${loader}/${enc(mc_version)}`);
      return v.slice(0, 20);
    }),
);

server.registerTool(
  "create_instance",
  {
    description:
      "Create a Minecraft instance with a mod loader. Downloads Minecraft, the loader and Java unless install is false (this can take a few minutes the first time).",
    inputSchema: {
      name: z.string().optional(),
      mc_version: z.string().describe("e.g. 1.21.1"),
      loader: z.enum(["fabric", "neoforge", "vanilla"]),
      loader_version: z.string().optional().describe("Defaults to the latest stable version"),
      install: z.boolean().optional().describe("Default true"),
    },
  },
  async ({ name, mc_version, loader, loader_version, install }) =>
    run(async () => {
      let inst = await call<Json>("POST", "/instances", { name, mc_version, loader, loader_version });
      if (install !== false) inst = await call<Json>("POST", `/instances/${enc(String(inst.id))}/install`);
      return inst;
    }),
);

// ---------- mods in an instance ----------

server.registerTool(
  "list_mods",
  {
    description: "List the mods installed in an instance, including which ones Claude built (claude_project).",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) => run(() => call("GET", `/instances/${enc(instance_id)}/mods`)),
);

server.registerTool(
  "set_mod_enabled",
  {
    description: "Enable or disable a mod in an instance by its file name (from list_mods).",
    inputSchema: { instance_id: z.string(), file_name: z.string(), enabled: z.boolean() },
  },
  async ({ instance_id, file_name, enabled }) =>
    run(() => call("POST", `/instances/${enc(instance_id)}/mods/${enc(file_name)}/enabled`, { enabled })),
);

server.registerTool(
  "search_online_mods",
  {
    description:
      "Search Modrinth or CurseForge for third-party mods that work with an instance's Minecraft version and loader, or for shader packs (kind 'shader') for its Minecraft version. CurseForge only works if the user has added an API key in Settings.",
    inputSchema: {
      instance_id: z.string(),
      query: z.string().describe("Search text; empty lists popular mods"),
      source: z.enum(["modrinth", "curseforge"]).optional().describe("Default modrinth"),
      sort: z.enum(["relevance", "downloads", "updated", "newest"]).optional(),
      limit: z.number().int().positive().max(50).optional(),
      kind: z.enum(["mod", "shader"]).optional().describe("Default mod"),
    },
  },
  async ({ instance_id, query, source, sort, limit, kind }) =>
    run(async () => {
      const src = source ?? "modrinth";
      const r = await call<{ hits: Json[]; total: number }>(
        "GET",
        `/browse/${src}/search?instance_id=${enc(instance_id)}&q=${enc(query)}&sort=${sort ?? "relevance"}&limit=${limit ?? 10}&kind=${kind ?? "mod"}`,
      );
      return {
        total: r.total,
        results: r.hits.map((h) => ({
          project_id: h.project_id,
          title: h.title,
          author: h.author,
          downloads: h.downloads,
          description: h.description,
          installed: h.installed,
          page: h.page_url,
        })),
      };
    }),
);

server.registerTool(
  "install_online_mod",
  {
    description:
      "Install a third-party mod from Modrinth or CurseForge into an instance (newest compatible version unless version_id is given), along with its required dependencies. With kind 'shader' it installs a shader pack into the instance's shaderpacks folder instead. Use project_id from search_online_mods with the same kind. Only install mods the user asked for or that a mod you're building needs.",
    inputSchema: {
      instance_id: z.string(),
      project_id: z.string(),
      source: z.enum(["modrinth", "curseforge"]).optional().describe("Default modrinth"),
      version_id: z.string().optional(),
      kind: z.enum(["mod", "shader"]).optional().describe("Default mod"),
    },
  },
  async ({ instance_id, project_id, source, version_id, kind }) =>
    run(() =>
      call("POST", `/instances/${enc(instance_id)}/${kind === "shader" ? "shaders" : "online-mods"}`, {
        source: source ?? "modrinth",
        project_id,
        version_id,
      }),
    ),
);

server.registerTool(
  "check_mod_updates",
  {
    description: "Check an instance's Modrinth / CurseForge mods for newer compatible versions.",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) =>
    run(async () => {
      const r = await call<{ updates: Json[]; checked: number; warnings: string[] }>("GET", `/instances/${enc(instance_id)}/updates`);
      return {
        checked: r.checked,
        updates: r.updates.map((u) => ({ file_name: u.file_name, title: u.title, latest: (u.latest as Json).version_number })),
        warnings: r.warnings,
      };
    }),
);

server.registerTool(
  "update_mods",
  {
    description: "Update an instance's Modrinth / CurseForge mods. Pass file_names to update only some; omit to update all.",
    inputSchema: { instance_id: z.string(), file_names: z.array(z.string()).optional() },
  },
  async ({ instance_id, file_names }) =>
    run(() => call("POST", `/instances/${enc(instance_id)}/updates`, { files: file_names })),
);

server.registerTool(
  "export_modpack",
  {
    description:
      "Export an instance as a Modrinth modpack (.mrpack). Modrinth mods are linked, Claude-made mods are included; other mods are left out unless include_other_jars is true (only when their licences allow sharing).",
    inputSchema: {
      instance_id: z.string(),
      version: z.string().optional(),
      include_config: z.boolean().optional(),
      include_other_jars: z.boolean().optional(),
    },
  },
  async ({ instance_id, version, include_config, include_other_jars }) =>
    run(() =>
      call("POST", `/instances/${enc(instance_id)}/export`, {
        version,
        include_config: include_config ?? true,
        include_other_jars: include_other_jars ?? false,
      }),
    ),
);

// ---------- mod projects ----------

server.registerTool(
  "list_mod_projects",
  {
    description: "List mod projects Claude has created, with their source folder, target instance and last build.",
    inputSchema: {},
  },
  async () => run(() => call("GET", "/projects")),
);

server.registerTool(
  "create_mod",
  {
    description:
      "Create a new mod project for an instance, from a Fabric or NeoForge template matching the instance's Minecraft version. Returns the source folder: edit files there, starting with its CLAUDE.md.",
    inputSchema: {
      instance_id: z.string(),
      mod_id: z.string().describe("lowercase a-z, 0-9, _ ; e.g. glowing_ores"),
      name: z.string().optional(),
      description: z.string().optional(),
      author: z.string().optional(),
    },
  },
  async (args) =>
    run(async () => {
      const p = await call<Json>("POST", "/projects", args);
      return {
        ...p,
        next_steps: `Source is in ${p.path}. Read CLAUDE.md there, write the mod, then call build_mod / install_mod with mod_id "${p.id}".`,
      };
    }),
);

server.registerTool(
  "get_mod_project",
  {
    description: "Details of one mod project: path, loader, versions, toolchain, last build, dev client state.",
    inputSchema: { mod_id: z.string() },
  },
  async ({ mod_id }) => run(() => call("GET", `/projects/${enc(mod_id)}`)),
);

server.registerTool(
  "build_mod",
  {
    description: "Compile a mod project with Gradle. Returns compiler errors on failure. Can take a few minutes the first time.",
    inputSchema: { mod_id: z.string() },
  },
  async ({ mod_id }) =>
    run(async () => formatBuild(await call<BuildResult>("POST", `/projects/${enc(mod_id)}/build`), "Build")),
);

server.registerTool(
  "install_mod",
  {
    description:
      "Build a mod project and install the jar into its instance (or another compatible instance), replacing earlier builds. It then appears in the launcher's mod list.",
    inputSchema: { mod_id: z.string(), instance_id: z.string().optional() },
  },
  async ({ mod_id, instance_id }) =>
    run(async () =>
      formatBuild(await call<BuildResult>("POST", `/projects/${enc(mod_id)}/install`, { instance_id }), "Build and install"),
    ),
);

server.registerTool(
  "run_dev_client",
  {
    description:
      "Start the loader's development client for a mod project (gradle runClient). Runs offline with a test player, so no account is needed. Returns immediately; use read_dev_log to follow it.",
    inputSchema: { mod_id: z.string() },
  },
  async ({ mod_id }) =>
    run(async () => {
      await call("POST", `/projects/${enc(mod_id)}/dev`);
      return "Dev client starting. It takes a while to reach the title screen; check read_dev_log for progress and errors.";
    }),
);

server.registerTool(
  "stop_dev_client",
  {
    description: "Stop a running dev client.",
    inputSchema: { mod_id: z.string() },
  },
  async ({ mod_id }) => run(() => call("DELETE", `/projects/${enc(mod_id)}/dev`)),
);

server.registerTool(
  "read_dev_log",
  {
    description: "Read the last lines of a mod project's dev client log.",
    inputSchema: { mod_id: z.string(), lines: z.number().int().positive().max(5000).optional() },
  },
  async ({ mod_id, lines }) =>
    run(async () => (await call<{ text: string }>("GET", `/projects/${enc(mod_id)}/dev/logs?lines=${lines ?? 200}`)).text),
);

// ---------- playing ----------

server.registerTool(
  "launch_game",
  {
    description: "Launch an instance with the user's signed-in Minecraft account (installs missing files first).",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) =>
    run(async () => {
      await call("POST", `/instances/${enc(instance_id)}/launch`, {});
      return "Game launching. Use read_game_log to follow it.";
    }),
);

server.registerTool(
  "stop_game",
  {
    description: "Force-stop a running game instance.",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) => run(() => call("POST", `/instances/${enc(instance_id)}/stop`)),
);

// ---------- live mode ----------

const behaviourSchema = z
  .object({
    project_id: z.string().describe("Mod project whose compiled classes contain the behaviour"),
    class_name: z.string().describe("Fully qualified class implementing dev.miocraft.live.api.LiveBlockBehaviour / LiveItemBehaviour"),
  })
  .optional();

server.registerTool(
  "get_live_status",
  {
    description:
      "Live mode state of an instance: which toggles are on (hotswap, content_slots, in_memory_textures, unfreeze_registries), whether the game is running with the hotswap agent, slots used, and defined live content with their in-game ids.",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) => run(() => call("GET", `/instances/${enc(instance_id)}/live`)),
);

server.registerTool(
  "hotswap_mod",
  {
    description:
      "Compile a mod project and push changed classes into the running game (needs live mode Hotswap, and the mod installed when the game started). Method bodies always swap; adding methods or fields works thanks to JetBrains Runtime. Code that only runs at startup won't run again, and resources need F3+T or /reload. The jar is updated automatically when the game exits. Also refreshes live behaviours.",
    inputSchema: { mod_id: z.string() },
  },
  async ({ mod_id }) => run(() => call("POST", `/projects/${enc(mod_id)}/hotswap`)),
);

server.registerTool(
  "define_live_block",
  {
    description:
      "Create or update a block in the running game without restarting (live mode Content slots). It fills a pre-registered slot: name, texture, hardness, light, drops and behaviour all apply live. Light applies to blocks placed after the change. Set register=true to instead register a real block (experimental route 4, needs Unfreeze registries; triggers one resource reload). Write the texture as a 16x16 PNG file and pass its absolute path.",
    inputSchema: {
      instance_id: z.string(),
      id: z.string().describe("lowercase a-z, 0-9, _"),
      name: z.string(),
      texture_path: z.string().optional(),
      hardness: z.number().optional().describe("Stone is 1.5, obsidian 50, -1 unbreakable"),
      resistance: z.number().optional(),
      light: z.number().int().min(0).max(15).optional(),
      requires_tool: z.boolean().optional(),
      drops: z.array(z.object({ item: z.string(), min: z.number().int().optional(), max: z.number().int().optional() })).optional(),
      behaviour: behaviourSchema,
      register: z.boolean().optional(),
    },
  },
  async ({ instance_id, ...b }) => run(() => call("POST", `/instances/${enc(instance_id)}/live/blocks`, { ...b, register: b.register ?? false })),
);

server.registerTool(
  "define_live_item",
  {
    description: "Create or update an item in the running game without restarting; see define_live_block.",
    inputSchema: {
      instance_id: z.string(),
      id: z.string(),
      name: z.string(),
      texture_path: z.string().optional(),
      max_stack: z.number().int().min(1).max(99).optional(),
      behaviour: behaviourSchema,
      register: z.boolean().optional(),
    },
  },
  async ({ instance_id, ...b }) => run(() => call("POST", `/instances/${enc(instance_id)}/live/items`, { ...b, register: b.register ?? false })),
);

server.registerTool(
  "remove_live_content",
  {
    description: "Remove a live block or item definition, freeing its slot. Runtime-registered content stays registered until the game restarts.",
    inputSchema: { instance_id: z.string(), kind: z.enum(["block", "item"]), id: z.string() },
  },
  async ({ instance_id, kind, id }) => run(() => call("DELETE", `/instances/${enc(instance_id)}/live/${kind}/${enc(id)}`)),
);

server.registerTool(
  "give_live_content",
  {
    description: "Give the player a live block or item in the running singleplayer game.",
    inputSchema: { instance_id: z.string(), kind: z.enum(["block", "item"]), id: z.string(), count: z.number().int().positive().optional() },
  },
  async ({ instance_id, ...b }) => run(() => call("POST", `/instances/${enc(instance_id)}/live/give`, { ...b, count: b.count ?? 1 })),
);

server.registerTool(
  "run_game_command",
  {
    description:
      "Run a Minecraft command in the user's running singleplayer world, as the server (e.g. \"time set day\", \"tp @p 0 100 0\"). Results are in claude-live/commands.log. Only use commands the user asked for or that are clearly needed to test their mod.",
    inputSchema: { instance_id: z.string(), command: z.string() },
  },
  async ({ instance_id, command }) => run(() => call("POST", `/instances/${enc(instance_id)}/live/command`, { command })),
);

server.registerTool(
  "get_crash_info",
  {
    description:
      "Summary of an instance's latest crash: description, first exception, report and log paths, and installed mods that appear in the stack trace (with their mod project id if Claude made them). Returns null if there's no recent crash.",
    inputSchema: { instance_id: z.string() },
  },
  async ({ instance_id }) => run(async () => (await call("GET", `/instances/${enc(instance_id)}/crash`)) ?? "No recent crash."),
);

server.registerTool(
  "read_game_log",
  {
    description:
      "Read an instance's logs. kind: 'game' (logs/latest.log), 'launcher' (raw process output, useful if the game died early) or 'crash' (newest crash report).",
    inputSchema: {
      instance_id: z.string(),
      kind: z.enum(["game", "launcher", "crash"]).optional(),
      lines: z.number().int().positive().max(5000).optional(),
    },
  },
  async ({ instance_id, kind, lines }) =>
    run(async () => {
      const r = await call<{ path: string | null; text: string }>(
        "GET",
        `/instances/${enc(instance_id)}/logs?kind=${kind ?? "game"}&lines=${lines ?? 200}`,
      );
      return r.path ? `${r.path}\n\n${r.text}` : r.text;
    }),
);

await server.connect(new StdioServerTransport());
