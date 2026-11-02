#!/usr/bin/env node
// replay-rust-engine.mjs: the launcher behind the Rust engine, bin/tribunal-engine,
// runs only the binary its pin names. A stand-in `gh` serves a stand-in
// release and a stand-in `uname` plays other machines, so this costs 0 tokens
// and no network. Tier 0.
//
// Usage: node rust/scripts/replay-rust-engine.mjs

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { manifestFrom, TARGETS } from "./pin-release.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const PLUGIN_ROOT = resolve(HERE, "..", "..");
const LAUNCHER = join(PLUGIN_ROOT, "bin", "tribunal-engine");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

if (process.platform === "win32") {
  console.log("replay-rust-engine: the stand-ins are shell scripts; skipped on Windows");
  process.exit(0);
}

const work = mkdtempSync(join(tmpdir(), "rust-engine-"));
const stand = join(work, "stand-ins");
mkdirSync(stand);
const script = (path, body) => {
  writeFileSync(path, `#!/bin/sh\n${body}`);
  chmodSync(path, 0o755);
};

// This machine's target, as the launcher should pick it.
const hostTarget = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-musl",
}[`${process.platform}-${process.arch}`];
if (!hostTarget) {
  console.log(`replay-rust-engine: no release target for ${process.platform}-${process.arch}; skipped`);
  process.exit(0);
}
const base = `tribunal-v0.0.1-${hostTarget}`;
const assetName = `${base}.tar.gz`;
const archive = join(work, "release", assetName);
mkdirSync(join(work, "release", base), { recursive: true });
const fakeBin = join(work, "release", base, "tribunal");
script(fakeBin, 'echo "args:$*"\necho "root:$TRIBUNAL_PLUGIN_ROOT"\nexit 7\n');
spawnSync("tar", ["-czf", archive, "-C", join(work, "release"), base]);
const sha = createHash("sha256").update(readFileSync(archive)).digest("hex");

const ghLog = join(work, "gh.log");
const gh = join(stand, "gh");
script(
  gh,
  `echo "$*" >> "${ghLog}"
[ -n "$GH_FAIL" ] && { echo "HTTP 404: release not found" >&2; exit 1; }
while [ $# -gt 0 ]; do [ "$1" = "--dir" ] && dir="$2"; shift; done
cp "${archive}" "$dir/"
`,
);
const ghCalls = () => (existsSync(ghLog) ? readFileSync(ghLog, "utf8").trim().split("\n").length : 0);
// The launcher must not need node: one first on PATH records any call.
const nodeLog = join(work, "node.log");
script(join(stand, "node"), `echo "$*" >> "${nodeLog}"\nexit 1\n`);
// FAKE_UNAME="<os> <arch>" plays another machine.
const realUname = spawnSync("sh", ["-c", "command -v uname"], { encoding: "utf8" }).stdout.trim();
script(
  join(stand, "uname"),
  `[ -z "$FAKE_UNAME" ] && exec "${realUname}" "$@"
set -- "$1" $FAKE_UNAME
case "$1" in -s) echo "$2" ;; -m) echo "$3" ;; *) exec "${realUname}" "$1" ;; esac
`,
);

let n = 0;
const setup = (manifest) => {
  n += 1;
  const m = join(work, `release-${n}.json`);
  writeFileSync(m, typeof manifest === "string" ? manifest : JSON.stringify(manifest, null, 2));
  const env = { ...process.env, TRIBUNAL_RUST_MANIFEST: m, TRIBUNAL_DATA_DIR: join(work, `data-${n}`), PATH: `${stand}:${process.env.PATH}` };
  delete env.TRIBUNAL_BIN;
  delete env.TRIBUNAL_PLUGIN_ROOT;
  delete env.FAKE_UNAME;
  delete env.GH_FAIL;
  return env;
};
const pinned = (s = sha) => ({ tag: "tribunal-rust-v0.0.1", repo: "o/r", assets: { [hostTarget]: { name: assetName, sha256: s } } });
const launch = (env, ...args) => spawnSync("bash", [LAUNCHER, ...args], { env, encoding: "utf8" });
const resolveWith = (env) => {
  const r = launch(env, "resolve");
  try {
    return { ...JSON.parse(r.stdout), status: r.status };
  } catch {
    return { ok: undefined, raw: r.stdout + r.stderr, status: r.status };
  }
};

