# Redesign asset blueprint

A pixel-art look for the launcher and the website: a full-window painted scene behind the home
screen, the player's own skin in the middle, chunky framed panels and buttons, and a pixel font for
headings.

## Placeholders in place now

- **UI kit and silhouette:** `design/tools/ui-kit.mjs` generates the frames in `ui/public/ui/` (copper
  theme) and `ui/public/ui/legacy/` (the original grays and green, for "Legacy colors"), plus
  `ui/public/skin/silhouette.png`. Hand-drawn files with the same names and sizes replace them; don't
  rerun the script after that, or it overwrites them.
- **Pixel font:** Monocraft (`ui/public/fonts/Monocraft.ttf`).
- **Logo:** a copper block face with an oxidised green M, 16 × 16 pixel art drawn by
  `design/tools/logo.mjs`. The script also writes `ui/public/icon.png` and every app icon in
  `src-tauri/icons/` (scaled with hard pixel edges, so don't use `tauri icon` for it).
- **Colours:** copper and verdigris from the logo, as variables at the top of `ui/src/style.css`.
  `body.legacy` holds the original palette.
- **Scene:** a flat colour, `--scene` in `ui/src/style.css`.

## Ground rules

- **Everything original.** No Mojang art (textures, the Steve/Alex skins, the Minecraft logo or font)
  and nothing traced from other launchers. Trees, blocks and terrain are fine as long as you draw them.
- **One pixel scale per layer type**, so pixels never appear in mixed sizes:
  - Scene art is drawn on a **640 × 360** canvas and shown at a whole-number zoom (2× in the default
    window, 3× at 1920 × 1080).
  - UI art (frames, buttons, icons) is drawn at 1× and shown at **2×**.
- **One palette** for everything. Pick one on [Lospec](https://lospec.com/palette-list) or make your
  own (around 32–48 colours), and save it as `design/palette.gpl` so every file uses it.
- **PNG with transparency** for everything except sounds. Don't blur or anti-alias edges; the app
  scales with nearest-neighbour.

## Source files (not shipped)

Keep the editable files here so they're versioned with the code.

```
design/
  ASSETS.md          this file
  palette.gpl        the shared palette
  logo/              logo source (.aseprite / .fig / .bbmodel) and icon-1024.png
  scene/             the scene as one layered .aseprite file
  ui/                frames, buttons and icons as .aseprite files
```

## Shipped launcher assets

Everything below goes in `ui/public/`. Files there are copied into the app as they are and are
referenced from CSS and TypeScript as `/brand/wordmark.png` and so on.

### 1. Brand — `ui/public/brand/`

| File | Size | Notes |
|---|---|---|
| `wordmark.png` | about 96 × 20 | "MioCraft" in pixel letters, shown at 2× in the top bar |
| `mark.png` | 32 × 32 | the logo on its own, for the loading screen and About |

App icons are generated, not drawn per size: `node design/tools/logo.mjs` writes every size into
`src-tauri/icons/` from the 16 × 16 logo. To use a hand-drawn logo instead, change the script to read
your 16 × 16 PNG rather than drawing one.

### 2. Home scene — `ui/public/scenes/sunset/`

The scene is split into layers so it can move slightly with the mouse (parallax). All layers are
**640 × 360**. Anything below the bottom panel is covered, so keep detail out of the bottom ~90 px.

| File | Content | Transparent? |
|---|---|---|
| `sky.png` | sky colour and sun | no |
| `clouds.png` | clouds; must tile left to right, since it drifts slowly | yes |
| `far.png` | distant hills or mountains | yes |
| `mid.png` | trees and terrain at middle distance | yes |
| `near.png` | foreground blocks and plants at the left and right edges | yes |

Leave the centre (roughly x 220–420, y 60–260) calm. The player model and the Play button sit there.

Later, extra scenes in sibling folders (`scenes/day/`, `scenes/night/`) can follow the clock or be
picked in Settings.

### 3. Ambient sprites — `ui/public/sprites/`

| File | Size | Notes |
|---|---|---|
| `particles.png` | 8 × 8 frames in a row, 4 frames per kind | things that drift through the scene: fireflies, leaves, butterflies. One row per kind |

### 4. UI kit — `ui/public/ui/`

Frames and buttons are **9-slice** images: the corners stay fixed and the edges stretch, so one
small image fits any size. Draw each at 1× with **4 px corners**.

| File | Size | Used for |
|---|---|---|
| `panel.png` | 12 × 12 | dark panels (instance row on the home screen, dialogs, mod list) |
| `button.png`, `button-hover.png`, `button-down.png` | 12 × 12 each | normal buttons |
| `play.png`, `play-hover.png`, `play-down.png` | 16 × 16 each | the big Play button (accent colour) |
| `tab.png`, `tab-active.png` | 12 × 12 each | top navigation tabs |
| `input.png` | 12 × 12 | text fields and dropdowns |
| `loading.png` | 16 × 16 frames, 8 in a row | loading spinner |

### 5. Icons — `ui/public/icons/`

One sheet, **`icons.png`, on a 16 × 16 grid**, shown at 2×. The order doesn't matter: once it exists,
the cell for each name gets written into the code.

Needed: home, instances, mods, claude, settings, play, stop, folder, edit, export, download,
console, list, search, close, minimize, maximize, wardrobe, add, refresh, warning, check.

Loader names (Fabric, Forge and so on) stay as text badges. Their logos belong to those projects.

### 6. Instance icons — `ui/public/instance-icons/`

About 12 icons at **32 × 32** that players can choose for an instance (a chest, pickaxe, sword,
sapling, rocket and so on), named by what they show: `chest.png`, `pickaxe.png`, …

Instance cards on the home screen show the instance's newest in-game screenshot. If it has none,
they use **`card-fallback.png`** (160 × 90, a small scene-style image) from the same folder.

### 7. Player model — `ui/public/skin/`

The signed-in player's real skin is shown as a 3D model (with the MIT-licensed skinview3d library), so
no skin art is needed. For signed-out users only:

| File | Size | Notes |
|---|---|---|
| `silhouette.png` | about 64 × 128 | a shadowy player shape with a "Sign in" feel. Not the Steve skin |

### 8. Fonts — `ui/public/fonts/`

Shipped with the app, so nothing is loaded from Google at runtime.

| File | Use | Candidates (all free under the OFL) |
|---|---|---|
| `pixel.woff2` | headings, buttons, tabs, the Play button | Pixelify Sans, Silkscreen, Monocraft |
| `body.woff2` | mod descriptions, settings, logs: anything longer than a few words | Inter, Geist |

Small text in a pixel font is hard to read, so long text stays in the body font.

### 9. Sounds (optional) — `ui/public/sfx/`

`click.ogg`, `hover.ogg`, `launch.ogg`: short, quiet, and off by default with a switch in Settings.
[jsfxr](https://sfxr.me) makes these in a minute.

## Website — `miocraft-site/assets/`

| File | Size | Notes |
|---|---|---|
| `scene/*.png` | same files as the launcher | hero background, same layers |
| `wordmark.png` | as above | header |
| `og.png` | 1200 × 630 | link preview image (wordmark over the scene) |
| `favicon.png` | 64 × 64 | from the logo |
| screenshots | real window captures | retaken once the new look is in |

## Home screen layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ [wordmark] Home Instances Mods Claude          [head] Name  ⚙  _ □ × │ top bar: tab.png, icons.png
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│   near.png            player model (skin / silhouette.png)  near.png│ scenes/sunset/*
│                                                                      │ sprites/particles.png
│                  [ ▶  PLAY  —  Survival 1.21.1 ] [≡]                │ play.png, button.png
│                                                                      │
│ ┌ Your instances ──────────────────────────────────────── View all ┐ │ panel.png
│ │ [screenshot] Survival   [screenshot] PvP 1.8.9   [fallback] New…  │ │ card-fallback.png,
│ └──────────────────────────────────────────────────────────────────┘ │ instance-icons/*
└──────────────────────────────────────────────────────────────────────┘
```

The list button next to Play switches which instance Play starts. The other pages (instance details,
mod browser, Claude projects, settings) keep the scene blurred and darkened behind framed panels.

## Order to make them in

1. Palette and fonts: they decide how everything else looks.
2. Logo and wordmark.
3. Scene layers: the biggest single piece, and what the look is built around.
4. UI kit and icons.
5. Instance icons, particles, silhouette, sounds.

Each group can be wired in as soon as it exists; nothing has to wait for the whole set.
