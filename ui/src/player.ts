// The home screen's 3D scene: the player sitting on a crafting table in the
// diorama (scene.ts), looking towards the cursor. Also the small head shown in
// the top bar. Skins come from the core (`/accounts/{id}/skin`), never straight
// from Mojang.

import { FunctionAnimation, SkinViewer, type PlayerObject } from "skinview3d";
import { DirectionalLight, Group, Mesh, MeshBasicMaterial, PCFSoftShadowMap, Vector3 } from "three";
import { api, apiBlob, enc } from "./api";
import { buildAtlas, buildDiorama, loadDiorama, makePetals, SUN, textureNames, type Petals, type Structure } from "./scene";

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
 * Signed in, the player sits on the crafting table in the diorama. Signed out,
 * the scene stays blank and the silhouette stands, as before the diorama.
 */
let seated = false;
let diorama: Group | null = null;

/**
 * What has to fit between the top of the screen and the Play button, in world
 * units relative to the seat: sitting, the top of the table up to the name
 * above the head; standing, the feet up to the name.
 */
const FRAMES = { seated: [-6, 36], standing: [-2, 50] } as const;

const seat = new Vector3(0, 0, 0);
let box: HTMLElement | null = null;

function setSeated(on: boolean) {
  seated = on;
  if (diorama) diorama.visible = on;
  if (petals) petals.object.visible = on;
  placeCamera();
  void loadTextures();
}

// ---------- real textures ----------

let structure: Structure | null = null;
let petals: Petals | null = null;
let textured = false;
let texturing = false;

/**
 * Swap the flat colours for the real block textures. The launcher reads them
 * from the player's own game files, so this only happens once signed in.
 */
