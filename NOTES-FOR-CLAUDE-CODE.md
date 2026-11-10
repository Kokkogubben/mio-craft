# Notes for Claude Code

This project was written in a sandbox that could **not** reach Mojang, Fabric,
NeoForge, Adoptium or Gradle servers, and couldn't build for Windows. Everything
below compiles and the core's unit tests pass (`cargo test -p launcher-core`, 24
tests including a live test of the local API), the MCP server was smoke-tested
against the headless core, and the UI was checked in a browser against seeded
data. What follows has **not** been run for real yet. Please verify it on the
user's Windows machine, roughly in this order, and fix whatever breaks.

## 1. Desktop shell compiles (`src-tauri/`)

Never compiled. Run `npm run setup` then `npm run dev` and fix any errors in
`src-tauri/src/lib.rs`. Things to look at:
- `tauri_plugin_opener::open_path` / `open_url` signatures.
- `tauri.conf.json`: `beforeDevCommand` / `beforeBuildCommand` use the object
  form with `"cwd": ".."` and the bundle resource mapping for the MCP script.
- The webview (`http://tauri.localhost`) calls the API at `http://127.0.0.1:<port>`;
  the core sends permissive CORS headers. Confirm `fetch` and `EventSource`
  both work (no mixed-content or CSP blocking; CSP is null).

## 2. Vanilla install and launch

Create a vanilla instance (e.g. 1.21.1) and press Play. Check:
- The version list loads (`mojang::manifest`).
- Libraries, assets and Java (Adoptium JDK zip) download and extract to
  `%APPDATA%\ClaudeLauncher`.
- The game launches. If it doesn't, compare the command in `launch::plan` with
  what the official launcher uses (argument rules, natives, classpath).
- Also try an old version (e.g. 1.8.9, which uses `minecraftArguments` and natives
  extraction) and a modern one.

Playing needs a signed-in account (section 5). Before that, the quickest way to
test launching is a mod project's dev client (section 4).

## 3. Loaders

- **Fabric**: `loaders::install_fabric` uses the profile JSON from Fabric meta.
  Should just work.
- **NeoForge**: `loaders::install_neoforge` runs the official installer with
  `--install-client <data dir>` (falling back to `--installClient`). Check:
  - Which flag the current installer actually accepts.
  - That it writes `versions/neoforge-<ver>/neoforge-<ver>.json` under our data dir.
  - That launching works with our classpath. We hard-link the vanilla client
    jar as `versions/neoforge-<ver>/neoforge-<ver>.jar` because NeoForge's
    `-DignoreList` refers to `${version_name}.jar`.
- NeoForge version filtering for year-based Minecraft versions (26.1 → `26.1.0.x`)
  is a guess from the version naming. Check `loaders::neoforge_prefix` against
  the real NeoForge maven list.

## 4. Mod templates and builds (most likely to need fixing)

Build tool versions are pinned per era in `crates/core/src/templates.rs`:

| Era | Gradle | Plugin |
|---|---|---|
| Fabric, MC ≤ 1.21.x | 8.14.3 | `fabric-loom` 1.11-SNAPSHOT, official mappings |
| Fabric, MC 26.x (unobfuscated) | 9.2.1 | `net.fabricmc.fabric-loom`, latest from maven, `implementation` deps, no mappings |
| NeoForge | 8.14.3 (9.2.1 for 26.x) | `net.neoforged.moddev`, latest from maven |

The 26.x Fabric setup (plugin id, no `mappings` line, `implementation` instead
of `modImplementation`) is the least certain part; check it against
https://docs.fabricmc.net/develop/. Also check:
- Gradle runs on a JDK ≥ 21 and compiles to the Minecraft version's Java target.
  The target JDK is passed via `org.gradle.java.installations.paths`.
  Confirm NeoForge's toolchain resolution picks it up.
- Old Fabric versions (1.14.4–1.16.5, Java 8) with Loom 1.11. May need an older
  Loom or Gradle pin.
