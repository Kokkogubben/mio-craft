// The home screen's diorama: a Minecraft structure (.mcstructure, saved with a
// structure block in Bedrock Edition) drawn in flat colours, like the logo.
// The file only holds block names and positions, so no game art ships with
// the launcher. 1 block = 16 units, the same scale as the player model.

import { BufferAttribute, BufferGeometry, Color, DoubleSide, Group, Mesh, MeshBasicMaterial, Vector3 } from "three";

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
const CROSS = new Set(["short_grass", "tall_grass", "fern", "dandelion", "poppy", "azure_bluet", "oxeye_daisy", "cornflower", "sweet_berry_bush", "deadbush"]);
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

// ---------- geometry ----------

// Each face: its normal, four corners (unit cube), and a fixed shade like Minecraft's.
const FACES = [
  { dir: [0, 1, 0], shade: 1.0, corners: [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]] },
  { dir: [0, -1, 0], shade: 0.5, corners: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]] },
  { dir: [0, 0, 1], shade: 0.8, corners: [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]] },
  { dir: [0, 0, -1], shade: 0.8, corners: [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]] },
  { dir: [1, 0, 0], shade: 0.65, corners: [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]] },
  { dir: [-1, 0, 0], shade: 0.65, corners: [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]] },
] as const;

class Builder {
  pos: number[] = [];
  col: number[] = [];
  private c = new Color();
  /** A quad from four corners (counter-clockwise seen from the front). */
  quad(v: number[][], colour: string, shade: number) {
    this.c.set(colour).multiplyScalar(shade);
    for (const i of [0, 1, 2, 0, 2, 3]) {
      this.pos.push(v[i][0], v[i][1], v[i][2]);
      this.col.push(this.c.r, this.c.g, this.c.b);
    }
  }
  mesh(doubleSided = false): Mesh {
    const g = new BufferGeometry();
    g.setAttribute("position", new BufferAttribute(new Float32Array(this.pos), 3));
    g.setAttribute("color", new BufferAttribute(new Float32Array(this.col), 3));
    g.computeBoundingSphere();
    return new Mesh(g, new MeshBasicMaterial({ vertexColors: true, side: doubleSided ? DoubleSide : undefined }));
  }
}

/** Small brightness changes per block, so large areas still read as blocks. */
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
};

/** Build the diorama, centred on x/z with its bottom at y = 0. */
export function buildDiorama(s: Structure): Diorama {
  const [X, Y, Z] = s.size;
  const at = (x: number, y: number, z: number) =>
    x < 0 || y < 0 || z < 0 || x >= X || y >= Y || z >= Z ? -1 : s.indices[x * Y * Z + y * Z + z];
  const kind = s.palette.map((b) => (SKIP.has(b.name) ? "none" : CROSS.has(b.name) ? "cross" : FLAT.has(b.name) ? "flat" : "cube"));
  const solid = (x: number, y: number, z: number) => {
    const i = at(x, y, z);
    return i >= 0 && kind[i] === "cube";
  };

  const cubes = new Builder();
  const plants = new Builder();
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
          const h = 13;
          const v = (a: number[], b: number[]) => [
            [bx + a[0], by, bz + a[1]], [bx + b[0], by, bz + b[1]],
            [bx + b[0], by + h, bz + b[1]], [bx + a[0], by + h, bz + a[1]],
          ];
          plants.quad(v([2, 2], [14, 14]), top, 0.9 * j);
          plants.quad(v([2, 14], [14, 2]), top, 0.8 * j);
          continue;
        }
        if (kind[i] === "flat") {
          // Petals and the like: a thin layer on the ground, more of it as it grows.
          const parts = 1 + Number(block.states.growth ?? 3);
          const quads = [[1, 1], [8, 1], [8, 8], [1, 8]].slice(0, Math.min(4, parts));
          for (const [qx, qz] of quads) {
            const yy = by + 1.5;
            plants.quad([[bx + qx, yy, bz + qz + 7], [bx + qx + 7, yy, bz + qz + 7], [bx + qx + 7, yy, bz + qz], [bx + qx, yy, bz + qz]], top, j);
          }
          continue;
        }

        // Pillars (logs) lying on their side show their ends sideways.
        const axis = String(block.states.pillar_axis ?? "y");
        for (const f of FACES) {
          if (solid(x + f.dir[0], y + f.dir[1], z + f.dir[2])) continue;
          const end = axis === "y" ? f.dir[1] !== 0 : axis === "x" ? f.dir[0] !== 0 : f.dir[2] !== 0;
          const colour = block.name.endsWith("_log") ? (end ? top : side) : f.dir[1] > 0 ? top : f.dir[1] < 0 ? bottom : side;
          const v = f.corners.map((c) => [bx + c[0] * 16, by + c[1] * 16, bz + c[2] * 16]);
          if (block.name === "grass_block" && f.dir[1] === 0) {
            // The logo's grass block: a green band along the top of each side.
            const cut = by + 13;
            const lower = v.map((p) => [p[0], p[1] > by ? cut : p[1], p[2]]);
            const upper = v.map((p) => [p[0], p[1] > by ? p[1] : cut, p[2]]);
            cubes.quad(lower, side, f.shade * j);
            cubes.quad(upper, GRASS_BAND, f.shade * j);
            continue;
          }
          cubes.quad(v, colour, f.shade * j);
        }
      }

  const group = new Group();
  group.add(cubes.mesh(), plants.mesh(true));
  return { group, seat, size: new Vector3(X * 16, Y * 16, Z * 16) };
}

/** Fetch and build the home diorama. */
export async function loadDiorama(url: string): Promise<Diorama> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Couldn't load the scene (${res.status})`);
  return buildDiorama(await parseMcstructure(await res.arrayBuffer()));
}
