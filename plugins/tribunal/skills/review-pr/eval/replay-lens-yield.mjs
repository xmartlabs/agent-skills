#!/usr/bin/env node
// replay-lens-yield.mjs — per-lens yield, sliced out of the shipping workflow.
//
// WHY IT EXISTS. A row carried `lensCount: 2` and nothing else about the lenses,
// so a lens that looked and found nothing was indistinguishable from a lens that
// never answered — and the last three recorded rows are all exactly that
// ambiguous shape (`lensCount: 2, findings: 0`). Answering "is this lens earning
// its dispatch?" meant reopening 250KB transcripts.
//
// The two properties that are easy to get wrong and are pinned here:
//   · a zero-yield lens must APPEAR, with `candidates: 0`. Omitting it is how the
//     data silently becomes "only lenses that found something exist".
//   · a merged finding must credit every contributing lens, which makes
//     `survivedToReport` an attribution that can exceed the report's finding
//     count. A future reader comparing it to `findings.length` would be wrong;
//     the region says so and this file proves the sum really can exceed it.
//
// Usage: node eval/replay-lens-yield.mjs

import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nlens yield — LENS-YIELD region of peer-review.js`);

const { api, violations } = loadRegion(WORKFLOW, "LENS-YIELD", ["buildLensYield"]);
const { buildLensYield } = api;
check("the region is pure (no run state, no dispatch)", violations.length === 0, violations.join("; "));

const byName = (rows) => Object.fromEntries(rows.map((r) => [r.lens, r]));

console.log(`\n--- a lens that found nothing is present, not absent ---`);
{
  const rows = buildLensYield(
    [
      { lens: "correctness", files: null },
      { lens: "dataContract", files: null },
    ],
    [{ findings: [{}] }, { findings: [] }],
    [{ lens: "correctness" }],
    [{ lens: "correctness", mergedFrom: [], survived: true }],
  );
  check("one entry per planned lens", rows.length === 2, JSON.stringify(rows));
  const m = byName(rows);
  check("the zero-yield lens appears", !!m.dataContract, JSON.stringify(rows));
  check("...with candidates 0", m.dataContract.candidates === 0, JSON.stringify(m.dataContract));
  check("...and is NOT marked crashed", m.dataContract.crashed === 0, JSON.stringify(m.dataContract));
  check("the productive lens is credited", m.correctness.survivedToReport === 1, JSON.stringify(m.correctness));
}

console.log(`\n--- crashed is not the same as found-nothing ---`);
{
  const rows = buildLensYield(
    [
      { lens: "silent", files: null },
      { lens: "dead", files: null },
    ],
    [{ findings: [] }, null],
    [],
    [],
  );
  const m = byName(rows);
  check("the lens that answered empty has crashed 0", m.silent.crashed === 0, JSON.stringify(m.silent));
  check("the lens that never answered has crashed 1", m.dead.crashed === 1, JSON.stringify(m.dead));
  check(
    "both have candidates 0, so crashed is the ONLY thing separating them",
    m.silent.candidates === 0 && m.dead.candidates === 0,
    JSON.stringify(rows),
  );
}

console.log(`\n--- a merged finding credits every contributor ---`);
{
  const reported = [
    { lens: "correctness", mergedFrom: ["dataContract"], survived: true },
  ];
  const rows = buildLensYield(
    [
      { lens: "correctness", files: null },
      { lens: "dataContract", files: null },
    ],
    [{ findings: [{}] }, { findings: [{}] }],
    [{ lens: "correctness" }, { lens: "dataContract" }],
    reported,
  );
  const m = byName(rows);
  check("the headline lens is credited", m.correctness.survivedToReport === 1, JSON.stringify(m.correctness));
  check("the merged-away lens is credited too", m.dataContract.survivedToReport === 1, JSON.stringify(m.dataContract));
  const sum = rows.reduce((a, r) => a + r.survivedToReport, 0);
  check(
    "the column is an attribution: it exceeds the report's finding count",
    sum === 2 && reported.length === 1,
    `sum=${sum} reported=${reported.length}`,
  );
  const dupe = buildLensYield(
    [{ lens: "correctness", files: null }],
    [{ findings: [{}] }],
    [{ lens: "correctness" }],
    [{ lens: "correctness", mergedFrom: ["correctness"], survived: true }],
  );
  check(
    "a lens listed in its own mergedFrom is credited once, not twice",
    dupe[0].survivedToReport === 1,
    JSON.stringify(dupe),
  );
}

console.log(`\n--- only survivors are credited ---`);
{
  const rows = buildLensYield(
    [{ lens: "correctness", files: null }],
    [{ findings: [{}, {}, {}] }],
    [{ lens: "correctness" }, { lens: "correctness" }, { lens: "correctness" }],
    [
      { lens: "correctness", mergedFrom: [], survived: true },
      { lens: "correctness", mergedFrom: [], survived: false },
      { lens: "correctness", mergedFrom: [], survived: undefined },
    ],
  );
  check("three candidates raised", rows[0].candidates === 3, JSON.stringify(rows[0]));
  check("one survivor credited", rows[0].survivedToReport === 1, JSON.stringify(rows[0]));
}

console.log(`\n--- a repeated focus is one entry with two dispatches ---`);
// lensCount above the focus list dispatches the same focus twice; both tag their
// candidates identically, so no attribution can split them. Reporting two entries
// would have each claiming the other's work.
{
  const rows = buildLensYield(
    [
      { lens: "correctness", files: ["a.ts", "b.ts"] },
      { lens: "correctness", files: ["c.ts"] },
    ],
    [{ findings: [{}] }, null],
    [{ lens: "correctness" }, { lens: "correctness" }],
    [],
  );
  check("one entry, not two", rows.length === 1, JSON.stringify(rows));
  check("dispatches counts both", rows[0].dispatches === 2, JSON.stringify(rows[0]));
  check("files sums across both", rows[0].files === 3, JSON.stringify(rows[0]));
  check("crashed counts the dead one only", rows[0].crashed === 1, JSON.stringify(rows[0]));
  check("candidates are pooled, not double-counted", rows[0].candidates === 2, JSON.stringify(rows[0]));
}

console.log(`\n--- files: null on an unpartitioned run, a count when scoped ---`);
{
  const un = buildLensYield([{ lens: "a", files: null }], [{ findings: [] }], [], []);
  check("unpartitioned is null, not 0", un[0].files === null, JSON.stringify(un[0]));
  const sc = buildLensYield([{ lens: "a", files: ["x", "y"] }], [{ findings: [] }], [], []);
  check("scoped is the file count", sc[0].files === 2, JSON.stringify(sc[0]));
  const empty = buildLensYield([{ lens: "a", files: [] }], [{ findings: [] }], [], []);
  check("an empty scope is 0, which is a real claim", empty[0].files === 0, JSON.stringify(empty[0]));
}

console.log(`\n--- notReached comes from the lens's own report ---`);
{
  const rows = buildLensYield(
    [{ lens: "a", files: null }, { lens: "b", files: null }],
    [{ findings: [], notReached: ["x.ts", "y.ts"] }, null],
    [],
    [],
  );
  const m = byName(rows);
  check("counted for the lens that reported it", m.a.notReached === 2, JSON.stringify(m.a));
  check("a crashed lens reports 0, not a guess", m.b.notReached === 0, JSON.stringify(m.b));
}

