#!/usr/bin/env node
// replay-lens-budget.mjs — the lens tool-call ceiling's bookkeeping.
//
// Why this needs its own replay. The budget itself is a prompt instruction and
// nothing offline can test whether a model obeys it. What CAN break offline is
// the plumbing around it, in the Review phase, which is the most expensive place
// in the run to throw an exception: a crashed lens arrives as `null`, a lens that
// reached everything omits `notReached` entirely, and both must pass through
// without an error. A budget change that killed the Review phase would be a
// spectacular own goal.
//
// The second guarantee is about honesty rather than crashes. `notChecked` is the
// field a reader consults to ask "what did this review miss?", and the budget
// creates a NEW way to miss things. If a lens's self-reported gap failed to reach
// that array, the row would claim full coverage of a pass that ran out of room —
// which is worse than the gap.
//
// Evaluates the REAL code sliced out of workflows/peer-review.js by its markers.
//
// Usage: node eval/replay-lens-budget.mjs

import { resolve } from "node:path";
import { readFileSync } from "node:fs";
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

const { api, violations } = loadRegion(WORKFLOW, "LENS-BUDGET", [
  "collectNotReached",
  "mergeNotChecked"
]);
const { collectNotReached, mergeNotChecked } = api;

console.log(`\nLENS-BUDGET region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- collectNotReached: survives every result shape ---`);
{
  const results = [
    { findings: [], notReached: ["src/a.ts", "src/b.ts"] },
    { findings: [] },
    { findings: [], notReached: ["src/b.ts"] }
  ];
  const got = collectNotReached(results, 3);
  check("collects across lenses", got.includes("src/a.ts") && got.includes("src/b.ts"));
  check("deduplicates a surface two lenses both skipped", got.length === 2, JSON.stringify(got));
  check("a lens with no notReached contributes nothing", !got.includes("undefined"));
}
{
  // The one that matters: a crashed lens is null in the array.
  const got = collectNotReached([null, { findings: [], notReached: ["x.ts"] }, undefined], 3);
  check("a null/undefined lens result does not throw", got.length === 1 && got[0] === "x.ts");
}
{
  check("an empty results array yields nothing", collectNotReached([], 2).length === 0);
  check("a null results array does not throw", collectNotReached(null, 2).length === 0);
  check("undefined results does not throw", collectNotReached(undefined, 2).length === 0);
  check("lensCount 0 yields nothing", collectNotReached([{ notReached: ["a"] }], 0).length === 0);
}
{
  // bot-vetting sits after the lenses and has a different schema. Reading past
  // lensCount would mix its output into a coverage claim about the lenses.
  const results = [{ notReached: ["lens.ts"] }, { comments: [], notReached: ["bot.ts"] }];
  const got = collectNotReached(results, 1);
  check("results past lensCount are ignored", got.length === 1 && got[0] === "lens.ts", JSON.stringify(got));
}
{
  // A model returning the wrong type for an optional field must not crash the
  // phase, and must not contribute a garbage entry either.
  check("a string notReached is ignored, not spread into characters", collectNotReached([{ notReached: "src/a.ts" }], 1).length === 0);
  check("a numeric notReached is ignored", collectNotReached([{ notReached: 7 }], 1).length === 0);
  check("an object notReached is ignored", collectNotReached([{ notReached: { a: 1 } }], 1).length === 0);
}
{
  const got = collectNotReached([{ notReached: ["  src/a.ts  ", "", "   ", null, undefined, "src/b.ts"] }], 1);
  check("entries are trimmed", got.includes("src/a.ts"));
  check("blank and null entries are dropped", got.length === 2, JSON.stringify(got));
}

