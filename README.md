# Claude Launcher

A lightweight Minecraft launcher for Windows with Claude Code built in. Ask
Claude Code for a mod and it creates the project, writes the code, builds it and
installs it. The mod then shows up in the launcher's mod list, tagged
**✦ Made by Claude**, ready to play.

- **Instances** for any Minecraft version, with **Fabric** or **NeoForge** (or vanilla)
- **Mod list** per instance: enable/disable, delete, search, and a *Rebuild* button for Claude-made mods
- **Mod browser** for **Modrinth** and **CurseForge**: results are filtered to the instance's Minecraft version and loader, and required dependencies install automatically
- **Mod updates**: check browser-installed mods for newer compatible versions and update one or all
- **Make a mod with Claude**: describe a mod in the launcher and it opens Claude Code with your idea and the instance ready; "Ask Claude" does the same for changing an existing mod
- **Modpacks**: import Modrinth `.mrpack` files (Fabric and NeoForge) and export instances to share, with Claude's mods included
- **Sign-in tokens in Windows Credential Manager**, not in a plain file
- **Microsoft account sign-in**
- **Java and Gradle downloaded automatically**, so you don't need to install anything to play or to build mods
- **MCP server for Claude Code** with 23 tools: create instances and mods, build, install, run a test client, read logs and crash reports
- Small: a Tauri app (Rust + a web UI), not Electron

## How it fits together

```
 Claude Code ──stdio──► MCP server (mcp/) ──HTTP──┐
                                                  ▼
 Launcher window (ui/) ──HTTP + live events──► launcher core (crates/core)
                                               127.0.0.1, random port, token
```

All the launcher logic lives in `crates/core`. The desktop app (`src-tauri/`) is
a thin shell that starts the core and shows the UI. While the launcher is open,
the core serves a local API on `127.0.0.1`, protected by a random token. It
writes the port and token to `api.json` in the data folder, which is how the MCP
server finds it.

Data lives in `%APPDATA%\ClaudeLauncher`:

```
instances/<id>/minecraft/      game folder (mods/, saves/, logs/ …)
instances/<id>/claude-mods.json which jars in mods/ Claude built
instances/<id>/online-mods.json which jars came from Modrinth / CurseForge
exports/                       exported .mrpack modpacks
projects/<mod_id>/             mod source code Claude Code edits (has its own CLAUDE.md)
versions/ libraries/ assets/   shared Minecraft files
java/ gradle/                  downloaded JDKs and Gradle
```

## Getting started

### 1. Build or download

Push this repo to GitHub. The **Build** workflow produces a Windows installer
under the run's *Artifacts*. Tag a commit `v0.1.0` to attach it to a release.

To build locally on Windows you need [Rust](https://rustup.rs), Node 18+ and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) (WebView2 is
already on Windows 10/11):

```
npm run setup
npm run dev      # run with hot reload
npm run build    # make the installer in target/release/bundle/
```

### 2. Connect Claude Code

