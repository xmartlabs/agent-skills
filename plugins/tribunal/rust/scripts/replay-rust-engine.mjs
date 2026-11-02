#!/usr/bin/env node
// replay-rust-engine.mjs: the launcher behind `--engine rust` runs only the
// binary its pin names. A stand-in `gh` serves a stand-in release, so this
// costs 0 tokens and no network. Tier 0.
//
// Usage: node rust/scripts/replay-rust-engine.mjs

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PLUGIN_ROOT, resolveBinary, targetFor } from "../../skills/review-pr/scripts/rust-engine.mjs";
import { manifestFrom, TARGETS } from "./pin-release.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const LAUNCHER = join(HERE, "..", "..", "skills", "review-pr", "scripts", "rust-engine.mjs");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

if (process.platform === "win32") {
  console.log("replay-rust-engine: the stand-in gh is a shell script; skipped on Windows");
  process.exit(0);
}

console.log("\n--- which build a machine gets ---");
check("Apple silicon gets aarch64-apple-darwin", targetFor("darwin", "arm64") === "aarch64-apple-darwin");
check("an Intel Mac gets x86_64-apple-darwin", targetFor("darwin", "x64") === "x86_64-apple-darwin");
check("Linux x64 gets the static musl build", targetFor("linux", "x64") === "x86_64-unknown-linux-musl");
check("Windows x64 and Windows on ARM get the msvc build", targetFor("win32", "x64") === "x86_64-pc-windows-msvc" && targetFor("win32", "arm64") === "x86_64-pc-windows-msvc");
check("Linux arm64 has no build", targetFor("linux", "arm64") === null);
check("the plugin root is three levels above scripts/", existsSync(join(PLUGIN_ROOT, "skills", "review-pr", "SKILL.md")));

const work = mkdtempSync(join(tmpdir(), "rust-engine-"));
const target = targetFor(process.platform, process.arch);
const base = `tribunal-v0.0.1-${target}`;
const assetName = `${base}.tar.gz`;
const archive = join(work, "release", assetName);
mkdirSync(join(work, "release", base), { recursive: true });
const fakeBin = join(work, "release", base, "tribunal");
writeFileSync(fakeBin, '#!/bin/sh\necho "args:$*"\necho "root:$TRIBUNAL_PLUGIN_ROOT"\nexit 7\n');
chmodSync(fakeBin, 0o755);
spawnSync("tar", ["-czf", archive, "-C", join(work, "release"), base]);
const sha = createHash("sha256").update(readFileSync(archive)).digest("hex");

const ghLog = join(work, "gh.log");
const gh = join(work, "gh");
writeFileSync(
  gh,
  `#!/bin/sh
echo "$*" >> "${ghLog}"
[ -n "$GH_FAIL" ] && { echo "HTTP 404: release not found" >&2; exit 1; }
while [ $# -gt 0 ]; do [ "$1" = "--dir" ] && dir="$2"; shift; done
cp "${archive}" "$dir/"
`,
);
chmodSync(gh, 0o755);
const ghCalls = () => (existsSync(ghLog) ? readFileSync(ghLog, "utf8").trim().split("\n").length : 0);

let n = 0;
const setup = (manifest) => {
  n += 1;
  const m = join(work, `release-${n}.json`);
  writeFileSync(m, JSON.stringify(manifest));
  return { ...process.env, TRIBUNAL_RUST_MANIFEST: m, TRIBUNAL_DATA_DIR: join(work, `data-${n}`), TRIBUNAL_GH: gh, TRIBUNAL_BIN: "" };
};
const pinned = (s = sha) => ({ tag: "tribunal-rust-v0.0.1", repo: "o/r", assets: { [target]: { name: assetName, sha256: s } } });

