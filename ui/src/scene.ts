// The home screen's diorama: a Minecraft structure (.mcstructure, saved with a
// structure block in Bedrock Edition). It's first drawn in flat colours, like
// the logo; once the player has signed in, the launcher reads the real block
// textures from their own copy of the game and the scene switches to those.
// The file only holds block names and positions, so no game art ships with the
// launcher. 1 block = 16 units, the same scale as the player model.

import {
  BufferAttribute, BufferGeometry, CanvasTexture, Color, DoubleSide, FrontSide, Group, Mesh,
  MeshBasicMaterial, NearestFilter, Texture, Vector3,
} from "three";

export type Block = { name: string; states: Record<string, unknown> };
export type Structure = { size: [number, number, number]; palette: Block[]; indices: Int32Array };

// ---------- reading .mcstructure (little-endian NBT) ----------

/** Read a .mcstructure file. Newer files keep their data zlib-compressed inside. */
export async function parseMcstructure(buf: ArrayBuffer): Promise<Structure> {
  let root = readNbt(new Uint8Array(buf));
  let data = root.structure;
  if (data instanceof Uint8Array) data = readNbt(await inflate(data));
  const size = root.size as [number, number, number];
  const palette = (data.palette.default.block_palette as any[]).map((b) => ({
    name: String(b.name).replace(/^minecraft:/, ""),
    states: b.states ?? {},
  }));
  return { size, palette, indices: Int32Array.from(data.block_indices[0] as number[]) };
}