- `build_mod` / `install_mod` find the jar in `build/libs/` and copy it into the
  instance. Confirm the Claude badge shows in the mod list.
- `run_dev_client` (`gradle runClient`) reaches the title screen, and
  `read_dev_log` returns its log.

Quick end-to-end test through the MCP tools: create a Fabric 1.21.1 instance,
`create_mod` with id `test_mod`, `install_mod`, `run_dev_client`. Repeat for
NeoForge 1.21.1.

When fixing a template, update both `templates.rs` and its unit tests. Projects
that already exist keep their own Gradle files and just need editing.

## 5. Microsoft sign-in

`auth.rs` implements the device code flow → Xbox Live → XSTS → Minecraft. It
needs the user's own Azure client ID, approved for the Minecraft API (see
README). Once they have one: sign in, restart the launcher, and check the token
refresh (`usable_account`) still works when the Minecraft token has expired.

## 6. Mod browser (Modrinth / CurseForge)

`crates/core/src/browse.rs`. The parsers are unit-tested against sample JSON in
the shape of each API, and a full install (main mod + required dependency,
checksum check, tags) is tested against a fake local Modrinth server. Against
the real APIs, check:
- **Modrinth**: search facets (`categories:fabric|neoforge`, `versions:<mc>`)
  return sensible results; the versions endpoint filters by loader and game
  version. Modrinth asks for a User-Agent that identifies the app with contact
  info. It's currently `ClaudeLauncher/<version>` in `http.rs`; add a GitHub URL
  or email.
- **CurseForge** (needs the user's API key): the `modLoaderType` values
  (Fabric = 4, NeoForge = 6), `sortField` values, and that files with
  `downloadUrl: null` show "Manual download" in the UI instead of failing.
- Dependency detection: a dependency already installed by hand counts as present
  when its jar's mod id matches the project slug (with `-` → `_`). This is a
  heuristic and may reinstall libraries whose slug differs from their mod id.

## 7. Sign-in token storage (`secrets.rs`, `auth.rs`)

Tokens go into Windows Credential Manager via the `keyring` crate (service
`ClaudeLauncher`, entries `<uuid>:refresh` and `<uuid>:minecraft`; split because
of Windows' 2,560-byte limit per credential). Tested with an in-memory store
only. On Windows, check that sign-in saves entries (visible in Control Panel →
Credential Manager → Windows Credentials), that `accounts.json` no longer has
tokens, and that a 0.1 `accounts.json` with tokens gets migrated.

## 8. Opening Claude Code from the launcher (`claude_code.rs`)

Windows only, never run. Check:
- `find_program` finds `claude` (native installer: `%USERPROFILE%\.local\bin\claude.exe`;
  npm: `%APPDATA%\npm\claude.cmd`) and `node`.
- **Connect automatically** runs `claude mcp remove/add --scope user` and the
  tools then show up in Claude Code.
- **Open in Claude Code** opens a PowerShell window in the projects folder with
  Claude Code running the prompt. The prompt passes through `ps_quote`, which
  turns `"` into `'` because Windows PowerShell 5.1 mangles embedded double
  quotes in native arguments. Try ideas with quotes, apostrophes, `&`, `%` and
  non-ASCII text.
- Claude Code asks the user to trust the projects folder the first time; that's expected.

## 9. Modpacks (`modpack.rs`)

The round trip (export, then import into a new instance) and the path-traversal
guard are tested locally. Still to try: importing real packs from Modrinth
(large ones, `client-overrides`, files with `env.client = "unsupported"`), and
importing an exported pack into another launcher (Prism, Modrinth App).

## 10. Polish ideas (not done)

- Mod detail pages (README/gallery) inside the browser.
- Show the game's log level colours more faithfully; filter by source in the console.
- Tray icon / minimise to tray while playing.
- Code-signing the installer (Windows SmartScreen will warn otherwise).
- Auto-updates via the Tauri updater plugin.
- Tokens in `accounts.json` are stored in plain JSON; consider the Windows
  Credential Manager (`keyring` crate).
