#!/usr/bin/env node
// replay-availability-check-gate.mjs — workflows/tribunal.js's
// AVAILABILITY-CHECK-GATE region, run against real inputs.
//
// Regression cover for the 2026-08-18 fix (PR #2016, real Workflow run):
// `wantsAvailabilityCheck` used to be gated purely on
// `anyCandidateToRefute = panelCandidates.length > 0`. That skipped the
// codex/gemini CLI probe on every zero-candidate run — exactly the run shape
// `shouldRunCleanSweep` (CLEAN-SWEEP region) exists for — so `codexAvailable`
// stayed at its `false` default and the sweep always reported
// `codex-not-installed`, whether or not Codex was actually installed. On
// PR #2016 Codex was confirmed installed and healthy; the persisted row still
// read `codexEnabled: false, cleanSweep: {skippedReason: "codex-not-installed"}`.
//
// This file fault-injects the bug: it hand-computes what the OLD formula
// would have produced for the exact PR #2016 shape (0 candidates, Codex not
// force-disabled) and asserts that formula gives `false` — the bug — before
// asserting the shipped region gives `true` — the fix. A check that never
// reproduces the fault it claims to catch is not verification.
//
// Usage: node eval/replay-availability-check-gate.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

const { api } = loadRegion(WORKFLOW, "AVAILABILITY-CHECK-GATE", [
  "computeWantsAvailabilityCheck",
]);
const { computeWantsAvailabilityCheck } = api;

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- fault injection: reproduce the PR #2016 bug shape, then the fix -------

{
  // The exact shape that broke on PR #2016: zero panel candidates, Codex not
  // force-disabled (no --no-codex passed), Gemini's own flag irrelevant.
  const anyCandidateToRefute = false;
  const noCodex = false;
  const noGemini = true; // Gemini is default-off; PR #2016 ran with it off too.

  // The OLD formula, hand-reproduced (this is what shipped before the fix):
  const oldWantsAvailabilityCheck =
    (!noCodex || !noGemini) && anyCandidateToRefute;
  check(
    "the bug reproduces: the OLD formula skips the probe on PR #2016's exact shape",
    oldWantsAvailabilityCheck === false,
    `old formula gave ${oldWantsAvailabilityCheck}, expected false (the bug)`,
  );

  const { wantsAvailabilityCheck } = computeWantsAvailabilityCheck(
    anyCandidateToRefute,
    noCodex,
    noGemini,
  );
  check(
    "the fix holds: the shipped region runs the probe on PR #2016's exact shape",
    wantsAvailabilityCheck === true,
    `got ${wantsAvailabilityCheck}`,
  );
}

// --- candidates present: behavior must be UNCHANGED from the old formula ---

{
  const cases = [
    ["codex on, gemini on", false, false, true],
    ["codex off, gemini on", true, false, true],
    ["codex on, gemini off", false, true, true],
    ["both off", true, true, false],
  ];
  for (const [label, noCodex, noGemini, expected] of cases) {
    const oldFormula = (!noCodex || !noGemini) && true; // anyCandidateToRefute=true
    check(
      `pre-existing behavior preserved when a candidate exists (${label}): old formula itself gives ${expected}`,
      oldFormula === expected,
    );
    const { wantsAvailabilityCheck } = computeWantsAvailabilityCheck(
      true,
      noCodex,
      noGemini,
    );
    check(
      `candidates present, ${label} -> wantsAvailabilityCheck === ${expected}`,
      wantsAvailabilityCheck === expected,
      `got ${wantsAvailabilityCheck}`,
    );
  }
}

// --- zero candidates: the fixed branch, every noCodex/noGemini combination -

{
  const cases = [
    ["codex on, gemini on (the sweep needs codex, gemini is irrelevant)", false, false, true],
    ["codex on, gemini off", false, true, true],
    ["codex off (--no-codex): no sweep is possible, do not probe", true, false, false],
    ["codex off, gemini off", true, true, false],
  ];
  for (const [label, noCodex, noGemini, expected] of cases) {
    const { wantsAvailabilityCheck, wantsCleanSweepCheck } =
      computeWantsAvailabilityCheck(false, noCodex, noGemini);
    check(
      `zero candidates, ${label} -> wantsAvailabilityCheck === ${expected}`,
      wantsAvailabilityCheck === expected,
      `got ${wantsAvailabilityCheck}`,
    );
    check(
      `zero candidates, ${label} -> wantsCleanSweepCheck matches its own definition (!anyCandidateToRefute && !noCodex)`,
      wantsCleanSweepCheck === !noCodex,
      `got ${wantsCleanSweepCheck}`,
    );
  }
}

// --- wantsCleanSweepCheck is false whenever a candidate exists, regardless -

{
  for (const [noCodex, noGemini] of [
    [false, false],
    [true, false],
    [false, true],
    [true, true],
  ]) {
    const { wantsCleanSweepCheck } = computeWantsAvailabilityCheck(
      true,
      noCodex,
      noGemini,
    );
    check(
      `candidates present (noCodex=${noCodex}, noGemini=${noGemini}) -> wantsCleanSweepCheck === false`,
      wantsCleanSweepCheck === false,
      `got ${wantsCleanSweepCheck}`,
    );
  }
}

// --- degenerate input never throws ------------------------------------------

{
  let threw = false;
  try {
    computeWantsAvailabilityCheck(undefined, undefined, undefined);
  } catch {
    threw = true;
  }
  check("undefined inputs do not throw", !threw);
}

console.log(
  `\navailability-check-gate: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`,
);
process.exit(failures.length ? 1 : 0);