async function inflate(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as Uint8Array<ArrayBuffer>]).stream().pipeThrough(new DecompressionStream("deflate"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function readNbt(b: Uint8Array): any {
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  let p = 0;
  const str = () => {
    const n = dv.getUint16(p, true);
    p += 2;
    const s = new TextDecoder().decode(b.subarray(p, p + n));
    p += n;
    return s;
  };
  const payload = (t: number): any => {
    switch (t) {
      case 1: return dv.getInt8(p++);
      case 2: p += 2; return dv.getInt16(p - 2, true);
      case 3: p += 4; return dv.getInt32(p - 4, true);
      case 4: p += 8; return Number(dv.getBigInt64(p - 8, true));
      case 5: p += 4; return dv.getFloat32(p - 4, true);
      case 6: p += 8; return dv.getFloat64(p - 8, true);
      case 7: { const n = dv.getInt32(p, true); p += 4; p += n; return b.slice(p - n, p); }
      case 8: return str();
      case 9: { const et = b[p++]; const n = dv.getInt32(p, true); p += 4; return Array.from({ length: n }, () => payload(et)); }
      case 10: {
        const o: Record<string, any> = {};
        for (;;) {
          const tt = b[p++];
          if (tt === 0) return o;
          o[str()] = payload(tt);
        }
      }
      case 11: { const n = dv.getInt32(p, true); p += 4; return Array.from({ length: n }, () => payload(3)); }
      case 12: { const n = dv.getInt32(p, true); p += 4; return Array.from({ length: n }, () => payload(4)); }
      default: throw new Error(`Unknown NBT tag ${t}`);
    }
  };
  const t = b[p++];
  str();
  return payload(t);
}

// ---------- flat colours ----------

/** [top, side, bottom] colours of a block; one value means all faces. */
type Colours = string | [string, string, string];
const COLOURS: Record<string, Colours> = {
  grass_block: ["#6abe50", "#7a563a", "#7a563a"], // sides get a green band too (see below)
  dirt: "#7a563a",
  coarse_dirt: "#6f4f36",
  stone: "#85817d",
  cobblestone: "#77736f",
  coal_ore: "#6f6b68",
  iron_ore: "#8f8379",
  copper_ore: "#8a7b6c",
  sand: "#dccf9f",
  gravel: "#86807b",
  oak_log: ["#a5814f", "#6b4a30", "#a5814f"],
  birch_log: ["#c9b77f", "#d8d3c6", "#c9b77f"],
  spruce_log: ["#7a5a37", "#3f2d1c", "#7a5a37"],
  cherry_log: ["#d29a99", "#3e2730", "#d29a99"],
  oak_leaves: "#3f8f3c",
  birch_leaves: "#6aa84a",
  cherry_leaves: "#eba8c8",
  oak_planks: "#a5814f",
  cherry_planks: "#e2b2ad",
  crafting_table: ["#b1834f", "#8a6238", "#8a6238"],
  short_grass: "#5aa840",
  tall_grass: "#5aa840",
  fern: "#4e9638",
  pink_petals: "#f3b4d1",
  water: "#3f76e4",
};
const GRASS_BAND = "#56a040";
/** Blocks that are not full cubes and aren't drawn as one. */
const CROSS = new Set(["short_grass", "tall_grass", "fern", "large_fern", "dandelion", "poppy", "azure_bluet", "oxeye_daisy", "cornflower", "sweet_berry_bush", "deadbush"]);
const FLAT = new Set(["pink_petals", "leaf_litter", "wildflowers"]);
const SKIP = new Set(["air", "structure_block", "structure_void", "barrier", "light_block"]);

/** A stable colour for blocks without one, so unknown blocks still show. */
function fallbackColour(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) | 0;
  return `hsl(${Math.abs(h) % 360} 25% 55%)`;
}

function coloursOf(name: string): [string, string, string] {
  const c = COLOURS[name] ?? fallbackColour(name);
  return typeof c === "string" ? [c, c, c] : c;
}

// ---------- real textures (from the player's own game files) ----------

/** The scene's biome tints: a cherry grove. Grass and most leaves are grey in the game files. */
const GRASS_TINT = "#b6db61";
const FOLIAGE_TINT = "#b6db61";
const UNTINTED_LEAVES = new Set(["cherry_leaves", "azalea_leaves", "flowering_azalea_leaves", "pale_oak_leaves"]);

/** Built from two textures: the grass side with its tinted overlay on top. */
const GRASS_SIDE = "@grass_side";
/** A plain white cell, for faces that stay flat colour in a textured scene. */
const WHITE = "@white";

type FaceName = "up" | "down" | "north" | "south" | "east" | "west";
const FACE_NAMES: FaceName[] = ["up", "down", "north", "south", "east", "west"];

/** Texture names to try for one face of a block, best first (without "block/"). */
function candidates(b: Block, face: FaceName): string[] {
  const n = b.name;
  const side = face !== "up" && face !== "down";
  if (n === "grass_block") return face === "up" ? ["grass_block_top"] : face === "down" ? ["dirt"] : [GRASS_SIDE];
  if (n === "crafting_table") {
    if (face === "up") return ["crafting_table_top"];
    if (face === "down") return ["oak_planks"];
    return face === "north" || face === "west" ? ["crafting_table_front"] : ["crafting_table_side"];
  }
  if (/_(log|stem)$/.test(n)) {
    const axis = String(b.states.pillar_axis ?? "y");
    const end = axis === "y" ? !side : axis === "x" ? face === "east" || face === "west" : face === "north" || face === "south";
    return end ? [`${n}_top`] : [n];
  }
  if (n === "short_grass") return ["short_grass", "grass"]; // renamed in 1.20.3
  if (n === "tall_grass" || n === "large_fern") return [b.states.upper_block_bit ? `${n}_top` : `${n}_bottom`];
  if (face === "up") return [`${n}_top`, n];
  if (face === "down") return [`${n}_bottom`, `${n}_top`, n];
  return [`${n}_side`, n];
}

function tintOf(texture: string): string | null {
  if (/^(grass_block_top|grass_block_side_overlay|short_grass|grass|fern|tall_grass_.*|large_fern_.*)$/.test(texture)) return GRASS_TINT;
  if (texture.endsWith("_leaves") && !UNTINTED_LEAVES.has(texture)) return FOLIAGE_TINT;
  return null;
}

/** Every texture the structure could use, to ask the launcher for ("block/dirt", …). */
export function textureNames(s: Structure): string[] {
  const used = new Set(s.indices);
  const out = new Set<string>();
  s.palette.forEach((b, i) => {
    if (SKIP.has(b.name) || !used.has(i)) return;
    for (const f of FACE_NAMES)
      for (const c of candidates(b, f)) {
        if (c === GRASS_SIDE) out.add("grass_block_side").add("grass_block_side_overlay");
        else out.add(c);
      }
  });
  return [...out].map((n) => `block/${n}`);
}

type Img = CanvasImageSource & { width: number };

/** Textures packed into one image; `slots` maps a texture name to its [u0, v0, u1, v1]. */
export type Atlas = { texture: CanvasTexture; slots: Map<string, [number, number, number, number]> };

/** Pack textures (name → image, without "block/") into one atlas, tinted where the game tints them. */
export function buildAtlas(images: Map<string, Img>): Atlas {
  const names = [...images.keys(), WHITE];
  if (images.has("grass_block_side") && images.has("grass_block_side_overlay")) names.push(GRASS_SIDE);
  const cols = Math.max(1, Math.ceil(Math.sqrt(names.length)));
  const rows = Math.max(1, Math.ceil(names.length / cols));
  const canvas = document.createElement("canvas");
  canvas.width = cols * 16;
  canvas.height = rows * 16;
  const g = canvas.getContext("2d")!;
  g.imageSmoothingEnabled = false;
  const slots: Atlas["slots"] = new Map();
  const inset = 0.01; // in pixels, so neighbouring textures never bleed in
  names.forEach((name, i) => {
    const x = (i % cols) * 16, y = Math.floor(i / cols) * 16;
    if (name === WHITE) {
      g.fillStyle = "#fff";
      g.fillRect(x, y, 16, 16);
    } else if (name === GRASS_SIDE) {
      draw(g, images.get("grass_block_side")!, x, y, null);
      draw(g, images.get("grass_block_side_overlay")!, x, y, GRASS_TINT);
    } else {
      draw(g, images.get(name)!, x, y, tintOf(name));
    }
    slots.set(name, [
      (x + inset) / canvas.width,
      1 - (y + 16 - inset) / canvas.height,
      (x + 16 - inset) / canvas.width,
      1 - (y + inset) / canvas.height,
    ]);
  });
  const texture = new CanvasTexture(canvas);
  texture.magFilter = NearestFilter;
  texture.minFilter = NearestFilter;
  texture.generateMipmaps = false;
  return { texture, slots };
}

/** Draw the first 16 × 16 frame of a texture (animated ones are a tall strip), multiplied by a tint. */
function draw(g: CanvasRenderingContext2D, img: Img, x: number, y: number, tint: string | null) {
  const scratch = document.createElement("canvas");
  scratch.width = scratch.height = 16;
  const s = scratch.getContext("2d", { willReadFrequently: true })!;
  s.imageSmoothingEnabled = false;
  s.drawImage(img, 0, 0, img.width, img.width, 0, 0, 16, 16);
  if (tint) {
    const c = new Color(tint);
    const px = s.getImageData(0, 0, 16, 16);
    for (let i = 0; i < px.data.length; i += 4) {
      px.data[i] *= c.r;
      px.data[i + 1] *= c.g;
      px.data[i + 2] *= c.b;
    }
    s.putImageData(px, 0, 0);
  }
  g.drawImage(scratch, x, y);
}

// ---------- geometry ----------

// Each face: its name, normal, four corners (unit cube; bottom-left, bottom-right,
// top-right, top-left seen from outside), and a fixed shade like the game's.
const FACES = [
  { name: "up", dir: [0, 1, 0], shade: 1.0, corners: [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]] },
  { name: "down", dir: [0, -1, 0], shade: 0.5, corners: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]] },
  { name: "south", dir: [0, 0, 1], shade: 0.8, corners: [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]] },
  { name: "north", dir: [0, 0, -1], shade: 0.8, corners: [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]] },
  { name: "east", dir: [1, 0, 0], shade: 0.6, corners: [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]] },
  { name: "west", dir: [-1, 0, 0], shade: 0.6, corners: [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]] },
] as const;

