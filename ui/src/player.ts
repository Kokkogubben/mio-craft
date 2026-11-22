// The home screen's 3D scene: the player sitting on a crafting table in the
// diorama (scene.ts), looking towards the cursor. Also the small head shown in
// the top bar. Skins come from the core (`/accounts/{id}/skin`), never straight
// from Mojang.

import { FunctionAnimation, SkinViewer, type PlayerObject } from "skinview3d";
import { Vector3 } from "three";
import { apiBlob, enc } from "./api";
import { loadDiorama } from "./scene";

const SILHOUETTE = "/skin/silhouette.png";
const SCENE = "/scene/home.mcstructure";

let viewer: SkinViewer | null = null;
/** Account whose skin is shown ("" = the signed-out silhouette). */
let shown: string | null = null;
const heads = new Map<string, string>();

/** The head of an account whose skin has been loaded, as a data URL. */
export const cachedHead = (id: string): string | null => heads.get(id) ?? null;

// ---------- camera ----------

/** Which way the camera looks at the player, in degrees around the vertical axis. */
let viewYaw = 225;
/** How far above the player the camera is, in degrees. */
const VIEW_PITCH = 12;
const FOV = 30;
/**
 * What has to fit between the top of the screen and the Play button, in world
 * units relative to the seat: the top of the table up to the name above the head.
 */
const FRAME_BOTTOM = -6, FRAME_TOP = 36;

const seat = new Vector3(0, 0, 0);
let box: HTMLElement | null = null;

function placeCamera() {
  if (!viewer || !box) return;
  const w = viewer.width, h = viewer.height;
  // The free space above the Play button, in canvas pixels.
  const play = document.getElementById("home-play");
  const playTop = play && play.offsetHeight ? play.getBoundingClientRect().top - box.getBoundingClientRect().top : h * 0.65;
  const top = 8, bottom = Math.max(top + 100, playTop - 8);
  const perUnit = (bottom - top) / (FRAME_TOP - FRAME_BOTTOM);

  const target = seat.clone().add(new Vector3(0, (FRAME_TOP + FRAME_BOTTOM) / 2, 0));
  const yaw = (viewYaw * Math.PI) / 180, pitch = (VIEW_PITCH * Math.PI) / 180;
  const dist = h / perUnit / 2 / Math.tan((FOV * Math.PI) / 360);
  const dir = new Vector3(Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch));
  viewer.camera.position.copy(target).addScaledVector(dir, dist);
  viewer.controls.target.copy(target);
  viewer.camera.lookAt(target);
  // Shift the picture so the target sits in the middle of the free space, not the screen.
  viewer.camera.setViewOffset(w, h, 0, h / 2 - (top + bottom) / 2, w, h);
  // The player faces the camera.
  viewer.playerObject.rotation.y = yaw;
  viewer.playerObject.position.copy(seat).add(new Vector3(0, 6, 0)); // hips on the table top
}

// ---------- pose: sitting, breathing, head following the cursor ----------

const cursor = { x: 0, y: 0, active: false };
const head = { yaw: 0, pitch: 0 };
let label: HTMLElement | null = null;
const headPos = new Vector3();

function sitting(player: PlayerObject, t: number, delta: number) {
  const s = player.skin;
  // Legs forward over the table's edge, swinging a little.
  s.rightLeg.rotation.set(-1.45 + Math.sin(t * 1.1) * 0.07, 0.12, 0);
  s.leftLeg.rotation.set(-1.45 + Math.sin(t * 1.1 + 2.2) * 0.07, -0.12, 0);
  // Hands resting on the knees, rising and falling with the breath.
  const breath = Math.sin(t * 1.6);
  s.rightArm.rotation.set(-0.55 + breath * 0.025, 0, 0.06);
  s.leftArm.rotation.set(-0.55 + breath * 0.025, 0, -0.06);
  s.body.rotation.x = 0.04 + breath * 0.012;

  // Look towards the cursor, eased and limited to a natural range.
  let ty = 0, tp = 0;
  if (cursor.active && viewer) {
    s.head.getWorldPosition(headPos);
    const p = headPos.project(viewer.camera);
    const w = viewer.width, h = viewer.height;
    const hx = (p.x * 0.5 + 0.5) * w, hy = (-p.y * 0.5 + 0.5) * h;
    ty = Math.max(-0.9, Math.min(0.9, ((cursor.x - hx) / (w * 0.5)) * 1.1));
    tp = Math.max(-0.5, Math.min(0.45, ((cursor.y - hy) / (h * 0.5)) * 0.8));
  }
  const k = Math.min(1, delta * 5);
  head.yaw += (ty - head.yaw) * k;
  head.pitch += (tp - head.pitch) * k;
  s.head.rotation.set(head.pitch, head.yaw, 0);

  // Keep the name above the player's head.
  if (label && viewer) {
    s.head.getWorldPosition(headPos);
    headPos.y += 13;
    const p = headPos.project(viewer.camera);
    label.style.transform = `translate(${(p.x * 0.5 + 0.5) * viewer.width}px, ${(-p.y * 0.5 + 0.5) * viewer.height}px) translate(-50%, -100%)`;
  }
}

// ---------- setup ----------

export function initPlayer(canvas: HTMLCanvasElement, nameLabel: HTMLElement) {
  box = canvas.parentElement!;
  const home = box;
  label = nameLabel;
  // For trying camera angles in development: ?sceneYaw=40
  const y = Number(new URLSearchParams(location.search).get("sceneYaw") ?? NaN);
  if (Number.isFinite(y)) viewYaw = y;
  viewer = new SkinViewer({ canvas, width: home.clientWidth || 1, height: home.clientHeight || 1, skin: SILHOUETTE, fov: FOV });
  viewer.controls.enableRotate = false;
  viewer.controls.enableZoom = false;
  viewer.controls.enablePan = false;
  viewer.controls.maxDistance = 5000;
  viewer.animation = new FunctionAnimation(sitting);
  placeCamera();
  shown = "";

  // Reframe when the window, the Play button or the instance list changes size.
  const resize = new ResizeObserver(() => {
    if (!viewer || home.clientWidth === 0) return;
    if (viewer.width !== home.clientWidth || viewer.height !== home.clientHeight) viewer.setSize(home.clientWidth, home.clientHeight);
    placeCamera();
  });
  for (const id of ["home-play", "home-recent"]) {
    const el = document.getElementById(id);
    if (el) resize.observe(el);
  }
  resize.observe(home);
  window.addEventListener("pointermove", (e) => {
    const r = home.getBoundingClientRect();
    cursor.x = e.clientX - r.left;
    cursor.y = e.clientY - r.top;
    cursor.active = true;
  });
  document.documentElement.addEventListener("pointerleave", () => (cursor.active = false));

  // The player shows straight away; the diorama joins as soon as it's built.
  loadDiorama(SCENE)
    .then((d) => {
      if (!viewer) return;
      viewer.scene.add(d.group);
      if (d.seat) seat.copy(d.seat);
      placeCamera();
    })
    .catch((e) => console.warn("Home scene:", e));
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
