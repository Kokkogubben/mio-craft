// Writes target/release/bundle/latest.json, the file installed copies of
// MioCraft read to find updates (see plugins.updater.endpoints in
// src-tauri/tauri.conf.json). Run by the Build workflow on version tags.
//
//   TAG=v0.2.0 REPO=owner/repo node .github/scripts/update-manifest.mjs
import fs from "node:fs";
import path from "node:path";

const tag = process.env.TAG;
const repo = process.env.REPO;
if (!tag || !repo) throw new Error("set TAG (e.g. v0.2.0) and REPO (owner/name)");

const version = tag.replace(/^v/, "");
const conf = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8"));
if (conf.version !== version) {
  // An installer that reports an older version than its tag would offer itself as an update forever.
  throw new Error(`tag ${tag} doesn't match the app version ${conf.version} in src-tauri/tauri.conf.json`);
}

const dir = "target/release/bundle/nsis";
const installers = fs.readdirSync(dir).filter((f) => f.endsWith("-setup.exe"));
if (installers.length !== 1) throw new Error(`expected one installer in ${dir}, found: ${installers.join(", ") || "none"}`);
const exe = installers[0];
const sig = path.join(dir, `${exe}.sig`);
if (!fs.existsSync(sig)) throw new Error(`${sig} is missing: was the build signed (TAURI_SIGNING_PRIVATE_KEY)?`);

const manifest = {
  version,
  notes: `See https://github.com/${repo}/releases/tag/${tag}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature: fs.readFileSync(sig, "utf8").trim(),
      url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(exe)}`,
    },
  },
};
const out = "target/release/bundle/latest.json";
fs.writeFileSync(out, JSON.stringify(manifest, null, 2) + "\n");
console.log(`wrote ${out} for ${exe}`);
