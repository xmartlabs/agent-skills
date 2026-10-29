// orphaned-worktrees.mjs — which peer-review-wt-* worktrees never got cleaned up.
//
// `tribunal.js` creates worktrees at several points (Fusion A/B head, the
// grounded-typecheck fallback, self-heal on a resumed run) with `mktemp -d
// "${TMPDIR:-/tmp}/peer-review-wt-<pr>-XXXXXX"`, and removes them with `git
// worktree remove --force` only as part of the LAST agent() dispatch in the
// whole script. Nothing runs in a top-level try/finally or on
// process.on('exit'|'SIGINT'|'SIGTERM') — there is no such handler in
// tribunal.js — so any of the ~15 sequential stages between worktree creation
// and that final cleanup throwing (an agent dispatch failure, a schema
// validation error, a session cutoff) exits the script before cleanup runs and
// leaks the worktree.
//
// This is the same failure shape lost-runs.mjs already tracks for the metrics
// row (a run dying mid-flight), applied to the worktree it may have left
// behind. Consistent with this harness's own style, it detects and reports —
// it does not delete anything unless explicitly asked with --clean, matching
// how lost-runs.mjs never mutates inflight-runs.jsonl on your behalf.
//
// A leaked worktree is a DISK leak, not a correctness bug: tribunal.js:2943-51
// documents that the switch to `mktemp -d` (unique path per run, not a fixed
// per-PR path) already stopped a stale worktree from corrupting a LATER run
// (PR #1935) — self-heal logic at tribunal.js:3484-3527 recreates a missing
// worktree transparently. What's missing is reclaiming the orphan's disk once
// it is no longer needed by anything.
import { execFileSync } from "node:child_process";
import { statSync } from "node:fs";
import { isMainModule, reviewRepo } from "./paths.mjs";

export const ORPHAN_GRACE_MIN = 60; // matches lost-runs.mjs's IN_FLIGHT_GRACE_MIN

// Pure: parsed `git worktree list --porcelain` entries -> which ones are
// candidate peer-review orphans, given the current time. A worktree younger
// than the grace period is left alone even if nothing is actively using it —
// it may belong to a run still in flight, and the age is the only signal
// available here (no per-worktree PR/run correlation is persisted anywhere).
export function findOrphanedWorktrees(porcelainOutput, nowMs = null) {
  const entries = [];
  let current = null;
  for (const line of String(porcelainOutput || "").split("\n")) {
    if (line.startsWith("worktree ")) {
      if (current) entries.push(current);
      current = { path: line.slice("worktree ".length).trim() };
    } else if (current && line.startsWith("branch ")) {
      current.branch = line.slice("branch ".length).trim();
    } else if (current && line === "detached") {
      current.detached = true;
    } else if (current && (line === "locked" || line.startsWith("locked "))) {
      current.locked = true;
      current.lockReason = line.slice("locked".length).trim() || null;
    }
  }
  if (current) entries.push(current);

  // Matched on the `peer-review-wt-` naming convention alone, not on a
  // tmpdir-prefix check: os.tmpdir() and the path git records for a worktree
  // can disagree on macOS, where `/tmp` is a symlink to `/private/tmp` and
  // worktree registration stores the resolved `/private/...` path while
  // os.tmpdir() returns the unresolved one — a prefix check silently matched
  // nothing here. The naming convention is specific enough on its own.
  //
  // A locked entry is excluded outright, not merely deprioritized: `git
  // worktree lock` is someone saying "I'm still using this" (single --force
  // can't remove it either — confirmed it exits 128 asking for `-f -f` — but
  // the plain, non---clean report listing it indistinguishably from a true
  // orphan is itself the failure mode this tool exists to prevent).
  const allMatching = entries.filter((e) => /peer-review-wt-/.test(e.path));
  const candidates = allMatching.filter((e) => !e.locked);
  const lockedSkipped = allMatching.filter((e) => e.locked);

  const orphans = [];
  for (const e of candidates) {
    let mtimeMs = null;
    try {
      mtimeMs = statSync(e.path).mtimeMs;
    } catch {
      // Directory is already gone but git still has it registered — a
      // classic `git worktree prune` case, still worth reporting since prune
      // is not run automatically anywhere in this harness either.
      orphans.push({ ...e, ageMinutes: null, gone: true });
      continue;
    }
    const ageMin = nowMs ? (nowMs - mtimeMs) / 60000 : null;
    if (ageMin === null || ageMin >= ORPHAN_GRACE_MIN) {
      orphans.push({
        ...e,
        ageMinutes: ageMin === null ? null : Math.round(ageMin),
        gone: false,
      });
    }
  }
  return { orphans, lockedSkipped };
}

if (isMainModule(import.meta.url)) {
  const repo = reviewRepo();
  if (!repo) {
    console.log("no review repo resolvable (set TRIBUNAL_REPO_DIR) — nothing to check");
    process.exit(0);
  }
  let porcelain;
  try {
    porcelain = execFileSync(
      "git",
      ["-C", repo, "worktree", "list", "--porcelain"],
      { encoding: "utf8" },
    );
  } catch (err) {
    console.log(`could not list worktrees for ${repo}: ${err.message}`);
    process.exit(0);
  }
  const { orphans, lockedSkipped } = findOrphanedWorktrees(porcelain, Date.now());
  if (lockedSkipped.length) {
    console.log(
      `${lockedSkipped.length} locked peer-review worktree(s) skipped (not reported as orphans, not touched):`,
    );
    for (const l of lockedSkipped)
      console.log(`  ${l.path}${l.lockReason ? ` — ${l.lockReason}` : ""}`);
  }
  if (!orphans.length) {
    console.log("no orphaned peer-review worktrees");
    process.exit(0);
  }
  console.log(`ORPHANED WORKTREES (${orphans.length}):`);
  for (const o of orphans) {
    console.log(
      `  ${o.path}  ${o.gone ? "(directory already gone, still registered)" : `${o.ageMinutes} min old`}`,
    );
  }

  if (process.argv.includes("--clean")) {
    for (const o of orphans) {
      try {
        execFileSync("git", ["-C", repo, "worktree", "remove", "--force", o.path]);
        console.log(`  removed ${o.path}`);
      } catch (err) {
        console.log(`  failed to remove ${o.path}: ${err.message}`);
      }
    }
  } else {
    console.log("\nrun with --clean to remove them (git worktree remove --force)");
  }
  process.exit(0);
}
