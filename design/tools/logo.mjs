// Draws the MioCraft logo: a copper block face with a green (oxidised) M,
// as 16×16 pixel art, and writes it at the sizes the app needs.
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
function copper(x, y) {
  let i = 5 + Math.round((noise(1, 0, y) - 0.5) * 1.6) + Math.round((noise(2, x, y) - 0.5) * 2);
  if (y === 0 || x === 0) i = Math.max(i, 6); // lit top/left edge
  if (y === 15 || x === 15) i = Math.min(i, 2) - (x === 15 && y === 15 ? 1 : 0); // shaded bottom/right edge
  return COPPER[Math.max(0, Math.min(COPPER.length - 1, i))];
}

const M = ["XX......XX", "XXX....XXX", "XXXX..XXXX", "XX.XXXX.XX", "XX..XX..XX", "XX......XX", "XX......XX", "XX......XX"];
const MX = 3, MY = 4;
const onM = (x, y) => M[y - MY]?.[x - MX] === "X";

/** The 16×16 logo, as a colour per pixel. */
function logo(x, y) {
  if (onM(x, y)) {
    // mostly mid greens; lighter along the top of a stroke, darker at its foot
    let i = 1 + Math.floor(noise(11, x, y) * 4);
    if (!onM(x, y - 1)) i = Math.min(GREEN.length - 1, i + 1);
    if (!onM(x, y + 1)) i = Math.max(0, i - 1);
    return GREEN[i];
  }
  if (onM(x, y - 1)) return SHADOW; // the M is set into the copper
  return copper(x, y);
}

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
