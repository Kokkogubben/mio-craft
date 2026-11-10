# MioCraft privacy policy

MioCraft is a desktop application that runs entirely on your computer. It has no
telemetry, analytics, advertising or accounts of its own, and it sends nothing
to its author. There is no MioCraft server.

## What MioCraft contacts

MioCraft only talks to the services it needs to do what you ask:

| Service | When | What is sent |
|---|---|---|
| Mojang (`piston-meta.mojang.com`, `resources.download.minecraft.net`, `libraries.minecraft.net`) | Listing versions, installing an instance | Download requests |
| Fabric, NeoForge, Adoptium, JetBrains, Gradle | Installing a mod loader, Java or build tools | Download requests |
| Modrinth | Using the mod browser, checking for mod updates, importing modpacks | Your search text, the instance's Minecraft version and loader, and which browser-installed mods to check for updates. Imported modpacks may also download files from GitHub or GitLab |
| CurseForge | Only if you add a CurseForge API key | The same, plus your API key |
| Microsoft, Xbox Live and Minecraft services | Signing in and refreshing your session | The tokens Microsoft's sign-in flow requires |
| `mc-heads.net` | Showing your player head once you are signed in | Your Minecraft account UUID |

Every request identifies the app as `MioCraft/<version>` with a link to this
repository. Each of these services has its own privacy policy that applies to
the requests it receives, including your IP address.

## Signing in

You sign in in your browser on Microsoft's own sign-in page, using Microsoft's
OAuth 2.0 device code flow. MioCraft never sees, asks for or stores your
Microsoft password. It receives sign-in tokens from Microsoft, which it uses
only to sign you in to Xbox Live and Minecraft services and to start the game.

## What is stored on your computer

- **Sign-in tokens** are stored in Windows Credential Manager. Removing the
  account in MioCraft deletes them.
- **Your Minecraft profile name and UUID** are stored in `accounts.json` in the
  data folder (`%APPDATA%\MioCraft`).
- **Settings**, including the Azure client ID and CurseForge API key you enter,
  are stored in `config.json` in the data folder.
- **Game files, instances, mods, logs and mod projects** are stored in the data
  folder. Your Minecraft access token is removed from the game output the
  launcher records.

Uninstalling MioCraft and deleting the data folder removes all of it.

## Local API

While MioCraft is open it serves an API for its own window and for Claude Code.
It listens only on `127.0.0.1` (your own computer) and requires a random token
that changes every time the launcher starts. It never returns sign-in tokens.

## Claude Code

Claude Code is a separate program that you install and run yourself. When you
use it with MioCraft, what it sends to Anthropic is covered by Anthropic's own
terms and privacy policy, not by this one.

## Contact

Questions or concerns: open an issue at
<https://github.com/Kokkogubben/mio-craft/issues>.
