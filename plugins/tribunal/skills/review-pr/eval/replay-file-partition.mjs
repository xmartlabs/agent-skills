#!/usr/bin/env node
// replay-file-partition.mjs — churn-aware lens file partitioning.
//
// Why this needs its own replay. The file partitioning that decides which
// files go to which lens dispatch (FILE-PARTITION region) had ZERO Tier-0
// offline coverage before this change — replay-lens-budget.mjs covers the
// unrelated LENS_TOOL_CALL_BUDGET bookkeeping, replay-diff-sectioning.mjs
// covers splitDiffByFile. Neither exercises partitionFiles/
// partitionFilesByChurn at all.
//
// What this establishes: the new churn-aware partitioner (a) degrades
// byte-identically to the old round-robin when no usable churn data exists
// (strictly non-regressive), (b) actually balances total churn per group
// under a skewed fixture instead of dumping it all in one group, (c) is
// deterministic, and (d) never changes group COUNT or total file count —
// only which files land in which already-decided group.
//
// What this does NOT establish: that better balance actually narrows the
// turn-count spread across lens agents in a real run. That's Tier 1 — read
// agentCost.byKind.lens on a pinned comparison run (see
// references/metrics-schema.md, "Fields added 2026-07-30").
//
// Evaluates the REAL code sliced out of workflows/peer-review.js by its
// markers.
//
// Usage: node eval/replay-file-partition.mjs

import { resolve } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";


const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { api, violations } = loadRegion(WORKFLOW, "FILE-PARTITION", [
  "partitionFiles",
  "partitionFilesByChurn",
]);
const { partitionFiles, partitionFilesByChurn } = api;