/** Texture corners matching a quad's four corners. */
const UV = [[0, 0], [1, 0], [1, 1], [0, 1]];
/** The same, turned a quarter, for logs lying on their side. */
const UV_TURNED = [[1, 0], [1, 1], [0, 1], [0, 0]];

type Slot = readonly [number, number, number, number];

class Builder {
  pos: number[] = [];
  col: number[] = [];
  uv: number[] = [];
  private c = new Color();
  /** Where flat-colour quads sample the atlas: the middle of its white cell. */
  private white: [number, number];
  constructor(atlas?: Atlas) {
    const w = atlas?.slots.get(WHITE);
    this.white = w ? [(w[0] + w[2]) / 2, (w[1] + w[3]) / 2] : [0, 0];
  }
  /**
   * A quad from four corners. With a texture slot it's textured (the shade
   * darkens it); without, it's drawn in the flat colour.
   */
  quad(v: number[][], colour: string, shade: number, slot?: Slot, uv: number[][] = UV) {
    this.c.set(slot ? "#ffffff" : colour).multiplyScalar(shade);
    for (const i of [0, 1, 2, 0, 2, 3]) {
      this.pos.push(v[i][0], v[i][1], v[i][2]);
      this.col.push(this.c.r, this.c.g, this.c.b);
      if (slot) this.uv.push(slot[0] + uv[i][0] * (slot[2] - slot[0]), slot[1] + uv[i][1] * (slot[3] - slot[1]));
      else this.uv.push(this.white[0], this.white[1]);
    }
  }
  mesh(map: Texture | null, doubleSided = false): Mesh {
    const g = new BufferGeometry();
    g.setAttribute("position", new BufferAttribute(new Float32Array(this.pos), 3));
    g.setAttribute("color", new BufferAttribute(new Float32Array(this.col), 3));
    g.setAttribute("uv", new BufferAttribute(new Float32Array(this.uv), 2));
    g.computeBoundingSphere();
    const m = new MeshBasicMaterial({ vertexColors: true, map, alphaTest: map ? 0.5 : 0, side: doubleSided ? DoubleSide : FrontSide });
    return new Mesh(g, m);
  }
}

