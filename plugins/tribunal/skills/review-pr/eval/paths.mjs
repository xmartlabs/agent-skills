#!/usr/bin/env node
// paths.mjs — the one place that knows where the harness lives on disk.
//
// WHY IT EXISTS. Until the harness was packaged as a plugin, 36 call sites
// across 28 files resolved their own paths against `homedir()/.claude/...`.
// That worked while the harness only ever lived in one place. A plugin is
// copied into a version-keyed cache directory whose path nobody can predict,
// so every one of those call sites would have resolved to a file that is not
// there — silently, since most of them feed `readFileSync` inside a try or a
// region slicer that reports "region not found" rather than "wrong path".
//
// This is the same move `rows.mjs` already made for metrics.jsonl, for the same
// reason: a rule that lives in 28 copies is a rule that will be wrong in at
// least one of them. `replay-region-coverage.mjs` guards that nothing goes back
// to resolving its own.
//
// DATA vs CODE. The plugin directory is replaced wholesale on every update, so
// nothing writable may live inside it. Measurement state resolves to DATA_DIR
// instead, which is outside the plugin and survives updates.

import { existsSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { cwdRepo, resolveRepo } from "../../../lib/operator.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

// `CLAUDE_PLUGIN_ROOT` is set when a plugin component runs inside Claude Code.
// The eval battery is also run by hand from a terminal, where it is not set, so
// the relative walk is the fallback rather than the other way round: it is
// derived from this file's own location and cannot be stale.
//   <root>/skills/review-pr/eval/paths.mjs -> ../../.. -> <root>
export const PLUGIN_ROOT =
  process.env.CLAUDE_PLUGIN_ROOT || resolve(HERE, "..", "..", "..");

export const SKILL_DIR = resolve(PLUGIN_ROOT, "skills", "review-pr");
// The commented source is what the battery reads. The Workflow runs the built
// copy, the same code with comment lines removed (scripts/build-workflow.mjs).
export const WORKFLOW = resolve(PLUGIN_ROOT, "workflow-src", "tribunal.js");
export const WORKFLOW_BUILT = resolve(PLUGIN_ROOT, "workflows", "tribunal.js");
export const LIB_DIR = resolve(PLUGIN_ROOT, "lib");
export const AGENTS_DIR = resolve(PLUGIN_ROOT, "agents");

export const lib = (name) => resolve(LIB_DIR, name);
export const agent = (name) => resolve(AGENTS_DIR, name);

// Writable state. Never inside PLUGIN_ROOT — see the DATA vs CODE note above.
export const DATA_DIR =
  process.env.TRIBUNAL_DATA_DIR || resolve(homedir(), ".claude", "tribunal-data");

export const METRICS_PATH = resolve(DATA_DIR, "metrics.jsonl");
export const AGENT_COSTS_PATH = resolve(DATA_DIR, "agent-costs.jsonl");
export const AGENT_COUNTS_PATH = resolve(DATA_DIR, "agent-counts.jsonl");
export const INFLIGHT_PATH = resolve(DATA_DIR, "inflight-runs.jsonl");
export const QUEUE_STATE_PATH = resolve(DATA_DIR, "queue-state.json");

// A client's recorded history and test data: the runs the replays re-decide,
// the fixtures cut from real runs, the labelled corpus, the specs and the
// release log. It lives in the documentation repo next to plugins/, never
// inside a plugin, so installing the plugin never copies a client's data.
// Outside that repo these paths do not exist, and every replay that needs one
// fails on the missing file instead of passing over nothing.
export const HARNESS_DATA =
  process.env.TRIBUNAL_HARNESS_DATA || resolve(PLUGIN_ROOT, "..", "..", "review-harness-data");
export const TRIBUNAL_DATA = resolve(HARNESS_DATA, "tribunal");
export const FIXTURES_DIR = resolve(TRIBUNAL_DATA, "fixtures");
export const CORPUS_DIR = resolve(TRIBUNAL_DATA, "corpus");
// The release log is this repository's own, generic: a row says which gate ran
// and what changed, and carries no client data. TRIBUNAL_RELEASES overrides it.
export const RELEASES_PATH = process.env.TRIBUNAL_RELEASES || resolve(PLUGIN_ROOT, "RELEASES.md");
// The replays that read HARNESS_DATA and so cannot run where it is absent. One
// list for the whole repo, kept by scripts/check-portable.sh.
export const NEEDS_DATA_PATH = resolve(PLUGIN_ROOT, "scripts", "needs-data.txt");
export const PROFILES_DIR = resolve(HARNESS_DATA, "profiles");
// A frozen ledger, so a replay re-decides the same recorded runs on every
// machine instead of whatever this operator's DATA_DIR happens to hold.
export const RECORDED_METRICS_PATH = resolve(TRIBUNAL_DATA, "ledger", "metrics.jsonl");
export const RECORDED_INFLIGHT_PATH = resolve(TRIBUNAL_DATA, "ledger", "inflight-runs.jsonl");

// Claude Code's own session transcripts. Genuinely outside the plugin, and
// genuinely under the real home directory — this one is not a packaging
// artefact and stays homedir-based.
export const PROJECTS_DIR = resolve(homedir(), ".claude", "projects");

// The per-workspace transcript directory. Was hardcoded to one contributor's
// path in six files, which made every cost analysis silently empty on anyone
// else's machine — `findRunDirs` on a missing directory returns nothing, so the
// failure reads as "this run had no agents" rather than as a wrong path.
export function projectDir(cwd = process.cwd()) {
  if (process.env.TRIBUNAL_PROJECT_DIR) return process.env.TRIBUNAL_PROJECT_DIR;
  return resolve(PROJECTS_DIR, projectSlug(resolve(cwd)));
}

// Claude Code's own rule: every character but a letter or a digit becomes "-".
// Replacing only "/", "." and "_" left a Windows cwd (C:\GIT\x) whole, and
// resolve() then returned the cwd itself instead of a directory under projects.
export function projectSlug(absPath) {
  return absPath.replace(/[^A-Za-z0-9]/g, "-");
}

// The repository under review, as a checkout on disk. Only the Tier-1 rigs
// need one (they re-read the code a recorded refuter read). TRIBUNAL_REPO_DIR
// wins; otherwise the repo is `repo`, else what a review would pick (the
// operator's defaultRepo, else the cwd's origin), and the checkout is the first
// of the cwd, ./<name> and ../<name> whose origin is exactly that repo. Null rather than a guess when none matches: a wrong
// checkout would score an arm against the wrong source.
export function reviewRepo(repo = "", { cwd = process.cwd(), originOf = cwdRepo } = {}) {
  if (process.env.TRIBUNAL_REPO_DIR) return process.env.TRIBUNAL_REPO_DIR;
  const target = repo || resolveRepo({ cwd, cwdRepoOf: originOf }).repo;
  if (!target) return null;
  const name = target.split("/")[1];
  for (const dir of [cwd, resolve(cwd, name), resolve(cwd, "..", name)])
    if (existsSync(dir) && originOf(dir) === target) return dir;
  return null;
}

// Is this module the entry point? Callers pass their own `import.meta.url`:
//
//   if (isMainModule(import.meta.url)) { ... }
//
// The obvious spelling, `import.meta.url === "file://" + process.argv[1]`, is
// wrong twice over, and both failures are silent — the guard is false, the body
// never runs, and the process exits 0, which is indistinguishable from a script
// that ran and found nothing.
//
//   - Through a symlink (the ~/.claude/skills/... link into this plugin's real,
//     version-keyed cache directory), argv[1] keeps the link path while
//     import.meta.url resolves to the real one.
//   - On Windows it can never match at all: import.meta.url normalises to
//     `file:///C:/Users/...` (three slashes, forward slashes) while the string
//     concatenation builds `file://C:\Users\...`. Measured 2026-08-10 — seven
//     eval scripts, `lost-runs.mjs` among them, had never executed on that
//     platform. A loss detector reporting silence over 44 started / 38 completed
//     runs is the failure this guard is supposed to make impossible.
//
// Resolving both sides to a real path fixes both. All NINE scripts under
// `scripts/` carry this as a local copy predating this one — `build-post-footer`,
// `count-in-context-agents`, `harness-version`, `incremental-scope`,
// `locate-last-row`, `persist-review-record`, `post-review`,
// `resolve-evidence-line`, `strip-draft-comments`. They are equivalent, and this
// is the shared version for the same reason the rest of this file exists. The
// count is the argument, so it is spelled as a list: nine copies is a rule that
// will be wrong in at least one of them, and "four" understated the case by more
// than half.
export function isMainModule(importMetaUrl) {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(importMetaUrl));
  } catch {
    return false;
  }
}
