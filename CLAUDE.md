# MioCraft

Minecraft launcher (Tauri 2 + TypeScript) with an MCP server so Claude Code can build mods.

- `crates/core/` — all launcher logic in Rust: installs, Fabric/NeoForge, launching, Microsoft auth,
  mod list, mod projects and templates, and the local HTTP API (`api.rs`). Test with `cargo test -p launcher-core`.
- `src-tauri/` — thin desktop shell that starts the core and serves the UI.
- `ui/` — vanilla TypeScript + Vite UI; talks only to the core's local API.
- `mcp/` — MCP server for Claude Code; a thin client of the local API.
- `crates/core/live/` — Java for live mode: the hotswap agent and the companion mod (Fabric 26.x),
  embedded into the launcher and built on demand. Calls marked `VERIFY(26.x)` were never compiled.

Start with NOTES-FOR-CLAUDE-CODE.md: it lists what was never run on Windows and needs verifying.
Keep logic in the core, not in the UI or the shell, so the UI and the MCP server stay in sync.
