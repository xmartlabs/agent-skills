#!/usr/bin/env node
// incremental-scope.mjs — given the PR's diff at a previously reviewed commit
// and at the current head, decide which files a re-review actually has to read.
//
// Usage:
//   node scripts/incremental-scope.mjs <baseline.patch> <current.patch>
//
// Prints ONE line of JSON:
//   {usable, reason, changed[], added[], unchanged[], gone[], inScope[]}
//
// WHY A SCRIPT AND NOT A FUNCTION IN peer-review.js. The workflow runtime has no
// fs, so anything it wants from disk has to come back through an agent's return
// value — and a 65KB patch relayed that way is the PR #1911 failure verbatim:
// the harness's own truncation banner came back in place of the diff, the size
// guard passed because a truncated string is SHORT, and every lens "reviewed" a
// filesystem path. Passing PATHS in and a small JSON out is the only shape where
// the large text never crosses the boundary. Same reason
// scripts/resolve-evidence-line.mjs takes a patch path.
//
// THE INPUTS MUST BE THREE-DOT PR DIFFS (`gh api compare/<base>...<sha>`), never
// `git diff <sha1> <sha2>`, and this is the design, not a detail. Merging the
// base branch into a PR moves the merge-base, so files that only the base
// changed drop out of the PR's diff entirely — while a file whose PR-diff
// genuinely changed, semantic conflicts included, reappears. Measured on PR
// #1968 (39962991 -> 83b6ecff): `git diff sha1 sha2` lists 6 files, two of them
// merge noise; comparing PR diffs lists the 4 the author actually touched.
//
// Exit code is always 0. A caller that cannot narrow safely must fall back to a
// FULL review, and `usable: false` says so in-band — a crash would make the
// caller guess.

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

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

// A deliberate second implementation of peer-review.js's `splitDiffByFile`, kept
// small and pinned against it: eval/replay-incremental-scope.mjs asserts both
// produce the SAME file inventory on the same fixture, so drift between them
// fails a test instead of quietly changing what gets reviewed.
export function splitDiffByFile(diff) {
  const sections = new Map();
  const unparsed = [];
  if (!diff) return { sections, unparsed };
  let path = null;
  let buf = [];
  const flush = () => {
    if (path === null) return;
    const prev = sections.get(path);
    const text = buf.join("\n");
    sections.set(path, prev ? `${prev}\n${text}` : text);
  };
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) {
      flush();
      buf = [line];
      // Take the b-side (the post-image name, what a rename reports). Git quotes
      // paths containing spaces or control characters; a header that does not
      // match this shape is RECORDED as unparsed rather than guessed at, and one
      // unparsed header invalidates the whole scope — see below.
      const m = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
      if (m) {
        path = m[2];
      } else {
        path = null;
        unparsed.push(line);
      }
      continue;
    }
    if (path !== null) buf.push(line);
  }
  flush();
  return { sections, unparsed };
}

export function scopeIncremental(baselineDiff, currentDiff) {
  const unusable = (reason) => ({
    usable: false,
    reason,
    changed: [],
    added: [],
    unchanged: [],
    gone: [],
    inScope: [],
    inScopeChurn: 0,
  });
  if (!baselineDiff || !currentDiff) return unusable("a diff was empty");
  const prev = splitDiffByFile(baselineDiff);
  const curr = splitDiffByFile(currentDiff);
  // An inventory we could not fully parse is a guess, and a guess must WIDEN a
  // review, never narrow it. Same rule peer-review.js already applies before it
  // will section a diff at all.
  if (prev.unparsed.length || curr.unparsed.length)
    return unusable(
      `unparsed diff header(s): ${prev.unparsed.length} baseline, ${curr.unparsed.length} current`,
    );
  if (!prev.sections.size || !curr.sections.size)
    return unusable("a diff produced no sections");

  const changed = [];
  const added = [];
  const unchanged = [];
  const gone = [];
  // Byte comparison of the SECTION, not of the file: a file can be rewritten
  // upstream and still carry an identical PR diff, and that file needs no second
  // look. What this cannot see is a change OUTSIDE the PR's diff that breaks a
  // file inside it — which is why the caller narrows lens ASSIGNMENT only, and
  // leaves reading rights at HEAD unrestricted.
  for (const [path, text] of curr.sections) {
    if (!prev.sections.has(path)) added.push(path);
    else if (prev.sections.get(path) !== text) changed.push(path);
    else unchanged.push(path);
  }
  for (const path of prev.sections.keys()) if (!curr.sections.has(path)) gone.push(path);

  const sorted = (a) => a.sort((x, y) => x.localeCompare(y));
  const inScope = sorted([...changed, ...added]);
  // Changed lines inside the scoped sections only. The caller sizes its lens
  // budget off churn as well as file count, and handing it the WHOLE PR's churn
  // on an incremental run would buy lenses for surface nobody is going to read —
  // the exact waste this feature exists to remove.
  const churnOf = (paths) =>
    paths.reduce((n, p) => {
      const body = curr.sections.get(p) || "";
      return (
        n +
        body
          .split("\n")
          .filter((l) => /^[+-]/.test(l) && !/^(\+\+\+|---)/.test(l)).length
      );
    }, 0);
  return {
    usable: true,
    reason: null,
    inScopeChurn: churnOf(inScope),
    changed: sorted(changed),
    added: sorted(added),
    unchanged: sorted(unchanged),
    // A file that LEFT the PR's diff is neither reviewable nor carryable: a
    // prior finding on it is reported as moot, never silently dropped.
    gone: sorted(gone),
    inScope,
  };
}

if (isMainModule()) {
  const [, , baselinePath, currentPath] = process.argv;
  const read = (p) => {
    try {
      return readFileSync(p, "utf8");
    } catch {
      return "";
    }
  };
  const result =
    baselinePath && currentPath
      ? scopeIncremental(read(baselinePath), read(currentPath))
      : {
          usable: false,
          reason: "usage: incremental-scope.mjs <baseline.patch> <current.patch>",
          changed: [],
          added: [],
          unchanged: [],
          gone: [],
          inScope: [],
        };
  console.log(JSON.stringify(result));
}