console.log("\n--- which build a machine gets ---");
{
  const all = {
    "aarch64-apple-darwin": { name: "tribunal-v0.0.1-aarch64-apple-darwin.tar.gz", sha256: "a".repeat(64) },
    "x86_64-apple-darwin": { name: "tribunal-v0.0.1-x86_64-apple-darwin.tar.gz", sha256: "b".repeat(64) },
    "x86_64-unknown-linux-musl": { name: "tribunal-v0.0.1-x86_64-unknown-linux-musl.tar.gz", sha256: "c".repeat(64) },
    "x86_64-pc-windows-msvc": { name: "tribunal-v0.0.1-x86_64-pc-windows-msvc.zip", sha256: "d".repeat(64) },
  };
  // A failed download names the asset the launcher picked.
  const picks = (uname) => resolveWith({ ...setup({ tag: "tribunal-rust-v0.0.1", repo: "o/r", assets: all }), FAKE_UNAME: uname, GH_FAIL: "1" });
  const asks = (uname, target) => {
    const r = picks(uname);
    return r.ok === false && r.error.includes(`tribunal-v0.0.1-${target}.`);
  };
  check("Apple silicon gets aarch64-apple-darwin", asks("Darwin arm64", "aarch64-apple-darwin"));
  check("an Intel Mac gets x86_64-apple-darwin", asks("Darwin x86_64", "x86_64-apple-darwin"));
  check("Linux x64 gets the static musl build", asks("Linux x86_64", "x86_64-unknown-linux-musl"));
  check(
    "Git Bash on Windows x64 and on ARM gets the msvc build",
    asks("MINGW64_NT-10.0-26100 x86_64", "x86_64-pc-windows-msvc") &&
      asks("MINGW64_NT-10.0-26100 aarch64", "x86_64-pc-windows-msvc") &&
      asks("MSYS_NT-10.0 x86_64", "x86_64-pc-windows-msvc"),
  );
  const calls = ghCalls();
  const arm = picks("Linux aarch64");
  check(
    "Linux arm64 has no build, and the refusal names the machine and the builds there are",
    arm.ok === false &&
      /no tribunal-rust-v0\.0\.1 build for Linux aarch64/.test(arm.error) &&
      arm.error.includes("x86_64-unknown-linux-musl") &&
      arm.error.includes("TRIBUNAL_BIN"),
    arm.error,
  );
  check("and nothing was downloaded for it", ghCalls() === calls);
}

