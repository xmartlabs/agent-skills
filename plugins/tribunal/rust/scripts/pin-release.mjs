#!/usr/bin/env node
// pin-release.mjs: pins a published `tribunal-rust-v*` release in
// rust/release.json, the file skills/review-pr/scripts/rust-engine.mjs
// trusts. It reads the release's own .sha256 assets, so run it after the
// release workflow has finished, and commit release.json with the plugin
// version that should start using that binary.
//
//   node rust/scripts/pin-release.mjs <tag> [--repo owner/name] [--write]
//
// Without --write it prints the manifest it would write. It refuses a
// release that is missing any of the four platforms.

import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-musl",
  "x86_64-pc-windows-msvc",
];

// `shasum -a 256` lines, "<hex>  <file>", into the manifest's assets.
export function manifestFrom(tag, repo, shaLines) {
  const assets = {};
  for (const line of shaLines) {
    const m = /^([0-9a-f]{64})\s+\*?(\S+)$/.exec(line.trim());
    if (!m) continue;
    const target = TARGETS.find((t) => m[2].endsWith(`-${t}.tar.gz`) || m[2].endsWith(`-${t}.zip`));
    if (target) assets[target] = { name: m[2], sha256: m[1] };
  }
  const missing = TARGETS.filter((t) => !assets[t]);
  if (missing.length) throw new Error(`${tag} has no sha256 for ${missing.join(", ")}`);
  return { tag, repo, assets };
}

function main(argv) {
  const tag = argv.find((a) => !a.startsWith("--") && argv[argv.indexOf(a) - 1] !== "--repo");
  const repo = argv.includes("--repo") ? argv[argv.indexOf("--repo") + 1] : "xmartlabs/agent-skills";
  if (!tag || !/^tribunal-rust-v\d+\.\d+\.\d+$/.test(tag)) {
    console.error("usage: pin-release.mjs tribunal-rust-v<x.y.z> [--repo owner/name] [--write]");
    return 2;
  }
  const dir = mkdtempSync(join(tmpdir(), "pin-release-"));
  try {
    const gh = spawnSync(
      process.env.TRIBUNAL_GH || "gh",
      ["release", "download", tag, "--repo", repo, "--pattern", "*.sha256", "--dir", dir],
      { encoding: "utf8" },
    );
    if (gh.error || gh.status !== 0) {
      console.error(`gh release download ${tag} failed: ${gh.error?.message || gh.stderr.trim()}`);
      return 1;
    }
    const lines = readdirSync(dir).flatMap((f) => readFileSync(join(dir, f), "utf8").split("\n"));
    const manifest = manifestFrom(tag, repo, lines);
    const text = `${JSON.stringify(manifest, null, 2)}\n`;
    if (argv.includes("--write")) {
      const out = join(dirname(fileURLToPath(import.meta.url)), "..", "release.json");
      writeFileSync(out, text);
      console.log(`wrote ${out}`);
    } else {
      process.stdout.write(text);
    }
    return 0;
  } catch (e) {
    console.error(e.message);
    return 1;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const isMain = () => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
};
if (process.argv[1] && isMain()) {
  process.exit(main(process.argv.slice(2)));
}
