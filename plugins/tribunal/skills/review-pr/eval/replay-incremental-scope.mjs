#!/usr/bin/env node
// replay-incremental-scope.mjs — what a re-review has to read, pinned against a
// golden derived from git rather than from the code under test.
//
// The failure mode this guards is the one the lever ledger names: a re-review
// that narrows its scope wrongly reports FEWER problems than the first pass and
// reads as an improvement. Nothing downstream can notice — no error, no warning,
// just a shorter report. So the scope function is pinned against real patches
// with an INDEPENDENT oracle.
//
// THE GOLDEN AND WHERE IT COMES FROM. PR #1968, the last posted review
// (`0eedf726`) against the current head (`d047fa9f`), both fetched as GitHub
// three-dot compare diffs. The expected answer was computed BEFORE this code
// existed, with `awk` to split the patches and `cmp` to compare the sections —
// a different implementation of the same definition, which is what makes it an
// oracle instead of a restatement:
//
//   NEW        src/jobs/jobAttempts.ts, src/jobs/jobAttempts.test.ts
//   CHANGED    src/jobs/accounts_worker.ts, src/controllers/v2/account.controller.test.ts
//   UNCHANGED  12
//   GONE       0
//
// The load-bearing assertion is not the count — it is that `prisma/schema.prisma`
// and the migration are NOT in scope. `git diff 0eedf726 d047fa9f` lists them,
// because the branch merged `development` twice; the PR's own diff does not,
// because they belong to the base. A scope built on `sha1..sha2` would send
// lenses to read two files the author never touched, on every re-review of every
// branch that ever merges from its base.
//
// Tests the SHIPPING script, scripts/incremental-scope.mjs — the same file the
// workflow shells out to. It is a script rather than a function inside
// peer-review.js because a 65KB patch cannot cross an agent boundary without
// risking the PR #1911 truncation failure; paths go in, small JSON comes out.
//
// It carries a second implementation of `splitDiffByFile`, so the last check
// here pins the two against each other on the same fixture: drift between the
// splitter that SECTIONS the diff and the splitter that SCOPES it would quietly
// change which files get reviewed, and now it fails a test instead.
//
// Usage: node eval/replay-incremental-scope.mjs

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { scopeIncremental, splitDiffByFile as scriptSplit } from "../scripts/incremental-scope.mjs";
import { FIXTURES_DIR, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIX = join(FIXTURES_DIR, "incremental-scope");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

console.log(`\nincremental scope — scripts/incremental-scope.mjs`);

const baseline = readFileSync(join(FIX, "0eedf726.patch"), "utf8");
const current = readFileSync(join(FIX, "d047fa9f.patch"), "utf8");

console.log(`\n--- the golden: PR #1968, 0eedf726 -> d047fa9f ---`);
const s = scopeIncremental(baseline, current);
check("usable", s.usable === true, s.reason || "");
check(
  "NEW is exactly the two jobAttempts files",
  same(s.added, ["src/jobs/jobAttempts.test.ts", "src/jobs/jobAttempts.ts"]),
  JSON.stringify(s.added),
);
check(
  "CHANGED is exactly accounts_worker + account.controller.test",
  same(s.changed, [
    "src/controllers/v2/account.controller.test.ts",
    "src/jobs/accounts_worker.ts",
  ]),
  JSON.stringify(s.changed),
);
check("UNCHANGED is 12", s.unchanged.length === 12, `${s.unchanged.length}`);
check("GONE is empty", s.gone.length === 0, JSON.stringify(s.gone));
check("in scope is 4 of 16 files", s.inScope.length === 4, `${s.inScope.length}`);
// Churn is scoped too, and that matters: the caller sizes its lens budget off
// churn as well as file count, so leaking the whole PR's churn into an
// incremental run buys lenses for surface nobody will read. Golden from the
// fixture's own +/- lines, excluding the ---/+++ file markers.
// Golden from awk over the fixture's four in-scope sections: 232 of the
// patch's 1657 changed lines. Pinned exactly, not as a range — a range here
// would be a guess dressed as an assertion, and the first version of this check
// was exactly that (it asserted "< 200" and failed against the real 232).
check(
  "churn is counted over the scoped sections only (232 of 1657)",
  s.inScopeChurn === 232,
  `${s.inScopeChurn}`,
);
// The whole point of comparing PR diffs instead of sha..sha.
check(
  "merge-only files are NOT in scope",
  !s.inScope.some((f) => /schema\.prisma|migration\.sql/.test(f)),
  JSON.stringify(s.inScope),
);
// Totality: every file at head is accounted for exactly once. A path that falls
// through every bucket is a file nobody reviews and nobody carries forward.
check(
  "every file at head lands in exactly one bucket",
  s.changed.length + s.added.length + s.unchanged.length === 16,
  `${s.changed.length}+${s.added.length}+${s.unchanged.length}`,
);

console.log(`\n--- fail-open: an untrustworthy inventory must WIDEN, never narrow ---`);
for (const [name, a, b] of [
  ["empty baseline", "", current],
  ["empty current", baseline, ""],
  ["null baseline", null, current],
]) {
  const r = scopeIncremental(a, b);
  check(`${name} -> unusable`, r.usable === false && r.inScope.length === 0, r.reason);
}
// A header the splitter cannot parse means the file list is a guess. Same rule
// the sectioning applies to itself.
const corrupted = current.replace(
  /^diff --git a\/src\/jobs\/accounts_worker\.ts b\/src\/jobs\/accounts_worker\.ts$/m,
  'diff --git "a/src/jobs/weird name.ts" "b/src/jobs/weird name.ts"',
);
const c = scopeIncremental(baseline, corrupted);
check(
  "an unparsed header makes the whole scope unusable",
  c.usable === false,
  `usable=${c.usable} reason=${c.reason}`,
);

console.log(`\n--- identity: nothing changed means nothing to re-review ---`);
const idem = scopeIncremental(current, current);
check("same diff twice -> 0 in scope", idem.usable && idem.inScope.length === 0);
check("same diff twice -> everything unchanged", idem.unchanged.length === 16, `${idem.unchanged.length}`);

console.log(`\n--- direction: reversing the pair turns NEW into GONE ---`);
const rev = scopeIncremental(current, baseline);
check(
  "the two jobAttempts files are GONE, not silently dropped",
  same(rev.gone, ["src/jobs/jobAttempts.test.ts", "src/jobs/jobAttempts.ts"]),
  JSON.stringify(rev.gone),
);

console.log(`\n--- anti-drift: the two splitters must agree on the inventory ---`);
// peer-review.js sections the diff for prompts; this script splits it to decide
// scope. Two implementations of one definition is a drift risk, so it is pinned
// here rather than warned about in a comment — a prose warning does not survive
// the moment it is most needed.
const { api: regionApi } = loadRegion(WORKFLOW, "DIFF-SECTIONING", ["splitDiffByFile"]);
for (const [name, text] of [
  ["baseline", baseline],
  ["current", current],
]) {
  const mine = [...scriptSplit(text).sections.keys()].sort();
  const theirs = [...regionApi.splitDiffByFile(text).sections.keys()].sort();
  check(`${name}: same file inventory as peer-review.js`, same(mine, theirs), `${mine.length} vs ${theirs.length}`);
  const mineText = scriptSplit(text).sections.get(mine[0]);
  const theirsText = regionApi.splitDiffByFile(text).sections.get(mine[0]);
  check(`${name}: same section text for ${mine[0]}`, mineText === theirsText);
}

// The real-PR fixture cannot exercise the two cases the splitters' own comments
// call out, because #1968 contains no renames and no quoted paths — and a check
// that passes vacuously is worse than none. Verified: flipping the script to key
// on the a-side (the exact drift this section exists to catch) left the fixture
// checks above completely green. These synthetic patches are what actually catch
// it.
const RENAME = [
  "diff --git a/src/old/name.ts b/src/new/name.ts",
  "similarity index 92%",
  "rename from src/old/name.ts",
  "rename to src/new/name.ts",
  "--- a/src/old/name.ts",
  "+++ b/src/new/name.ts",
  "@@ -1,2 +1,2 @@",
  "-const a = 1;",
  "+const a = 2;",
].join("\n");
for (const [label, split] of [
  ["script", scriptSplit],
  ["peer-review.js", regionApi.splitDiffByFile],
]) {
  const keys = [...split(RENAME).sections.keys()];
  check(
    `${label}: a rename keys on the POST-image name`,
    same(keys, ["src/new/name.ts"]),
    JSON.stringify(keys),
  );
}
// A quoted header (spaces or control characters in the path) must be recorded as
// unparsed by BOTH, because one unparsed header is what collapses a run back to
// the full diff. If one splitter silently accepted it, the two would disagree
// about the file list on exactly the PR where narrowing is most dangerous.
const QUOTED = 'diff --git "a/src/weird name.ts" "b/src/weird name.ts"\n@@ -1 +1 @@\n-x\n+y';
for (const [label, split] of [
  ["script", scriptSplit],
  ["peer-review.js", regionApi.splitDiffByFile],
]) {
  const r = split(QUOTED);
  check(`${label}: a quoted header is unparsed, not guessed`, r.unparsed.length === 1 && r.sections.size === 0);
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall incremental-scope expectations hold");
