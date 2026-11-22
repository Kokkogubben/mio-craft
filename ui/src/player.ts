// The 3D player on the home screen, and the small head shown in the top bar.
// Skins come from the core (`/accounts/{id}/skin`), never straight from Mojang.

import { IdleAnimation, SkinViewer } from "skinview3d";
import { apiBlob, enc } from "./api";

const SILHOUETTE = "/skin/silhouette.png";

let viewer: SkinViewer | null = null;
/** Account whose skin is shown ("" = the signed-out silhouette). */
let shown: string | null = null;
const heads = new Map<string, string>();

/** The head of an account whose skin has been loaded, as a data URL. */
export const cachedHead = (id: string): string | null => heads.get(id) ?? null;

function size(): [number, number] {
  const h = Math.round(Math.min(420, Math.max(220, window.innerHeight * 0.42)));
  return [Math.round(h * 0.62), h];
}

export function initPlayer(canvas: HTMLCanvasElement) {
  const [width, height] = size();
  viewer = new SkinViewer({ canvas, width, height, skin: SILHOUETTE, zoom: 0.9, fov: 40 });
  viewer.animation = new IdleAnimation();
  viewer.controls.enableZoom = false;
  viewer.controls.enablePan = false;
  // A slight turn reads as 3D at a glance.
  viewer.playerObject.rotation.y = -0.45;
  shown = "";
  window.addEventListener("resize", () => viewer?.setSize(...size()));
}

/** Stop drawing frames while the home screen is hidden. */
export function pausePlayer(paused: boolean) {
  if (viewer) viewer.renderPaused = paused;
}

/**
 * Show the account's skin (or the silhouette for null). Returns a data URL of
 * the player's head for the top bar, or null when there is no skin to show.
 */
export async function showAccount(id: string | null): Promise<string | null> {
  if (!id) {
    if (viewer && shown !== "") viewer.loadSkin(SILHOUETTE, { model: "default" });
    shown = "";
    return null;
  }
  if (shown === id) return heads.get(id) ?? null;
  try {
    const { blob, headers } = await apiBlob(`/accounts/${enc(id)}/skin`);
    const url = URL.createObjectURL(blob);
    const img = await loadImage(url);
    const model = headers.get("x-skin-model") === "slim" ? "slim" : "default";
    if (viewer) viewer.loadSkin(img, { model });
    shown = id;
    const head = headOf(img);
    heads.set(id, head);
    URL.revokeObjectURL(url);
    return head;
  } catch {
    // Offline with nothing cached, or the profile has no skin: keep what's shown.
    return null;
  }
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((ok, fail) => {
    const img = new Image();
    img.onload = () => ok(img);
    img.onerror = () => fail(new Error("Couldn't read the skin image"));
    img.src = src;
  });
}

/** The face (and hat layer) of a skin, as a 32×32 data URL. */
function headOf(img: HTMLImageElement): string {
  const c = document.createElement("canvas");
  c.width = c.height = 32;
  const g = c.getContext("2d")!;
  g.imageSmoothingEnabled = false;
  g.drawImage(img, 8, 8, 8, 8, 0, 0, 32, 32);
  g.drawImage(img, 40, 8, 8, 8, 0, 0, 32, 32);
  return c.toDataURL();
}
