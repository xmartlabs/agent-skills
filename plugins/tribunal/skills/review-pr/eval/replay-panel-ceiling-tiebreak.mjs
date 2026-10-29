#!/usr/bin/env node
// replay-panel-ceiling-tiebreak.mjs — workflows/tribunal.js's
// PANEL-CEILING-TIEBREAK region, run against real inputs.
//
// A DISTINCT concern from eval/replay-intake.mjs. That file tests who faces a
// panel and needsPanel/panelPriority's own ranking (re-derived via a
// regex-lift, not loadRegion — panelPriority closes over run state, so it
// stays outside any marked region). This file tests something else: what
// happens AT the boundary once ranking is done — whether an exact tie in
// panelPriority right at the cutoff gets a real judgment call instead of the
// arbitrary emission-order tiebreak, and whether a malformed/incomplete
// agent response ever crashes or drops a finding (it must not — the
// fallback is the original, already-tested emission order, by construction).
//
// Two things NOT establishable offline, named rather than silently skipped:
// the cap-triggered skip-and-log, and the real agent() dispatch's ranking
// quality — both need a live/Tier-1 run or a real PR with >12 tied findings.
//
// Usage: node eval/replay-panel-ceiling-tiebreak.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

const { api } = loadRegion(WORKFLOW, "PANEL-CEILING-TIEBREAK", [
  "panelCeilingTiedGroup",
  "resolvePanelCeilingTiebreak",
  "PANEL_TIEBREAK_CAP",
]);
const { panelCeilingTiedGroup, resolvePanelCeilingTiebreak, PANEL_TIEBREAK_CAP } = api;

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- panelCeilingTiedGroup ---------------------------------------------------

{
  // Real boundary tie: priorities [8,4,4,4,2], maxPanels=3.
  const eligible = [
    { slotId: 1, priority: 8, i: 0 },
    { slotId: 2, priority: 4, i: 1 },
    { slotId: 3, priority: 4, i: 2 },
    { slotId: 4, priority: 4, i: 3 },
    { slotId: 5, priority: 2, i: 4 },
  ];
  const diag = panelCeilingTiedGroup(eligible, 3);
  check("boundaryPriority is the value at the cutoff", diag.boundaryPriority === 4, JSON.stringify(diag));
  check(
    "guaranteedIds holds only the strictly-higher-priority finding",
    JSON.stringify(diag.guaranteedIds) === JSON.stringify([1]),
    JSON.stringify(diag),
  );
  check(
    "tiedGroup holds every finding at the boundary priority",
    diag.tiedGroup.length === 3 && diag.tiedGroup.every((f) => f.priority === 4),
    JSON.stringify(diag),
  );
  check("slotsForTiedGroup accounts for the guaranteed slot", diag.slotsForTiedGroup === 2, JSON.stringify(diag));
}

{
  // No ceiling bite: eligible pool fits inside maxPanels.
  const eligible = [
    { slotId: 1, priority: 8, i: 0 },
    { slotId: 2, priority: 4, i: 1 },
  ];
  check(
    "returns null when the ceiling never bites",
    panelCeilingTiedGroup(eligible, 5) === null,
  );
  check(
    "returns null when the pool exactly fills the ceiling",
    panelCeilingTiedGroup(eligible, 2) === null,
  );
}

{
  // Tie exists AND the ceiling genuinely bites (a lower tier is excluded),
  // but the boundary tier itself is fully accommodated — no real contest
  // within it, caller should not dispatch. priorities [8,4,4,1], maxPanels=3:
  // the priority-1 finding is dropped, but both priority-4 findings fit.
  const eligible = [
    { slotId: 1, priority: 8, i: 0 },
    { slotId: 2, priority: 4, i: 1 },
    { slotId: 3, priority: 4, i: 2 },
    { slotId: 4, priority: 1, i: 3 },
  ];
  const diag = panelCeilingTiedGroup(eligible, 3);
  check(
    "a tie that fits exactly reports tiedGroup.length === slotsForTiedGroup (caller's cue to skip the dispatch)",
    diag !== null && diag.tiedGroup.length === diag.slotsForTiedGroup,
    JSON.stringify(diag),
  );
}

// --- resolvePanelCeilingTiebreak --------------------------------------------

{
  const tiedGroup = [{ slotId: 2 }, { slotId: 3 }, { slotId: 4 }];
  const r = resolvePanelCeilingTiebreak(tiedGroup, 2, [4, 2, 3]);
  check(
    "a valid full permutation splits winners/losers at slotsForTiedGroup",
    r.ok === true &&
      JSON.stringify(r.winningIds) === JSON.stringify([4, 2]) &&
      JSON.stringify(r.losingIds) === JSON.stringify([3]),
    JSON.stringify(r),
  );
}

{
  const tiedGroup = [{ slotId: 2 }, { slotId: 3 }, { slotId: 4 }];
  const malformed = [
    ["missing a tied id", [4, 2]],
    ["extra id not in the tied group", [4, 2, 3, 99]],
    ["duplicate id", [4, 4, 3]],
    ["null order", null],
    ["undefined order", undefined],
    ["non-array order", "nope"],
    ["empty array when a real tie exists", []],
  ];
  let allDegradedSafely = true;
  for (const [label, order] of malformed) {
    try {
      const r = resolvePanelCeilingTiebreak(tiedGroup, 2, order);
      if (r.ok !== false || typeof r.reason !== "string") {
        allDegradedSafely = false;
        console.log(`        unexpected result for "${label}": ${JSON.stringify(r)}`);
      }
    } catch (err) {
      allDegradedSafely = false;
      console.log(`        threw for "${label}": ${err.message}`);
    }
  }
  check(
    "every malformed/incomplete agent response degrades to {ok:false}, never throws",
    allDegradedSafely,
  );
}

// --- the cap is pinned, not silently drifting -------------------------------

check(
  `PANEL_TIEBREAK_CAP is pinned to its current shipped value (12)`,
  PANEL_TIEBREAK_CAP === 12,
  `got ${PANEL_TIEBREAK_CAP} — if this changed on purpose, update this assertion in the same commit`,
);

console.log(
  `\npanel-ceiling-tiebreak: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`,
);
process.exit(failures.length ? 1 : 0);