/** Small brightness changes per block, so large flat areas still read as blocks. */
function jitter(x: number, y: number, z: number): number {
  let n = (x * 73856093) ^ (y * 19349663) ^ (z * 83492791);
  n = Math.imul(n ^ (n >>> 13), 1274126177);
  return 0.96 + (((n ^ (n >>> 16)) >>> 0) / 4294967295) * 0.08;
}

export type Diorama = {
  group: Group;
  /** Where the player sits: the top centre of the crafting table, in scene units. */
  seat: Vector3 | null;
  /** The structure's size in scene units. */
  size: Vector3;
  /** The parsed structure, to build it again with textures. */
  structure: Structure;
};

/**
 * Build the diorama, centred on x/z with its bottom at y = 0. With an atlas,
 * faces it has textures for are textured; everything else stays flat colour.
 */
export function buildDiorama(s: Structure, atlas?: Atlas): Diorama {
  const [X, Y, Z] = s.size;
  const at = (x: number, y: number, z: number) =>
    x < 0 || y < 0 || z < 0 || x >= X || y >= Y || z >= Z ? -1 : s.indices[x * Y * Z + y * Z + z];
  const kind = s.palette.map((b) => (SKIP.has(b.name) ? "none" : CROSS.has(b.name) ? "cross" : FLAT.has(b.name) ? "flat" : "cube"));
  // Faces next to an opaque cube are hidden. Leaves can be seen through once textured.
  const opaque = s.palette.map((b, i) => kind[i] === "cube" && !b.name.endsWith("_leaves"));
  const hides = (x: number, y: number, z: number) => {
    const i = at(x, y, z);
    return i >= 0 && opaque[i];
  };
  const slotFor = (b: Block, face: FaceName): Slot | undefined => {
    if (!atlas) return undefined;
    for (const c of candidates(b, face)) {
      const slot = atlas.slots.get(c);
      if (slot) return slot;
    }
    return undefined;
  };

  const cubes = new Builder(atlas);
  const plants = new Builder(atlas);
  let seat: Vector3 | null = null;
  const ox = -X / 2, oz = -Z / 2;

  for (let x = 0; x < X; x++)
    for (let y = 0; y < Y; y++)
      for (let z = 0; z < Z; z++) {
        const i = at(x, y, z);
        if (i < 0 || kind[i] === "none") continue;
        const block = s.palette[i];
        const [top, side, bottom] = coloursOf(block.name);
        const j = jitter(x, y, z);
        const bx = (x + ox) * 16, by = y * 16, bz = (z + oz) * 16;
        if (block.name === "crafting_table") seat ??= new Vector3(bx + 8, by + 16, bz + 8);

        if (kind[i] === "cross") {
          const slot = slotFor(block, "north");
          const [lo, hi, h] = slot ? [0.8, 15.2, 16] : [2, 14, 13];
          const v = (a: number[], b: number[]) => [
            [bx + a[0], by, bz + a[1]], [bx + b[0], by, bz + b[1]],
            [bx + b[0], by + h, bz + b[1]], [bx + a[0], by + h, bz + a[1]],
          ];
          plants.quad(v([lo, lo], [hi, hi]), top, (slot ? 1 : 0.9) * j, slot);
          plants.quad(v([lo, hi], [hi, lo]), top, (slot ? 1 : 0.8) * j, slot);
          continue;
        }
        if (kind[i] === "flat") {
          // Petals and the like: a thin layer on the ground, more of it as it grows.
          const slot = slotFor(block, "up");
          const parts = Math.min(4, 1 + Number(block.states.growth ?? 3));
          const yy = by + 2;
          if (slot) {
            // Textured: one quarter of the texture per part.
            for (const [qx, qz] of [[0, 0], [0, 8], [8, 8], [8, 0]].slice(0, parts)) {
              const u0 = qx / 16, u1 = (qx + 8) / 16, v0 = 1 - (qz + 8) / 16, v1 = 1 - qz / 16;
              plants.quad(
                [[bx + qx, yy, bz + qz + 8], [bx + qx + 8, yy, bz + qz + 8], [bx + qx + 8, yy, bz + qz], [bx + qx, yy, bz + qz]],
                top, j, slot, [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
              );
            }
          } else {
            for (const [qx, qz] of [[1, 1], [8, 1], [8, 8], [1, 8]].slice(0, parts))
              plants.quad([[bx + qx, yy, bz + qz + 7], [bx + qx + 7, yy, bz + qz + 7], [bx + qx + 7, yy, bz + qz], [bx + qx, yy, bz + qz]], top, j);
          }
          continue;
        }

        const axis = String(block.states.pillar_axis ?? "y");
        const isLog = /_(log|stem)$/.test(block.name);
        for (const f of FACES) {
          const nx = x + f.dir[0], ny = y + f.dir[1], nz = z + f.dir[2];
          if (hides(nx, ny, nz)) continue;
          // Leaves hide the faces of the same leaves beside them, like the game's fast leaves.
          if (!opaque[i] && at(nx, ny, nz) === i) continue;
          const v = f.corners.map((c) => [bx + c[0] * 16, by + c[1] * 16, bz + c[2] * 16]);
          const slot = slotFor(block, f.name);
          if (slot) {
            // Bark runs along a log, so turn it on logs lying on their side.
            const turned = isLog && ((axis === "x" && f.dir[0] === 0) || (axis === "z" && f.dir[0] !== 0));
            cubes.quad(v, "", f.shade, slot, turned ? UV_TURNED : UV);
            continue;
          }
          const end = axis === "y" ? f.dir[1] !== 0 : axis === "x" ? f.dir[0] !== 0 : f.dir[2] !== 0;
          const colour = isLog ? (end ? top : side) : f.dir[1] > 0 ? top : f.dir[1] < 0 ? bottom : side;
          if (block.name === "grass_block" && f.dir[1] === 0) {
            // The logo's grass block: a green band along the top of each side.
            const cut = by + 13;
            cubes.quad(v.map((p) => [p[0], p[1] > by ? cut : p[1], p[2]]), side, f.shade * j);
            cubes.quad(v.map((p) => [p[0], p[1] > by ? p[1] : cut, p[2]]), GRASS_BAND, f.shade * j);
            continue;
          }
          cubes.quad(v, colour, f.shade * j);
        }
      }

  const map = atlas?.texture ?? null;
  const group = new Group();
  group.add(cubes.mesh(map), plants.mesh(map, true));
  return { group, seat, size: new Vector3(X * 16, Y * 16, Z * 16), structure: s };
}

/** Fetch and build the home diorama in flat colours. */
export async function loadDiorama(url: string): Promise<Diorama> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Couldn't load the scene (${res.status})`);
  return buildDiorama(await parseMcstructure(await res.arrayBuffer()));
}
