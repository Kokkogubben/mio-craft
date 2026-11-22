// Draws the MioCraft logo: a copper block face with a green (oxidised) M,
// as 16×16 pixel art, and writes it at the sizes the app needs. Also draws the
// whole block in 3D (M on one side, C on the other) for the website.
// The copper texture is our own (brushed streaks and a bevel), not Mojang's.
//
//   node design/tools/logo.mjs

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

// ---------- minimal PNG writer ----------
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function png(w, h, px) {
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) raw.set(px(x, y), y * (w * 4 + 1) + 1 + x * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.set([8, 6, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const hex = (h) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16), 255];
function noise(seed, x, y) {
  let n = (x * 374761393 + y * 668265263 + seed * 2147483647) | 0;
  n = Math.imul(n ^ (n >>> 13), 1274126177);
  return ((n ^ (n >>> 16)) >>> 0) / 4294967295;
}

// Copper, dark → light. These are also the launcher's theme colours.
const COPPER = ["#904931", "#9a5038", "#a75a40", "#b26247", "#c26b4c", "#c87456", "#d67b5b", "#e3826c"];
// Verdigris (oxidised copper), dark → light.
const GREEN = ["#357a64", "#3f8f75", "#4ea384", "#5fb896", "#6cc4a2", "#86d4b4"];
const SHADOW = "#24584a";

/** Brushed copper: each row has its own tone, pixels vary a little, edges are bevelled. */
function copper(x, y, seed = 0) {
  let i = 5 + Math.round((noise(1 + seed, 0, y) - 0.5) * 1.6) + Math.round((noise(2 + seed, x, y) - 0.5) * 2);
  if (y === 0 || x === 0) i = Math.max(i, 6); // lit top/left edge
  if (y === 15 || x === 15) i = Math.min(i, 2) - (x === 15 && y === 15 ? 1 : 0); // shaded bottom/right edge
  return COPPER[Math.max(0, Math.min(COPPER.length - 1, i))];
}

const M = ["XX......XX", "XXX....XXX", "XXXX..XXXX", "XX.XXXX.XX", "XX..XX..XX", "XX......XX", "XX......XX", "XX......XX"];
const C = ["..XXXXXX", ".XXXXXXX", "XX......", "XX......", "XX......", "XX......", ".XXXXXXX", "..XXXXXX"];

/**
 * A 16×16 copper face with a green letter set into it (rows of "X", top-left
 * at ox, oy), as a colour per pixel. No letter: plain copper.
 */
function face(rows, ox, oy, seed = 0, letterSeed = 11) {
  const on = (x, y) => rows?.[y - oy]?.[x - ox] === "X";
  return (x, y) => {
    if (on(x, y)) {
      // mostly mid greens; lighter along the top of a stroke, darker at its foot
      let i = 1 + Math.floor(noise(letterSeed, x, y) * 4);
      if (!on(x, y - 1)) i = Math.min(GREEN.length - 1, i + 1);
      if (!on(x, y + 1)) i = Math.max(0, i - 1);
      return GREEN[i];
    }
    if (on(x, y - 1)) return SHADOW; // the letter is set into the copper
    return copper(x, y, seed);
  };
}

/** The 16×16 logo, as a colour per pixel. */
const logo = face(M, 3, 4);

const grid = Array.from({ length: 16 }, (_, y) => Array.from({ length: 16 }, (_, x) => hex(logo(x, y))));
/** The logo scaled up with hard pixel edges. */
const scaled = (size) => png(size, size, (x, y) => grid[Math.floor((y * 16) / size)][Math.floor((x * 16) / size)]);

const out = (p, buf) => {
  fs.mkdirSync(path.dirname(path.join(root, p)), { recursive: true });
  fs.writeFileSync(path.join(root, p), buf);
  console.log("wrote", p);
};
/** A Windows .ico holding PNG images (supported since Windows Vista). */
function ico(sizes) {
  const imgs = sizes.map((s) => scaled(s));
  const head = Buffer.alloc(6 + 16 * sizes.length);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2); // icon
  head.writeUInt16LE(sizes.length, 4);
  let offset = head.length;
  sizes.forEach((s, i) => {
    const e = 6 + 16 * i;
    head[e] = s >= 256 ? 0 : s; // 0 means 256
    head[e + 1] = s >= 256 ? 0 : s;
    head.writeUInt16LE(1, e + 4); // colour planes
    head.writeUInt16LE(32, e + 6); // bits per pixel
    head.writeUInt32LE(imgs[i].length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += imgs[i].length;
  });
  return Buffer.concat([head, ...imgs]);
}

out("design/logo/logo-16.png", scaled(16));
out("design/logo/logo-1024.png", scaled(1024));
out("ui/public/icon.png", scaled(512));
// The app icons (listed in src-tauri/tauri.conf.json). Written here rather than
// with `tauri icon`, which smooths the pixels when it scales down.
out("src-tauri/icons/32x32.png", scaled(32));
out("src-tauri/icons/128x128.png", scaled(128));
out("src-tauri/icons/128x128@2x.png", scaled(256));
out("src-tauri/icons/icon.png", scaled(512));
out("src-tauri/icons/icon.ico", ico([16, 32, 48, 64, 128, 256]));

// ---------- the block in 3D, for the website ----------

/**
 * The block seen from above at an angle, like an item in the game's inventory:
 * the M face on the left, a C face on the right, plain copper on top. Each
 * texture pixel is k screen pixels wide. Transparent around the block.
 */
function block(k) {
  const h = k / 2, v = Math.round(k * 1.225);
  const W = 32 * k, H = 16 * k + 16 * v;
  // Each face: where its texture's (0, 0) corner lands, the screen step for one
  // texture pixel along x (u) and down y (v), its colours, and how lit it is.
  const faces = [
    { o: [16 * k, 0], u: [k, h], v: [-k, h], px: face(null, 0, 0, 40), light: 1 },
    { o: [0, 8 * k], u: [k, h], v: [0, v], px: face(M, 3, 4, 20, 31), light: 0.8 },
    { o: [16 * k, 16 * k], u: [k, -h], v: [0, v], px: face(C, 4, 4, 60, 23), light: 0.62 },
  ];
  return {
    W, H,
    at(x, y) {
      for (const f of faces) {
        // Screen point → texture point, by inverting the face's 2×2 matrix.
        const det = f.u[0] * f.v[1] - f.u[1] * f.v[0];
        const dx = x + 0.5 - f.o[0], dy = y + 0.5 - f.o[1];
        const s = (dx * f.v[1] - dy * f.v[0]) / det, t = (f.u[0] * dy - f.u[1] * dx) / det;
        if (s < 0 || t < 0 || s >= 16 || t >= 16) continue;
        const [r, g, b] = hex(f.px(Math.floor(s), Math.floor(t)));
        return [Math.round(r * f.light), Math.round(g * f.light), Math.round(b * f.light), 255];
      }
      return [0, 0, 0, 0];
    },
  };
}

for (const k of [4, 16]) {
  const b = block(k);
  out(`design/logo/block-${b.W}.png`, png(b.W, b.H, (x, y) => b.at(x, y)));
}
// The website's favicon: the small block, centred on a square.
{
  const b = block(4), pad = (b.H - b.W) / 2;
  out("design/logo/block-favicon.png", png(b.H, b.H, (x, y) => (x < pad || x >= pad + b.W ? [0, 0, 0, 0] : b.at(x - pad, y))));
}
