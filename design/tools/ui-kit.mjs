// Generates the placeholder pixel UI kit (9-slice frames) and the signed-out
// player skin into ui/public/. Replace any of these PNGs with hand-drawn art
// of the same size; this script only exists so there is something to start from.
//
//   node design/tools/ui-kit.mjs

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const out = (p) => path.join(root, "ui/public", p);

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
    raw[y * (w * 4 + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const [r, g, b, a] = px(x, y);
      raw.set([r, g, b, a], y * (w * 4 + 1) + 1 + x * 4);
    }
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
const hex = (h, a = 255) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16), a];
const CLEAR = [0, 0, 0, 0];
function write(p, buf) {
  fs.mkdirSync(path.dirname(out(p)), { recursive: true });
  fs.writeFileSync(out(p), buf);
  console.log("wrote", p);
}

// ---------- 9-slice frames: 12×12, 4 px corners ----------
// outline, then a highlight along the top/left and a shadow along the
// bottom/right (swapped for pressed states), then the fill.

function frame({ outline, fill, hi, lo, hiW = 1, loW = 2, pressed = false }) {
  const S = 12;
  return png(S, S, (x, y) => {
    const edge = x === 0 || y === 0 || x === S - 1 || y === S - 1;
    const corner = (x === 0 || x === S - 1) && (y === 0 || y === S - 1);
    if (corner) return CLEAR;
    if (edge) return outline;
    const [tl, br, tlW, brW] = pressed ? [lo, hi, loW, hiW] : [hi, lo, hiW, loW];
    if (y >= S - 1 - brW || x >= S - 1 - brW) return br;
    if (y <= tlW || x <= tlW) return tl;
    return fill;
  });
}

const O = hex("#0b0c0e");
const kit = {
  // neutral buttons (the app's gray surfaces)
  "button.png": { outline: O, fill: hex("#2a2d34"), hi: hex("#3d414b"), lo: hex("#1b1d21") },
  "button-hover.png": { outline: O, fill: hex("#343843"), hi: hex("#4b505c"), lo: hex("#212328") },
  "button-down.png": { outline: O, fill: hex("#24262c"), hi: hex("#3d414b"), lo: hex("#16171b"), pressed: true },
  // the green Play / primary button (the app's grass green)
  "play.png": { outline: O, fill: hex("#5aa843"), hi: hex("#82cc67"), lo: hex("#3d7a2c") },
  "play-hover.png": { outline: O, fill: hex("#67b84f"), hi: hex("#93d978"), lo: hex("#448633") },
  "play-down.png": { outline: O, fill: hex("#4f9a3a"), hi: hex("#82cc67"), lo: hex("#336a25"), pressed: true },
  // red Stop / destructive button
  "danger.png": { outline: O, fill: hex("#c4463f"), hi: hex("#e0695f"), lo: hex("#8c2e29") },
  "danger-hover.png": { outline: O, fill: hex("#d2514a"), hi: hex("#ec7a70"), lo: hex("#99342e") },
  "danger-down.png": { outline: O, fill: hex("#b33e38"), hi: hex("#e0695f"), lo: hex("#7a2723"), pressed: true },
  // panels, cards and fields
  "panel.png": { outline: O, fill: hex("#1a1b1f", 240), hi: hex("#2e3138", 240), lo: hex("#111214", 240), loW: 1 },
  "card.png": { outline: O, fill: hex("#202227"), hi: hex("#30333a"), lo: hex("#16171a") },
  "card-hover.png": { outline: O, fill: hex("#272a30"), hi: hex("#393c45"), lo: hex("#1a1b1f") },
  "card-active.png": { outline: hex("#6abe50"), fill: hex("#272a30"), hi: hex("#393c45"), lo: hex("#1a1b1f") },
  "tab-active.png": { outline: O, fill: hex("#272a30"), hi: hex("#393c45"), lo: hex("#6abe50") },
  "input.png": { outline: O, fill: hex("#121316"), hi: hex("#2a2d33"), lo: hex("#08090a"), hiW: 1, loW: 1, pressed: true },
};
for (const [name, spec] of Object.entries(kit)) write(`ui/${name}`, frame(spec));

// ---------- signed-out player: a plain dark figure (64×64 skin layout) ----------

const faces = [
  // [x, y, w, h] of every base-layer face in the 64×64 skin layout
  ...box(0, 0, 8, 8, 8), // head
  ...box(16, 16, 8, 12, 4), // body
  ...box(40, 16, 4, 12, 4), // right arm
  ...box(0, 16, 4, 12, 4), // right leg
  ...box(32, 48, 4, 12, 4), // left arm
  ...box(16, 48, 4, 12, 4), // left leg
];
/** The six faces of a box with width w, height h and depth d whose texture starts at (u, v). */
function box(u, v, w, h, d) {
  return [
    [u + d, v, w, d], // top
    [u + d + w, v, w, d], // bottom
    [u, v + d, d, h], // right side
    [u + d, v + d, w, h], // front
    [u + d + w, v + d, d, h], // left side
    [u + d + w + d, v + d, w, h], // back
  ];
}
const body = hex("#2a2d34");
const rim = hex("#363a43");
write(
  "skin/silhouette.png",
  png(64, 64, (x, y) => {
    for (const [fx, fy, fw, fh] of faces) {
      if (x >= fx && x < fx + fw && y >= fy && y < fy + fh) {
        const onEdge = x === fx || y === fy || x === fx + fw - 1 || y === fy + fh - 1;
        return onEdge ? rim : body;
      }
    }
    return CLEAR;
  }),
);