console.log("\n--- resolve ---");
{
  const r = resolveWith(setup({ tag: null, repo: "o/r", assets: {} }));
  check("no pinned release is a reasoned refusal, not a crash", r.ok === false && /no release is pinned/.test(r.error) && r.status === 0, JSON.stringify(r));
}
{
  const r = resolveWith(setup("not json at all"));
  check("an unreadable pin is refused", r.ok === false && /no release is pinned/.test(r.error), JSON.stringify(r));
}
{
  const r = resolveWith({ ...setup(pinned()), TRIBUNAL_RUST_MANIFEST: join(work, "missing.json") });
  check("a missing pin file is refused by path", r.ok === false && /cannot read .*missing\.json/.test(r.error), JSON.stringify(r));
}
{
  const env = setup(pinned());
  const before = ghCalls();
  const first = resolveWith(env);
  check("the first run downloads, verifies and unpacks", first.ok === true && first.source === "download" && existsSync(first.bin), JSON.stringify(first));
  check("it reports the tag and the target", first.tag === "tribunal-rust-v0.0.1" && first.target === hostTarget);
  check("the unpacked binary is executable", first.ok === true && (statSync(first.bin).mode & 0o111) !== 0);
  const asked = readFileSync(ghLog, "utf8").trim().split("\n").at(-1);
  check(
    "it asked gh for exactly the pinned asset of the pinned tag",
    ghCalls() === before + 1 && asked.startsWith(`release download tribunal-rust-v0.0.1 --repo o/r --pattern ${assetName} --dir `),
    asked,
  );
  const calls = ghCalls();
  const second = resolveWith(env);
  check("the second run reuses it without gh", second.ok === true && second.source === "cache" && ghCalls() === calls, JSON.stringify(second));
  const leftovers = () => spawnSync("sh", ["-c", `ls -d "${env.TRIBUNAL_DATA_DIR}"/bin/*/*.tmp-* "${env.TRIBUNAL_DATA_DIR}"/bin/*/*/*.tmp-* 2>/dev/null`], { encoding: "utf8" }).stdout.trim();
  check("nothing is left in temporary directories", !leftovers());
  const repinned = { ...env, TRIBUNAL_RUST_MANIFEST: join(work, "repinned.json") };
  writeFileSync(repinned.TRIBUNAL_RUST_MANIFEST, JSON.stringify(pinned("0".repeat(64))));
  const stale = resolveWith(repinned);
  check("a cache verified against another pin is not trusted", stale.ok === false && /refusing it/.test(stale.error), JSON.stringify(stale));
  const again = resolveWith(env);
  check("and the refused download did not replace the good cache", again.ok === true && again.source === "cache", JSON.stringify(again));
  check("nor left anything behind", !leftovers());
}
{
  const env = setup("{}");
  writeFileSync(env.TRIBUNAL_RUST_MANIFEST, JSON.stringify({ assets: { [hostTarget]: { sha256: sha, name: assetName } }, repo: "o/r", tag: "tribunal-rust-v0.0.1" }));
  const r = resolveWith(env);
  check("the pin is read whatever its key order and layout", r.ok === true && r.source === "download", JSON.stringify(r));
}
{
  const env = setup(pinned("f".repeat(64)));
  const r = resolveWith(env);
  check("an archive whose sha256 differs from the pin is refused", r.ok === false && /refusing it/.test(r.error), JSON.stringify(r));
  check("and nothing of it is kept", !existsSync(join(env.TRIBUNAL_DATA_DIR, "bin", "tribunal-rust-v0.0.1", hostTarget)));
}
{
  const r = resolveWith({ ...setup(pinned()), GH_FAIL: "1" });
  check("a failed download says what gh said", r.ok === false && /HTTP 404/.test(r.error), JSON.stringify(r));
}
{
  const r = resolveWith(setup({ tag: "tribunal-rust-v0.0.1", repo: "o/r", assets: { [hostTarget]: { name: assetName, sha256: "abc" } } }));
  check("a pin without a full sha256 is refused before any download", r.ok === false && /with a sha256/.test(r.error), JSON.stringify(r));
}
{
  const env = { ...setup({ tag: null, repo: "o/r", assets: {} }), TRIBUNAL_BIN: fakeBin };
  const r = resolveWith(env);
  check("TRIBUNAL_BIN wins over the pin", r.ok === true && r.source === "env" && r.bin === fakeBin && r.tag === null, JSON.stringify(r));
  const gone = resolveWith({ ...env, TRIBUNAL_BIN: join(work, "nope") });
  check("a TRIBUNAL_BIN that does not exist is refused", gone.ok === false && /does not exist/.test(gone.error));
}
{
  const odd = join(work, 'a "quoted" \\ dir', "tribunal");
  mkdirSync(dirname(odd), { recursive: true });
  script(odd, "exit 0\n");
  const r = resolveWith({ ...setup(pinned()), TRIBUNAL_BIN: odd });
  check("resolve prints valid JSON for a path with quotes and backslashes", r.ok === true && r.bin === odd, JSON.stringify(r));
}
{
  const env = setup("{}");
  writeFileSync(env.TRIBUNAL_RUST_MANIFEST, readFileSync(join(PLUGIN_ROOT, "rust", "release.json"), "utf8"));
  const shipped = JSON.parse(readFileSync(env.TRIBUNAL_RUST_MANIFEST, "utf8"));
  const r = resolveWith({ ...env, GH_FAIL: "1" });
  check(
    "the shipped release.json is read: the launcher asks for its tag and this machine's archive",
    r.ok === false && r.error.includes(`gh release download ${shipped.tag} ${shipped.assets[hostTarget].name} failed`),
    JSON.stringify(r),
  );
}

console.log("\n--- run ---");
{
  const env = setup(pinned());
  const run = launch(env, "run", "--", "review", "--pr", "12", "--context", "two words");
  check("run passes the arguments through", run.stdout.includes("args:review --pr 12 --context two words"), run.stdout + run.stderr);
  check("and the binary's exit status", run.status === 7, String(run.status));
  check("and tells the binary where the plugin is", run.stdout.includes(`root:${PLUGIN_ROOT}`), run.stdout);
  const kept = launch({ ...env, TRIBUNAL_PLUGIN_ROOT: "/elsewhere" }, "run", "--", "x");
  check("a TRIBUNAL_PLUGIN_ROOT already set is kept", kept.stdout.includes("root:/elsewhere"), kept.stdout);
  const none = launch(setup({ tag: null, repo: "o/r", assets: {} }), "run", "--", "review");
  check("with nothing to run it exits 3 and says why", none.status === 3 && /^tribunal-engine: no release is pinned/.test(none.stderr), none.stderr);
  const usage = launch(env, "review");
  check("anything but resolve or run -- is a usage error", usage.status === 2 && /usage/.test(usage.stderr));
  check("no step called node", !existsSync(nodeLog), existsSync(nodeLog) ? readFileSync(nodeLog, "utf8") : "");
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