console.log(`\n--- findings from outside the plan invent no rows ---`);
{
  const rows = buildLensYield(
    [{ lens: "correctness", files: null }],
    [{ findings: [{}] }],
    [{ lens: "correctness" }, { lens: "bot-comments" }],
    [
      { lens: "bot-comments", mergedFrom: [], survived: true },
      { lens: "unknown", mergedFrom: [], survived: true },
    ],
  );
  check("still one entry", rows.length === 1, JSON.stringify(rows));
  check("the off-plan candidate is not counted", rows[0].candidates === 1, JSON.stringify(rows[0]));
  check("the off-plan survivors credit nobody", rows[0].survivedToReport === 0, JSON.stringify(rows[0]));
}

console.log(`\n--- degenerate input degrades, never throws ---`);
{
  check("everything absent yields []", JSON.stringify(buildLensYield()) === "[]");
  check("nulls yield []", JSON.stringify(buildLensYield(null, null, null, null)) === "[]");
  const noLabel = buildLensYield([{ files: null }], [null], [], []);
  check("a plan entry with no label gets a positional one", noLabel[0].lens === "lens1", JSON.stringify(noLabel));
  const junk = buildLensYield(
    [{ lens: "a", files: null }],
    "not-an-array",
    [null, { lens: "a" }, 7],
    [null, { survived: true }, { lens: "a", mergedFrom: "nope", survived: true }],
  );
  check("junk in the arrays is skipped", junk[0].candidates === 1, JSON.stringify(junk[0]));
  check("a non-array mergedFrom does not throw", junk[0].survivedToReport === 1, JSON.stringify(junk[0]));
  check("a non-array results list marks every lens crashed", junk[0].crashed === 1, JSON.stringify(junk[0]));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall lens-yield expectations hold");
