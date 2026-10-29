#!/usr/bin/env node
// rust-engine.mjs: finds the `tribunal` binary for this machine and runs it.
//
//   node rust-engine.mjs resolve          prints {ok, bin, source, tag, target}
//                                         or {ok: false, error}; exits 0 either way
//   node rust-engine.mjs run -- <args>    resolves, then runs the binary with
//                                         <args> and exits with its status
//
// The binary is not in the plugin: each `tribunal-rust-v*` release attaches
// one archive per platform, and rust/release.json pins the release and every
// archive's sha256. The first run on a machine downloads its archive with
// `gh release download`, refuses it unless the sha256 matches the pin, and
// unpacks it under <tribunal-data>/bin/<tag>/<target>/; later runs reuse it.
// TRIBUNAL_BIN, when set, is used as is and nothing is downloaded.
//
// A failed resolve is not an error for the skill: it says so and runs the
// Workflow instead.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const PLUGIN_ROOT = resolve(HERE, "../../..");

const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-musl",
  "win32-x64": "x86_64-pc-windows-msvc",
  // Windows on ARM runs x64 binaries through its own emulation.
  "win32-arm64": "x86_64-pc-windows-msvc",
};

export function targetFor(platform, arch) {
  return TARGETS[`${platform}-${arch}`] ?? null;
}

export function dataDir(env = process.env) {
  if (env.TRIBUNAL_DATA_DIR) return env.TRIBUNAL_DATA_DIR;
  const home = env.HOME || env.USERPROFILE || homedir();
  return join(home, ".claude", "tribunal-data");
}

export function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

// tar on Windows: System32's bsdtar unpacks a .zip, and a GNU tar earlier on
// PATH (Git Bash) does not and reads "C:" as a remote host.
function tarCommand(env) {
  if (process.platform !== "win32") return "tar";
  return join(env.SystemRoot || "C:\\Windows", "System32", "tar.exe");
}

export function resolveBinary({ env = process.env, platform = process.platform, arch = process.arch } = {}) {
  if (env.TRIBUNAL_BIN) {
    return existsSync(env.TRIBUNAL_BIN)
      ? { ok: true, bin: env.TRIBUNAL_BIN, source: "env", tag: null, target: null }
      : { ok: false, error: `TRIBUNAL_BIN is ${env.TRIBUNAL_BIN}, which does not exist` };
  }
  const manifestPath = env.TRIBUNAL_RUST_MANIFEST || join(PLUGIN_ROOT, "rust", "release.json");
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (e) {
    return { ok: false, error: `cannot read ${manifestPath}: ${e.message}` };
  }
  if (!manifest.tag) return { ok: false, error: `no release is pinned in ${manifestPath}` };
  const target = targetFor(platform, arch);
  if (!target) return { ok: false, error: `no ${manifest.tag} build for ${platform}-${arch}` };
  const asset = manifest.assets?.[target];
  if (!asset?.name || !/^[0-9a-f]{64}$/.test(asset.sha256 ?? "")) {
    return { ok: false, error: `${manifestPath} pins no ${target} archive with a sha256` };
  }
  const exe = target.includes("windows") ? "tribunal.exe" : "tribunal";
  const base = asset.name.replace(/\.(tar\.gz|zip)$/, "");
  const dir = join(dataDir(env), "bin", manifest.tag, target);
  const bin = join(dir, base, exe);
  const marker = join(dir, "verified");
  const cached = (s) => ({ ok: true, bin, source: s, tag: manifest.tag, target });
  if (existsSync(bin) && existsSync(marker) && readFileSync(marker, "utf8").trim() === asset.sha256) {
    return cached("cache");
  }

  mkdirSync(dirname(dir), { recursive: true });
  const work = mkdtempSync(`${dir}.tmp-`);
  try {
    const gh = spawnSync(
      env.TRIBUNAL_GH || "gh",
      ["release", "download", manifest.tag, "--repo", manifest.repo, "--pattern", asset.name, "--dir", work, "--clobber"],
      { encoding: "utf8", env },
    );
    const archive = join(work, asset.name);
    if (gh.error || gh.status !== 0 || !existsSync(archive)) {
      const why = gh.error?.message || gh.stderr?.trim() || `exit ${gh.status}`;
      return { ok: false, error: `gh release download ${manifest.tag} ${asset.name} failed: ${why}` };
    }
    const got = sha256File(archive);
    if (got !== asset.sha256) {
      return { ok: false, error: `${asset.name} has sha256 ${got}, the pin is ${asset.sha256}; refusing it` };
    }
    const tar = spawnSync(tarCommand(env), ["-xf", archive, "-C", work], { encoding: "utf8" });
    if (tar.error || tar.status !== 0) {
      return { ok: false, error: `cannot unpack ${asset.name}: ${tar.error?.message || tar.stderr.trim()}` };
    }
    rmSync(archive);
    if (!existsSync(join(work, base, exe))) {
      return { ok: false, error: `${asset.name} holds no ${base}/${exe}` };
    }
    if (process.platform !== "win32") chmodSync(join(work, base, exe), 0o755);
    writeFileSync(join(work, "verified"), `${asset.sha256}\n`);
    rmSync(dir, { recursive: true, force: true });
    try {
      renameSync(work, dir);
    } catch (e) {
      // Another run unpacked the same archive first: theirs is as good.
      if (!(existsSync(bin) && existsSync(marker))) throw e;
    }
    return cached("download");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function main(argv) {
  const [cmd, ...rest] = argv;
  if (cmd === "resolve") {
    console.log(JSON.stringify(resolveBinary()));
    return 0;
  }
  if (cmd === "run" && rest[0] === "--") {
    const r = resolveBinary();
    if (!r.ok) {
      console.error(`rust-engine: ${r.error}`);
      return 3;
    }
    // The binary finds agents/, methodology/ and lib/ through this; its
    // compiled-in default is the CI machine's checkout.
    const run = spawnSync(r.bin, rest.slice(1), {
      stdio: "inherit",
      env: { ...process.env, TRIBUNAL_PLUGIN_ROOT: process.env.TRIBUNAL_PLUGIN_ROOT || PLUGIN_ROOT },
    });
    if (run.error) {
      console.error(`rust-engine: cannot start ${r.bin}: ${run.error.message}`);
      return 3;
    }
    return run.status ?? 1;
  }
  console.error("usage: rust-engine.mjs resolve | run -- <tribunal args>");
  return 2;
}

// realpath on both sides: the plugin cache can reach this file through a
// symlink, and a plain comparison then never runs main.
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