console.log(`\n--- mergeNotChecked: both kinds of blind spot reach the row ---`);
{
  const got = mergeNotChecked(["out-of-scope.ts"], ["ran-out-of-budget.ts"]);
  check("a harness-derived gap survives", got.includes("out-of-scope.ts"));
  check("a lens-reported gap survives", got.includes("ran-out-of-budget.ts"));
  check("both, not one", got.length === 2);
}
{
  const got = mergeNotChecked(["same.ts"], ["same.ts"]);
  check("a file that is both out of scope and unreached appears once", got.length === 1);
}
{
  check("no gaps at all yields an empty array, not null", Array.isArray(mergeNotChecked([], [])) && mergeNotChecked([], []).length === 0);
  check("null uncovered does not throw", mergeNotChecked(null, ["a"]).length === 1);
  check("null notReached does not throw", mergeNotChecked(["a"], null).length === 1);
  check("both null does not throw", mergeNotChecked(null, null).length === 0);
  check("blank entries are dropped", mergeNotChecked(["", "a"], [""]).length === 1);
  // Variadic since the incremental re-review added a third kind of blind spot
  // (files deliberately not re-read). A third argument to a two-argument
  // function is dropped SILENTLY by JS, and the row would then claim coverage of
  // files nobody opened — the exact claim this field exists to refuse.
  check(
    "a THIRD list reaches the row (incremental: files not re-read)",
    mergeNotChecked(["a.ts"], ["b.ts"], ["c.ts (unchanged — not re-read)"]).length === 3,
  );
  check("a fourth list would too", mergeNotChecked(["a"], ["b"], ["c"], ["d"]).length === 4);
}

console.log(`\n--- the budget constant is stated, and is a ceiling not a floor ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const m = /const LENS_TOOL_CALL_BUDGET = (\d+);/.exec(src);
  check("the budget is a named constant", !!m, "LENS_TOOL_CALL_BUDGET not found");
  const budget = m ? Number(m[1]) : 0;
  // Measured within PR #1911: lenses under 30 turns returned 0.20 findings. A
  // budget at or below that is not a saving, it is a broken review — so the
  // number itself is pinned against the floor the data established.
  check(`budget (${budget}) is above the 30-turn failure floor`, budget > 30, `${budget}`);
  check(`budget (${budget}) is at or below the 50-turn knee`, budget <= 50, `${budget}`);
  const instr = /const LENS_BUDGET_INSTRUCTION = `([\s\S]*?)`;/.exec(src);
  check("the instruction exists", !!instr);
  const text = instr ? instr[1] : "";
  // Both halves have to be in the prompt. "Aim under 40" alone reads as "stop at
  // 40", and the 0-30 bucket is what that produces.
  check("the instruction says it is not a reason to stop early", /not a reason to stop early/i.test(text));
  check("the instruction interpolates the constant, not a hardcoded number", /\$\{LENS_TOOL_CALL_BUDGET\}/.test(text));
  check("the instruction tells the lens where to report what it skipped", /notReached/.test(text));
  check("the row records the budget it ran under", /lensToolCallBudget: LENS_TOOL_CALL_BUDGET/.test(src));
  check("the row separates budget skips from partition gaps", /lensBudgetSkips: lensNotReached\.length/.test(src));
  check("notReached is in the findings schema", /notReached: \{ type: "array"/.test(src));
}

console.log(`
What this does NOT establish:
  · That a lens OBEYS the budget. It is a prompt instruction with no enforcement
    mechanism, by design — there is no way to cap an agent's tool calls, and a
    prompt claiming otherwise would simply be false. Whether it lands is Tier 1:
    read agentCost.byKind.lens.turns on the next runs.
  · That obeying it is free. The evidence is observational — different agents in
    different turn bands on one PR, not the same agent truncated. A lens stopped
    at 40 may not deliver what the 50-80 band delivered. The 80+ band finding
    slightly FEWER defects than the 50-80 band is the strongest argument that the
    marginal turns were not buying anything, and it is still not a controlled test.
  · That the number 40 is optimal. It sits between a measured failure floor (30)
    and a measured efficiency knee (50) on ONE PR's 71 lenses.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
