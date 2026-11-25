# Stable versions

MioCraft has two update channels:

- **Latest** (the default): every release, as soon as it's out. It's the GitHub release marked *Latest*.
- **Stable**: only versions that have been out a while without known problems. Turn it on in
  Settings → About → **Stable versions only**. Copies on it read the update file in the
  [`stable` release](https://github.com/Kokkogubben/mio-craft/releases/tag/stable).

A copy that is already on a newer version than stable keeps it until stable passes it; it never
goes back.

## When a version becomes stable

A released version is made stable when **all** of these hold:

1. **It has been out for at least 3 days.**
2. **It has been used for real**: a game was started with it, and updating to it worked.
3. **No serious known bug**: nothing that stops playing, installing instances or mods, signing in or
   updating, and nothing that loses data (worlds, mods, settings, accounts). Small visual problems
   don't block it.
4. **Its build passed CI**, and it has a signed installer and `latest.json` in its release.

The newest version that qualifies is promoted; skipping versions is fine. If a version has a serious
bug, it is fixed in a new release (for example 0.9.1), and that release starts its own 3 days.

Who checks: Claude checks this list whenever it releases a new version, and whenever a version is
reported working or broken. When a version qualifies, Claude promotes it without asking and updates
the table below.

To promote by hand:

```
gh workflow run stable.yml -f version=0.9.0
```

## Versions

| Version | Released | Status | Notes |
|---|---|---|---|
| 0.9.0 | 2026-11-25 | Latest, not yet stable | Waits for the rules above. Not yet tried in a real game: window size and fullscreen, the command before launch, playtime, minimizing while playing, building a modpack with Claude, installing resource packs from the browser. |
| 0.8.0 | 2026-11-23 | **Stable** | First stable version. |
