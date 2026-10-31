#!/usr/bin/env node
// harness-version.mjs — a content hash identifying WHICH version of the
// peer-review harness produced a given run. Deterministic, 0 tokens, no git
// required (it works before, during and after the repo exists).
//
// Why. metrics.jsonl holds 44 rows produced by an unknown number of code
// versions, with nothing on the row saying which. That single gap is why "are we
// better or worse than before" cannot be answered from the data we already paid
// to collect: every comparison silently mixes versions. `schemaVersion` does not
// substitute — it describes the row's SHAPE, not the code that filled it.
//
// The hash covers only files that can change a review's BEHAVIOUR. Measurement
// data (metrics.jsonl, agent-*.jsonl), specs and the eval corpus are excluded on
// purpose: they change constantly and their movement is not a new harness. The
// eval scripts are excluded for the same reason — a replay is a test of the
// harness, not a part of it.
//
// Usage:
//   node scripts/harness-version.mjs             # id + file count
//   node scripts/harness-version.mjs --short     # just the id, for stamping
//   node scripts/harness-version.mjs --manifest  # every path with its own hash
//
// A missing file is a HARD failure, not a skipped line: silently hashing 11 of
// 12 files would produce a stable-looking id for a harness that lost a piece.
// A missing directory is not: it holds no files, so it adds none to the hash,
// and the id changes with the files it lost.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PLUGIN_ROOT } from "../eval/paths.mjs";

// A strict `import.meta.url === file://${process.argv[1]}` comparison breaks
// when the script is invoked through a symlink (e.g. the ~/.claude/skills/...
// symlink into this plugin's real, version-keyed cache directory): argv[1]
// keeps the symlink path while import.meta.url resolves to the real one, so
// they never match, main() silently never runs, and the script exits 0 having
// done nothing. Resolve both sides to their real path before comparing.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

// Was `resolve(homedir(), ".claude")` until the harness became a plugin. A
// plugin is copied into a version-keyed cache directory, so a hash anchored to
// the home directory would have hashed whatever older copy still sat in
// ~/.claude — or thrown on a machine that never had one.
const ROOT = PLUGIN_ROOT;

// THE SURFACE — defined once here and imported by eval/release.mjs, so a freeze
// and a version id can never disagree about what the harness is.
// THE SURFACE — defined once here and imported by eval/release.mjs, so a freeze
// and a version id can never disagree about what the harness is.
//
// `lib/` and `workflows/` are DIRECTORIES, not a hand-kept list of names, since
// 2026-09-09. They were a list, and a list fails in the direction that costs
// you: a module added to lib/ was outside the freeze until somebody remembered
// to name it, and nothing said otherwise. Two had already fallen through.
// `lib/verified.mjs` is the evidence gate that demotes blocking findings and was
// never listed at all; `lib/resolve-verified-anchors.mjs` shipped in v52 and
// `workflows/harness-selfcheck.js` moved into the plugin in v54, and neither was
// added. So a file under these directories is in the freeze unless someone
// writes down why it is not, which is the direction that fails safe.
export const SURFACE = ["skills/review-pr/SKILL.md"];

// Deliberately outside the freeze, with the reason. Empty today, and the entry
// shape is the point: eval/replay-surface-coverage.mjs refuses an exclusion with
// no reason and refuses one naming a file that no longer exists, so this cannot
// become a place to quietly park things.
export const SURFACE_EXCLUDED = {};