console.log("\n--- resolve ---");
{
  const r = resolveBinary({ env: setup({ tag: null, repo: "o/r", assets: {} }) });
  check("no pinned release is a reasoned refusal, not a crash", !r.ok && /no release is pinned/.test(r.error), JSON.stringify(r));
}
{
  const env = setup(pinned());
  const first = resolveBinary({ env });
  check("the first run downloads, verifies and unpacks", first.ok && first.source === "download" && existsSync(first.bin), JSON.stringify(first));
  check("the unpacked binary is executable", first.ok && (statSync(first.bin).mode & 0o111) !== 0);
  check("it asked gh for exactly the pinned asset of the pinned tag", /release download tribunal-rust-v0\.0\.1 --repo o\/r --pattern /.test(readFileSync(ghLog, "utf8")) && readFileSync(ghLog, "utf8").includes(assetName));
  const calls = ghCalls();
  const second = resolveBinary({ env });
  check("the second run reuses it without gh", second.ok && second.source === "cache" && ghCalls() === calls, JSON.stringify(second));
  check("nothing is left in temporary directories", !spawnSync("sh", ["-c", `ls -d "${env.TRIBUNAL_DATA_DIR}"/bin/*/*.tmp-* 2>/dev/null`], { encoding: "utf8" }).stdout.trim());
  const repinned = { ...env, TRIBUNAL_RUST_MANIFEST: join(work, "repinned.json") };
  writeFileSync(repinned.TRIBUNAL_RUST_MANIFEST, JSON.stringify(pinned("0".repeat(64))));
  const stale = resolveBinary({ env: repinned });
  check("a cache verified against another pin is not trusted", !stale.ok && /refusing it/.test(stale.error), JSON.stringify(stale));
}
{
  const env = setup(pinned("f".repeat(64)));
  const r = resolveBinary({ env });
  check("an archive whose sha256 differs from the pin is refused", !r.ok && /refusing it/.test(r.error), JSON.stringify(r));
  check("and nothing of it is kept", !existsSync(join(env.TRIBUNAL_DATA_DIR, "bin", "tribunal-rust-v0.0.1", target)));
}
{
  const env = { ...setup(pinned()), GH_FAIL: "1" };
  const r = resolveBinary({ env });
  check("a failed download says what gh said", !r.ok && /HTTP 404/.test(r.error), JSON.stringify(r));
}
{
  const r = resolveBinary({ env: setup({ tag: "tribunal-rust-v0.0.1", repo: "o/r", assets: { [target]: { name: assetName, sha256: "abc" } } }) });
  check("a pin without a full sha256 is refused before any download", !r.ok && /with a sha256/.test(r.error), JSON.stringify(r));
}
{
  const r = resolveBinary({ env: setup(pinned()), platform: "linux", arch: "arm64" });
  check("a platform with no build is refused by name", !r.ok && /linux-arm64/.test(r.error), JSON.stringify(r));
}
{
  const env = { ...setup({ tag: null, repo: "o/r", assets: {} }), TRIBUNAL_BIN: fakeBin };
  const r = resolveBinary({ env });
  check("TRIBUNAL_BIN wins over the pin", r.ok && r.source === "env" && r.bin === fakeBin, JSON.stringify(r));
  const gone = resolveBinary({ env: { ...env, TRIBUNAL_BIN: join(work, "nope") } });
  check("a TRIBUNAL_BIN that does not exist is refused", !gone.ok && /does not exist/.test(gone.error));
}

console.log("\n--- run ---");
{
  const env = setup(pinned());
  const run = spawnSync(process.execPath, [LAUNCHER, "run", "--", "review", "--pr", "12"], { env, encoding: "utf8" });
  check("run passes the arguments through", /args:review --pr 12/.test(run.stdout), run.stdout + run.stderr);
  check("and the binary's exit status", run.status === 7, String(run.status));
  check("and tells the binary where the plugin is", run.stdout.includes(`root:${resolve(PLUGIN_ROOT)}`), run.stdout);
  const none = spawnSync(process.execPath, [LAUNCHER, "run", "--", "review"], { env: setup({ tag: null, repo: "o/r", assets: {} }), encoding: "utf8" });
  check("with nothing to run it exits 3, the skill's cue to use the Workflow", none.status === 3 && /no release is pinned/.test(none.stderr), none.stderr);
  const res = spawnSync(process.execPath, [LAUNCHER, "resolve"], { env: setup({ tag: null, repo: "o/r", assets: {} }), encoding: "utf8" });
  check("resolve prints one JSON line and exits 0 even when it refuses", res.status === 0 && JSON.parse(res.stdout).ok === false);
}

console.log("\n--- pin-release ---");
{
  const lines = TARGETS.map((t, i) => `${String(i).repeat(64)}  tribunal-v1.2.3-${t}.${t.includes("windows") ? "zip" : "tar.gz"}`);
  const m = manifestFrom("tribunal-rust-v1.2.3", "o/r", [...lines, "", "not a sha line"]);
  check("every platform's archive and sha256 is pinned", TARGETS.every((t) => m.assets[t]?.sha256?.length === 64) && m.assets["x86_64-pc-windows-msvc"].name.endsWith(".zip"));
  let threw = "";
  try {
    manifestFrom("tribunal-rust-v1.2.3", "o/r", lines.slice(1));
  } catch (e) {
    threw = e.message;
  }
  check("a release missing a platform is refused by name", /aarch64-apple-darwin/.test(threw), threw);
  const shipped = JSON.parse(readFileSync(join(PLUGIN_ROOT, "rust", "release.json"), "utf8"));
  check("the shipped release.json has the launcher's shape", "tag" in shipped && typeof shipped.repo === "string" && typeof shipped.assets === "object");
}

rmSync(work, { recursive: true, force: true });
if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe launcher runs only the binary its pin names, and says why when it cannot");
