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
/** One set of frames from a theme's colours. */
const kit = (c) => ({
  // neutral buttons (the app's surfaces)
  "button.png": { outline: O, fill: hex(c.btn[0]), hi: hex(c.btn[1]), lo: hex(c.btn[2]) },
  "button-hover.png": { outline: O, fill: hex(c.btnHover[0]), hi: hex(c.btnHover[1]), lo: hex(c.btnHover[2]) },
  "button-down.png": { outline: O, fill: hex(c.btnDown[0]), hi: hex(c.btn[1]), lo: hex(c.btnDown[2]), pressed: true },
  // the Play / primary button (the theme's accent)
  "play.png": { outline: O, fill: hex(c.play[0]), hi: hex(c.play[1]), lo: hex(c.play[2]) },
  "play-hover.png": { outline: O, fill: hex(c.playHover[0]), hi: hex(c.playHover[1]), lo: hex(c.playHover[2]) },
  "play-down.png": { outline: O, fill: hex(c.playDown[0]), hi: hex(c.play[1]), lo: hex(c.playDown[2]), pressed: true },
  // red Stop / destructive button
  "danger.png": { outline: O, fill: hex("#c4463f"), hi: hex("#e0695f"), lo: hex("#8c2e29") },
  "danger-hover.png": { outline: O, fill: hex("#d2514a"), hi: hex("#ec7a70"), lo: hex("#99342e") },
  "danger-down.png": { outline: O, fill: hex("#b33e38"), hi: hex("#e0695f"), lo: hex("#7a2723"), pressed: true },
  // panels, cards and fields
  "panel.png": { outline: O, fill: hex(c.panel[0], 240), hi: hex(c.panel[1], 240), lo: hex(c.panel[2], 240), loW: 1 },
  "card.png": { outline: O, fill: hex(c.card[0]), hi: hex(c.card[1]), lo: hex(c.card[2]) },
  "card-hover.png": { outline: O, fill: hex(c.cardHover[0]), hi: hex(c.cardHover[1]), lo: hex(c.cardHover[2]) },
  "card-active.png": { outline: hex(c.accent), fill: hex(c.cardHover[0]), hi: hex(c.cardHover[1]), lo: hex(c.cardHover[2]) },
  "tab-active.png": { outline: O, fill: hex(c.cardHover[0]), hi: hex(c.cardHover[1]), lo: hex(c.accent) },
  "input.png": { outline: O, fill: hex(c.input[0]), hi: hex(c.input[1]), lo: hex(c.input[2]), hiW: 1, loW: 1, pressed: true },
});
// [fill, highlight, shadow] for each frame
const themes = {
  // copper, from the logo (the default)
  "": {
    accent: "#d67b5b",
    btn: ["#30271f", "#463a31", "#1e1814"],
    btnHover: ["#3a2f27", "#53453a", "#241d18"],
    btnDown: ["#29211c", "", "#17120f"],
    play: ["#c26b4c", "#e3826c", "#904931"],
    playHover: ["#d67b5b", "#eb9a80", "#9a5038"],
    playDown: ["#b26247", "", "#7a3e29"],
    panel: ["#1c1714", "#3b302a", "#120e0c"],
    card: ["#231d1a", "#352c26", "#181310"],
    cardHover: ["#2b2420", "#3f342d", "#1c1714"],
    input: ["#140f0d", "#2e2621", "#090706"],
  },
  // the original grays and grass green ("Legacy colors")
  "legacy/": {
    accent: "#6abe50",
    btn: ["#2a2d34", "#3d414b", "#1b1d21"],
    btnHover: ["#343843", "#4b505c", "#212328"],
    btnDown: ["#24262c", "", "#16171b"],
    play: ["#5aa843", "#82cc67", "#3d7a2c"],
    playHover: ["#67b84f", "#93d978", "#448633"],
    playDown: ["#4f9a3a", "", "#336a25"],
    panel: ["#1a1b1f", "#2e3138", "#111214"],
    card: ["#202227", "#30333a", "#16171a"],
    cardHover: ["#272a30", "#393c45", "#1a1b1f"],
    input: ["#121316", "#2a2d33", "#08090a"],
  },
};
for (const [dir, colours] of Object.entries(themes)) {
  for (const [name, spec] of Object.entries(kit(colours))) write(`ui/${dir}${name}`, frame(spec));
}

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