console.log(`\nFILE-PARTITION region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

function totalChurn(group, churnMap) {
  return group.reduce((sum, f) => sum + (churnMap[f] || 0), 0);
}
function allFiles(groups) {
  return groups.flat().sort();
}

console.log(`\n--- fallback identity: no/empty churn data -> byte-identical to round-robin ---`);
{
  const files = Array.from({ length: 13 }, (_, i) => `src/file${i}.ts`);
  const groups = 4;
  const rr = partitionFiles(files, groups);
  check(
    "no churnMap arg",
    JSON.stringify(partitionFilesByChurn(files, null, groups)) === JSON.stringify(rr),
  );
  check(
    "empty churnMap object",
    JSON.stringify(partitionFilesByChurn(files, {}, groups)) === JSON.stringify(rr),
  );
  check(
    "churnMap with only non-finite values",
    JSON.stringify(
      partitionFilesByChurn(files, { "src/file0.ts": NaN, "src/file1.ts": undefined }, groups),
    ) === JSON.stringify(rr),
  );
}
{
  // A range of file counts / group counts, not just one shape.
  for (const [n, groups] of [[1, 1], [6, 1], [7, 2], [12, 3], [30, 5]]) {
    const files = Array.from({ length: n }, (_, i) => `f${i}.ts`);
    const rr = partitionFiles(files, groups);
    const cw = partitionFilesByChurn(files, null, groups);
    check(
      `fallback matches round-robin for ${n} files / ${groups} groups`,
      JSON.stringify(rr) === JSON.stringify(cw),
    );
  }
}

console.log(`\n--- balance: a skewed fixture spreads churn instead of concentrating it ---`);
{
  const bigFile = "src/huge-migration.ts";
  const tiny = Array.from({ length: 30 }, (_, i) => `src/tiny${i}.ts`);
  const files = [bigFile, ...tiny];
  const churnMap = { [bigFile]: 5000, ...Object.fromEntries(tiny.map((f) => [f, 10])) };
  const groups = 6;

  const rr = partitionFiles(files, groups);
  const cw = partitionFilesByChurn(files, churnMap, groups);

  const rrBigGroupIdx = rr.findIndex((g) => g.includes(bigFile));
  const cwBigGroupIdx = cw.findIndex((g) => g.includes(bigFile));
  check("round-robin puts the big file in some group", rrBigGroupIdx !== -1);
  check("churn-aware puts the big file in some group", cwBigGroupIdx !== -1);
  check(
    "churn-aware gives the big file's group FEWER other files than an even split",
    cw[cwBigGroupIdx].length < rr[rrBigGroupIdx].length,
    `churn-aware=${cw[cwBigGroupIdx].length} round-robin=${rr[rrBigGroupIdx].length}`,
  );

  // One 5000-churn file among 300 total churn of tiny files cannot be
  // balanced against 5 sibling groups no matter the algorithm — the correct,
  // optimal LPT outcome is to isolate it ALONE and spread every tiny file
  // evenly across the remaining groups. Assert exactly that, rather than an
  // unconditional "totals near the mean" bound that no algorithm could meet
  // for this fixture.
  check(
    "the big file's group gets NO other files (optimal isolation)",
    cw[cwBigGroupIdx].length === 1,
    `group=${JSON.stringify(cw[cwBigGroupIdx])}`,
  );
  const otherGroups = cw.filter((_, i) => i !== cwBigGroupIdx);
  const otherTotals = otherGroups.map((g) => totalChurn(g, churnMap));
  const otherSpread = Math.max(...otherTotals) - Math.min(...otherTotals);
  check(
    "the remaining (non-big-file) groups are evenly balanced among themselves",
    otherSpread <= 10, // 30 tiny files at churn 10 each across 5 groups: at most a 1-file (10-churn) imbalance
    `totals=${JSON.stringify(otherTotals)} spread=${otherSpread}`,
  );

  const rrTotals = rr.map((g) => totalChurn(g, churnMap));
  const rrSpread = Math.max(...rrTotals) - Math.min(...rrTotals);
  const cwSpread = Math.max(...cw.map((g) => totalChurn(g, churnMap))) - Math.min(...cw.map((g) => totalChurn(g, churnMap)));
  check(
    "churn-aware spread is no worse than round-robin's spread on the same skewed input",
    cwSpread <= rrSpread,
    `churn-aware spread=${cwSpread} round-robin spread=${rrSpread}`,
  );
}

console.log(`\n--- partial churn map: missing entries use the group average, not zero ---`);
{
  const files = ["a.ts", "b.ts", "c.ts", "d.ts"];
  const churnMap = { "a.ts": 1000 }; // b/c/d missing
  const groups = 2;
  const cw = partitionFilesByChurn(files, churnMap, groups);
  check("every file still placed", allFiles(cw).join(",") === [...files].sort().join(","));
  // Missing files get avgKnown (1000, since only one known value) rather than
  // 0 — so they still carry weight in the bin-packing instead of being free.
  check("no group is empty", cw.every((g) => g.length > 0), JSON.stringify(cw));
}

console.log(`\n--- determinism ---`);
{
  const files = Array.from({ length: 17 }, (_, i) => `f${i}.ts`);
  const churnMap = Object.fromEntries(files.map((f, i) => [f, (i * 37) % 500]));
  const a = JSON.stringify(partitionFilesByChurn(files, churnMap, 4));
  const b = JSON.stringify(partitionFilesByChurn(files, churnMap, 4));
  check("same input twice yields the same output", a === b);
}

console.log(`\n--- group-count invariant: only membership moves ---`);
{
  const files = Array.from({ length: 20 }, (_, i) => `f${i}.ts`);
  const churnMap = Object.fromEntries(files.map((f, i) => [f, (i * 13) % 300]));
  for (const groups of [1, 2, 3, 7]) {
    const cw = partitionFilesByChurn(files, churnMap, groups);
    check(`groups=${groups}: group count unchanged`, cw.length === groups, `got ${cw.length}`);
    check(
      `groups=${groups}: every file present exactly once`,
      allFiles(cw).join(",") === [...files].sort().join(","),
    );
  }
}

console.log(`\n--- edge cases do not throw ---`);
{
  check("empty file list", JSON.stringify(partitionFilesByChurn([], {}, 3)) === JSON.stringify([[], [], []]));
  check("null files", JSON.stringify(partitionFilesByChurn(null, {}, 3)) === JSON.stringify([[], [], []]));
  check("groups=1 short-circuits to one group with everything", (() => {
    const files = ["a.ts", "b.ts"];
    const cw = partitionFilesByChurn(files, { "a.ts": 100, "b.ts": 5 }, 1);
    return cw.length === 1 && cw[0].length === 2;
  })());
}

console.log(`
What this does NOT establish:
  · That balanced partitioning actually narrows the turn-count spread across
    lens agents in a real run. That's Tier 1 — a pinned comparison against a
    recent baseline row for the same PR/commit, reading agentCost.byKind.lens.
  · That the greedy LPT heuristic is optimal. It is simple and deterministic,
    which is what a marked region evaluated by \`new Function()\` requires;
    an optimal bin-packing was not attempted.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