// Directories whose every matching file counts, sorted for determinism.
const SURFACE_DIRS = [
  { dir: "skills/review-pr/references", exts: [".md"] },
  { dir: "skills/review-pr/scripts", exts: [".mjs"] },
  // The orchestrator and the self-check that audits it. Both are executed by
  // the Workflow runtime; a change to either changes what a run does.
  { dir: "workflows", exts: [".js"] },
  // Every module and script the harness shells out to or imports at run time.
  { dir: "lib", exts: [".mjs", ".sh"] },
  // The agent definitions the harness dispatches into. Excluded until
  // 2026-07-27, which meant a frozen version did not freeze the 240-line prompt
  // that shapes every lens, the bot vetting, the completeness critic and the
  // prior-finding verifier — editing it changed every review with no version
  // bump. A hash over "the behavioural surface" that omits the most influential
  // prompt in the harness is not a hash over the behavioural surface. That
  // sentence is why lib/ and workflows/ became directories too.
  //
  // `code-review-expert.md` used to be a SYMLINK into the BE repo, which meant a
  // checkout of any tag restored a dangling link on a machine without that repo
  // at that path. Packaging the harness as a plugin vendored a real copy here,
  // so the hash now covers bytes this repository can actually restore. The cost
  // is that the same definition is versioned in two repositories on different
  // branch cadences, and nothing yet detects them drifting apart.
  { dir: "agents", exts: [".md"] },
  // The methodology the lens personas Read before every review
  // (agents/code-review-expert.md). Outside the hash until 2026-10-19: appending
  // a rule to code-review-methodology.md changed every review and left the id
  // unchanged. A project profile is NOT hashed here; a run records its
  // profileSha256 separately, so the harness id stays comparable across projects.
  { dir: "methodology", exts: [".md"] },
];

// The battery is hashed SEPARATELY, and the separation is the point.
//
// A freeze row asserts "the offline battery was green at this content hash", and
// until now the hash did not pin the battery: `eval/` was outside the surface,
// so `pr-harness-4af7ff70ff8e` is the same id `main` carried while
// `replay-embedded-bash` was red. The row's central claim could not be
// re-verified by checking out its own hash.
//
// The fix is not to fold `eval/` into the surface. `harnessVersion` is the field
// that makes any comparison BETWEEN runs meaningful, and a test-only edit that
// moved it would put churn into exactly the field that must not have any: two
// runs would carry different ids while behaving identically. So the battery gets
// its own id, and a freeze row records both. The claim it can then make is "the
// battery at hash B was green over the harness at hash H", which is checkable.
//
// Everything under eval/ counts, fixtures and corpus included. A battery result
// over different fixtures is a different result, and unlike the harness hash
// this one has no cross-run comparison to protect, so completeness beats
// stability here.
const BATTERY_DIR = "skills/review-pr/eval";

function filesUnder(dir, exts) {
  const abs = join(ROOT, dir);
  if (!existsSync(abs)) return [];
  const out = [];
  for (const entry of readdirSync(abs, { withFileTypes: true })) {
    const rel = `${dir}/${entry.name}`;
    if (entry.isDirectory()) out.push(...filesUnder(rel, exts));
    else if (!exts || exts.some((e) => entry.name.endsWith(e))) out.push(rel);
  }
  return out;
}

function digest(paths) {
  const entries = paths.sort().map((rel) => {
    const abs = join(ROOT, rel);
    if (!existsSync(abs)) throw new Error(`harness surface file missing: ${rel}`);
    return { path: rel, sha: createHash("sha256").update(readFileSync(abs)).digest("hex") };
  });
  // Hash the MANIFEST TEXT, not the concatenated contents: two files swapping
  // their contents would collide under concatenation and not under this.
  const manifest = entries.map((e) => `${e.path}:${e.sha}`).join("\n");
  return { entries, manifest, short: createHash("sha256").update(manifest).digest("hex").slice(0, 12) };
}

export function surfaceFiles() {
  const fromDirs = SURFACE_DIRS.flatMap(({ dir, exts }) => filesUnder(dir, exts));
  return [...SURFACE, ...fromDirs].filter((f) => !(f in SURFACE_EXCLUDED)).sort();
}

// Every file the battery hash covers. Exported so the coverage replay and any
// future consumer read the same definition this module uses.
export function batteryFiles() {
  return filesUnder(BATTERY_DIR, null).sort();
}

export function batteryVersion() {
  const { entries, manifest, short } = digest(batteryFiles());
  return { id: `pr-battery-${short}`, entries, manifest };
}

export function harnessVersion() {
  const { entries, manifest, short } = digest(surfaceFiles());
  return { id: `pr-harness-${short}`, entries, manifest };
}

if (isMainModule()) {
  const { id, entries } = harnessVersion();
  if (process.argv.includes("--short")) {
    console.log(id);
  } else if (process.argv.includes("--manifest")) {
    console.log(id);
    for (const e of entries) console.log(`  ${e.sha.slice(0, 12)}  ${e.path}`);
  } else {
    console.log(`${id}  (${entries.length} files)`);
  }
}