async function loadTextures() {
  if (!structure || !seated || textured || texturing) return;
  texturing = true;
  try {
    const res = await api<{ version: string; textures: Record<string, string> }>("POST", "/scene/textures", { names: textureNames(structure) });
    const images = new Map<string, HTMLImageElement>();
    await Promise.all(
      Object.entries(res.textures).map(async ([name, png]) => {
        images.set(name.replace(/^block\//, ""), await loadImage(`data:image/png;base64,${png}`));
      }),
    );
    if (!images.size || !viewer) return;
    fadeIn(buildDiorama(structure, buildAtlas(images)).group);
    textured = true;
  } catch (e) {
    // Offline before the game files were ever downloaded: the flat colours stay.
    console.warn("Home scene textures:", e);
  } finally {
    texturing = false;
  }
}

/** Fade a new version of the diorama in over the current one, then drop the old one. */
function fadeIn(next: Group) {
  if (!viewer) return;
  const old = diorama;
  diorama = next;
  next.visible = seated;
  const mats: MeshBasicMaterial[] = [];
  // The new shadow layer takes over at the end, so shadows don't double up during the fade.
  const shadows: Mesh[] = [];
  next.traverse((o) => {
    if (!(o instanceof Mesh)) return;
    if (o.material instanceof MeshBasicMaterial) mats.push(o.material);
    else {
      o.visible = false;
      shadows.push(o);
    }
  });
  const set = (k: number) =>
    mats.forEach((m) => {
      m.opacity = k;
      // Keep see-through texture pixels cut out at every step of the fade.
      if (m.map) m.alphaTest = Math.max(0.01, 0.5 * k);
    });
  mats.forEach((m) => (m.transparent = true));
  set(0);
  viewer.scene.add(next);
  const start = performance.now();
  const step = (now: number) => {
    const k = Math.min(1, (now - start) / 450);
    set(k);
    if (k < 1) return void requestAnimationFrame(step);
    mats.forEach((m) => {
      m.transparent = false;
      m.needsUpdate = true;
    });
    shadows.forEach((m) => (m.visible = true));
    if (old && viewer) {
      viewer.scene.remove(old);
      old.traverse((o) => {
        if (o instanceof Mesh) {
          o.geometry.dispose();
          const m = o.material as MeshBasicMaterial;
          m.dispose();
          o.customDepthMaterial?.dispose();
        }
      });
    }
  };
  requestAnimationFrame(step);
}

function placeCamera() {
  if (!viewer || !box) return;
  const [FRAME_BOTTOM, FRAME_TOP] = seated ? FRAMES.seated : FRAMES.standing;
  const w = viewer.width, h = viewer.height;
  // The free space above the Play button, in canvas pixels.
  const play = document.getElementById("home-play");
  const playTop = play && play.offsetHeight ? play.getBoundingClientRect().top - box.getBoundingClientRect().top : h * 0.65;
  const top = 8, bottom = Math.max(top + 100, playTop - 8);
  const perUnit = (bottom - top) / (FRAME_TOP - FRAME_BOTTOM);

  const target = seat.clone().add(new Vector3(0, (FRAME_TOP + FRAME_BOTTOM) / 2, 0));
  const yaw = ((seated ? viewYaw : viewYaw + 25) * Math.PI) / 180;
  const pitch = ((seated ? VIEW_PITCH : 4) * Math.PI) / 180;
  const dist = h / perUnit / 2 / Math.tan((FOV * Math.PI) / 360);
  const dir = new Vector3(Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch));
  viewer.camera.position.copy(target).addScaledVector(dir, dist);
  viewer.controls.target.copy(target);
  viewer.camera.lookAt(target);
  // Shift the picture so the target sits in the middle of the free space, not the screen.
  viewer.camera.setViewOffset(w, h, 0, h / 2 - (top + bottom) / 2, w, h);
  // Sitting, the player faces the camera; standing, turned a little like before.
  viewer.playerObject.rotation.y = seated ? yaw : yaw - 0.45;
  // Sitting: hips on the table top. Standing: feet at the table top's height.
  viewer.playerObject.position.copy(seat).add(new Vector3(0, seated ? 6 : 16, 0));
}

// ---------- pose: sitting, breathing, head following the cursor ----------

const cursor = { x: 0, y: 0, active: false };
const head = { yaw: 0, pitch: 0 };
let label: HTMLElement | null = null;
const headPos = new Vector3();

/** How far the seated player leans back, in radians (negative tips the top away from the camera). */
const LEAN = -0.26;
/** Where the hips are in the skin model: the point the upper body leans around. */
const HIPS_Y = -12;

/** A point on the model's centre line at height `y`, turned by `angle` about the hips. Returns [x, y, z]. */
function aboutHips(y: number, angle: number): [number, number, number] {
  const d = y - HIPS_Y;
  return [0, HIPS_Y + d * Math.cos(angle), d * Math.sin(angle)];
}

function pose(player: PlayerObject, t: number, delta: number) {
  const s = player.skin;
  const breath = Math.sin(t * 1.6);
  if (seated) petals?.update(delta);
  if (seated) {
    // Legs forward over the table's edge, swinging a little.
    s.rightLeg.rotation.set(-1.45 + Math.sin(t * 1.1) * 0.07, 0, 0);
    s.leftLeg.rotation.set(-1.45 + Math.sin(t * 1.1 + 2.2) * 0.07, 0, 0);
    // Leaning back a little from the hips, rocking slightly with the breath.
    const lean = LEAN + breath * 0.012;
    const [, by, bz] = aboutHips(-6, lean);
    s.body.position.set(0, by, bz);
    s.body.rotation.x = lean;
    const [, hy, hz] = aboutHips(0, lean);
    s.head.position.set(0, hy, hz);
    // Arms straight down and back, hands on the table behind, a little out to the sides.
    const [, sy, sz] = aboutHips(-2, lean);
    s.rightArm.position.set(-5, sy, sz);
    s.leftArm.position.set(5, sy, sz);
    s.rightArm.rotation.set(0.27, 0, -0.08);
    s.leftArm.rotation.set(0.27, 0, 0.08);
  } else {
    // Standing still, arms swaying gently with the breath.
    s.rightLeg.rotation.set(0, 0, 0);
    s.leftLeg.rotation.set(0, 0, 0);
    s.head.position.set(0, 0, 0);
    s.body.position.set(0, -6, 0);
    s.body.rotation.x = 0;
    s.rightArm.position.set(-5, -2, 0);
    s.leftArm.position.set(5, -2, 0);
    s.rightArm.rotation.set(0, 0, 0.05 + breath * 0.03);
    s.leftArm.rotation.set(0, 0, -0.05 - breath * 0.03);
  }

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
  viewer.animation = new FunctionAnimation(pose);
  addSun(viewer);
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

  // The player shows straight away; the diorama joins as soon as it's built
  // (hidden while signed out).
  loadDiorama(SCENE)
    .then((d) => {
      if (!viewer) return;
      diorama = d.group;
      diorama.visible = seated;
      viewer.scene.add(diorama);
      structure = d.structure;
      petals = makePetals(d.structure);
      if (petals) {
        petals.object.visible = seated;
        viewer.scene.add(petals.object);
      }
      void loadTextures();
      if (d.seat) seat.copy(d.seat);
      placeCamera();
    })
    .catch((e) => console.warn("Home scene:", e));
}

/**
 * Sunlight that only casts shadows: the player and the blocks keep their own
 * lighting, and shadows fall on the scene (see the shadow layer in scene.ts).
 */
function addSun(v: SkinViewer) {
  v.renderer.shadowMap.enabled = true;
  v.renderer.shadowMap.type = PCFSoftShadowMap;
  const sun = new DirectionalLight(0xffffff, 0);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  Object.assign(sun.shadow.camera, { left: -200, right: 200, top: 200, bottom: -200, near: 1, far: 1000 });
  sun.shadow.bias = -0.0005;
  sun.shadow.normalBias = 0.5;
  sun.position.copy(SUN).multiplyScalar(450);
  v.scene.add(sun, sun.target);
  v.playerObject.traverse((o) => {
    if (o instanceof Mesh) o.castShadow = true;
  });
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
  if (seated !== !!id) setSeated(!!id);
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
