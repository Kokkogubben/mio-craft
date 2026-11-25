// Makes a released version the stable one: copies that release's latest.json
// into the `stable` release, which copies of MioCraft with "Stable versions
// only" on read for updates. Run by the "Promote to stable" workflow; see STABLE.md.
//
//   VERSION=0.9.0 REPO=owner/repo GH_TOKEN=... node .github/scripts/promote-stable.mjs
import { execFileSync } from "node:child_process";
import fs from "node:fs";

const version = (process.env.VERSION || "").replace(/^v/, "").trim();
const repo = process.env.REPO;
const allowOlder = process.env.ALLOW_OLDER === "true";
if (!/^\d+\.\d+\.\d+$/.test(version) || !repo) throw new Error("set VERSION (e.g. 0.9.0) and REPO (owner/name)");

const gh = (...args) => execFileSync("gh", [...args, "--repo", repo], { encoding: "utf8" });
const tryGh = (...args) => {
  try {
    return gh(...args);
  } catch {
    return null;
  }
};
const newer = (a, b) => {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i];
  return false;
};

// The manifest the release build wrote for this version.
fs.mkdirSync("out", { recursive: true });
gh("release", "download", `v${version}`, "--pattern", "latest.json", "--dir", "out", "--clobber");
const manifest = JSON.parse(fs.readFileSync("out/latest.json", "utf8"));
if (manifest.version !== version) throw new Error(`v${version}'s latest.json is for ${manifest.version}`);

// Never go backwards by accident: a copy that updated to stable would keep the newer one anyway.
const current = tryGh("release", "view", "stable", "--json", "name", "--jq", ".name");
const was = current?.match(/(\d+\.\d+\.\d+)/)?.[1];
if (was && newer(was, version) && !allowOlder) {
  throw new Error(`stable is already ${was}, newer than ${version}. Re-run with allow_older to go back.`);
}

manifest.notes = `MioCraft ${version} is the stable version. See https://github.com/${repo}/releases/tag/v${version}`;
fs.writeFileSync("out/latest.json", JSON.stringify(manifest, null, 2) + "\n");

const title = `Stable: MioCraft ${version}`;
const notes = [
  `**The stable version of MioCraft is ${version}.** Download it from [the ${version} release](https://github.com/${repo}/releases/tag/v${version}).`,
  "",
  "This release only holds the update file for the stable channel (Settings → About → **Stable versions only**).",
  "A version becomes stable after it has been out a while without known problems; see [STABLE.md](https://github.com/" + repo + "/blob/main/STABLE.md).",
  "The newest version is always the release marked Latest.",
].join("\n");

if (current === null) {
  gh("release", "create", "stable", "out/latest.json", "--title", title, "--notes", notes, "--latest=false", "--target", "main");
} else {
  gh("release", "upload", "stable", "out/latest.json", "--clobber");
  gh("release", "edit", "stable", "--title", title, "--notes", notes);
}
console.log(`stable is now ${version}${was ? ` (was ${was})` : ""}`);
