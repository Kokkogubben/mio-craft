// Instance icons: built-in pixel glyphs (8×8, drawn for MioCraft), tile colours,
// and turning a picture into a small square PNG.

const ART: Record<string, string[]> = {
  pickaxe: ["........", ".#####..", ".....##.", "....#.##", "...#...#", "..#....#", ".#......", "#......."],
  sword: ["......##", ".....###", "....###.", ".#.###..", "..###...", "..##....", ".#..#...", "#......."],
  tree: ["...##...", "..####..", ".######.", "..####..", ".######.", "########", "...##...", "...##..."],
  heart: ["........", ".##..##.", "########", "########", ".######.", "..####..", "...##...", "........"],
  star: ["...##...", "...##...", "########", ".######.", "..####..", ".######.", ".##..##.", "##....##"],
  gem: ["...##...", "..####..", ".######.", "########", "########", ".######.", "..####..", "...##..."],
  flame: ["....#...", "...##...", "..###.#.", "..#####.", ".###.##.", ".##..##.", ".##..##.", "..####.."],
  house: ["...##...", "..####..", ".######.", "########", ".######.", ".##..##.", ".##..##.", ".##..##."],
  potion: ["..####..", "...##...", "...##...", "..####..", ".######.", "########", "########", ".######."],
  moon: ["...##...", ".###....", ".##.....", "###.....", "###.....", ".##.....", ".###....", "...##..."],
  bolt: ["....###.", "...###..", "..###...", ".######.", "...###..", "..###...", ".###....", ".##....."],
};

export const GLYPHS = Object.keys(ART);

/** Tile colours offered in the picker (the loader's colour is the default). */
export const ICON_COLORS = ["#c8734f", "#d9b23f", "#5fa64a", "#3fa6a0", "#4f86c8", "#8f5fc8", "#d0679b", "#c84f4f", "#7d7a76"];

/** An 8×8 glyph as crisp SVG, one rect per horizontal run of pixels. */
export function glyphSvg(name: string, size = 18): string {
  const rows = ART[name];
  if (!rows) return "";
  let d = "";
  rows.forEach((row, y) => {
    for (let x = 0; x < row.length; ) {
      if (row[x] !== "#") {
        x++;
        continue;
      }
      let w = 0;
      while (row[x + w] === "#") w++;
      d += `M${x} ${y}h${w}v1h-${w}z`;
      x += w;
    }
  });
  return `<svg class="glyph" width="${size}" height="${size}" viewBox="0 0 8 8" shape-rendering="crispEdges" fill="currentColor" aria-hidden="true"><path d="${d}"/></svg>`;
}

/** Dark or light ink, whichever reads better on a tile colour. */
export function inkFor(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#1b1411" : "#fff";
}

/** A picture file as a 64×64 PNG data URL, centre-cropped. Small pictures (pixel art) stay sharp. */
export async function pictureToIcon(file: File): Promise<string> {
  const url = URL.createObjectURL(file);
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const i = new Image();
      i.onload = () => resolve(i);
      i.onerror = () => reject(new Error("That file isn't a picture MioCraft can read"));
      i.src = url;
    });
    const side = Math.min(img.naturalWidth, img.naturalHeight);
    const c = document.createElement("canvas");
    c.width = c.height = 64;
    const ctx = c.getContext("2d")!;
    ctx.imageSmoothingEnabled = side > 64;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(img, (img.naturalWidth - side) / 2, (img.naturalHeight - side) / 2, side, side, 0, 0, 64, 64);
    return c.toDataURL("image/png");
  } finally {
    URL.revokeObjectURL(url);
  }
}