The launcher's MCP server runs on [Node.js](https://nodejs.org) (LTS), so install
that alongside [Claude Code](https://claude.com/claude-code). Then open the
launcher, go to **Settings → Connect Claude Code** and press **Connect
automatically**. Or run the command it shows, which looks like this:

```
claude mcp add --scope user claude-launcher -- node "C:\Program Files\Claude Launcher\mcp\claude-launcher-mcp.mjs"
```

Keep the launcher open while Claude Code works. Then ask something like:

> Make a Fabric mod for my "Creative Lab" instance that makes ores glow faintly.

Claude Code creates the project, edits the code, runs `build_mod` until it
compiles, installs it with `install_mod`, and can test it in an offline dev client
with `run_dev_client`.

You can also start from the launcher: on an instance's **Claude projects** tab,
describe the mod and press **Open in Claude Code**. A terminal opens with Claude
Code and a prompt that already names the instance, its Minecraft version and
loader, and the tools to use.

### 3. CurseForge (optional)

Modrinth works out of the box. To browse CurseForge too, get a free API key at
[console.curseforge.com](https://console.curseforge.com/) and paste it into
**Settings → CurseForge API key**. Some CurseForge authors don't allow downloads
from other apps; for those, the launcher shows a link to the mod's page so you
can download it yourself and drop it in the mods folder.

### 4. Microsoft sign-in (needed to play)

Testing mods in the dev client needs no account. To **play** an instance you
need to sign in with a Microsoft account that owns Minecraft: Java Edition.
Microsoft requires every third-party launcher to use its own Azure app:

1. Go to [portal.azure.com](https://portal.azure.com) → **App registrations** → **New registration**.
2. Name it (e.g. "Claude Launcher"). For supported account types choose **Personal Microsoft accounts only**.
3. Leave the redirect URI empty and register.
4. In **Authentication**, set **Allow public client flows** to **Yes** and save.
5. Copy the **Application (client) ID** and paste it into the launcher's **Settings → Azure client ID**.
6. New apps must be approved for the Minecraft API before sign-in works. Submit
   your client ID through Mojang's review form (linked from Microsoft's
   Minecraft launcher documentation). Until approval, sign-in fails at the
   "Minecraft rejected the sign-in" step.

## MCP tools

| Tool | What it does |
|---|---|
| `launcher_status` | Check the launcher is running; data folder, sign-in state |
| `list_instances`, `create_instance` | See or make instances (downloads Minecraft, the loader and Java) |
| `list_minecraft_versions`, `list_loader_versions` | Available versions |
| `list_mods`, `set_mod_enabled` | An instance's mod list |
| `search_online_mods`, `install_online_mod` | Find and install Modrinth / CurseForge mods (with required dependencies) |
| `check_mod_updates`, `update_mods` | Update browser-installed mods |
| `export_modpack` | Export an instance as a `.mrpack` |
| `list_mod_projects`, `get_mod_project`, `create_mod` | Mod projects from version-matched templates |
| `build_mod` | Compile with Gradle; returns compiler errors |
| `install_mod` | Build and install into the instance (replaces older builds) |
| `run_dev_client`, `stop_dev_client`, `read_dev_log` | Offline test client (`gradle runClient`) |
| `launch_game`, `stop_game`, `read_game_log` | Play the real instance; read `latest.log`, raw output or the newest crash report |

## Version support

- **Playing**: any Minecraft version in Mojang's manifest, including snapshots.
- **Fabric mod templates**: Minecraft 1.14.4 and newer (official Mojang mappings;
  no mappings from 26.1, which is unobfuscated).
- **NeoForge**: Minecraft 1.20.2 and newer (NeoForge doesn't exist for older versions).

## Developing

```
cargo test -p launcher-core                         # core unit + API tests
cargo run -p launcher-core --bin launcher-headless  # the core without a window
cd ui && npx vite                                   # then open http://localhost:5173/?port=<port>&token=<token> from api.json
```

Set `CLAUDE_LAUNCHER_HOME` to use a separate data folder while developing (both
the core and the MCP server respect it).

See [NOTES-FOR-CLAUDE-CODE.md](NOTES-FOR-CLAUDE-CODE.md) for what still needs
checking on a real Windows machine.

## Modpacks

**Import**: the download button next to *Instances* takes a `.mrpack`. Fabric and
NeoForge packs are supported (Forge and Quilt aren't). Files are only downloaded
from the hosts the format allows (Modrinth's CDN, GitHub, GitLab) and checked
against their hashes.

**Export**: *Export* on an instance writes a `.mrpack` to the `exports` folder.
Modrinth mods are linked rather than copied, and mods Claude made are included.
Other mods (from CurseForge or added by hand) are left out unless you tick the
box confirming you're allowed to share them.

## Legal

Not affiliated with Mojang, Microsoft or Anthropic. Playing requires a
Minecraft: Java Edition account. The launcher downloads game files from Mojang's
servers and doesn't redistribute them.
