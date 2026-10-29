#!/usr/bin/env node
// replay-vote-field-coverage.mjs — every vote field the panel GATE reads must
// survive into the row, or be explicitly accounted for somewhere else.
//
// WHY IT EXISTS. On 2026-07-27, PR #1923 produced a row that could not explain
// its own behaviour: the cost breaker logged a declined `refine` on a finding
// whose persisted votes all read `coreStands: false` with `borderline: false` —
// precisely the state in which `needsRefine` returns false. The gate was correct
// and the log was correct. The ROW was missing the input that decided it:
// `tally.coreStands` is `coreStands || refutationType === "TECHNICALLY_WRONG"`,
// and `refutationType` — a REQUIRED field of the vote schema that drives three
// separate decisions — was persisted nowhere.
//
// That is the second time this exact shape has cost real analysis:
// `touchesHighRiskSurface` and `isBehaviorChanging` were `undefined` in 257 of 258
// recorded findings while four mechanisms decided on them. Twice is a pattern, so
// this is the mechanical version of "do not drop a gate's inputs".
//
// Usage: node eval/replay-vote-field-coverage.mjs

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
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

console.log(`\nvote-field coverage — what the gate reads vs what the row keeps`);

// --- What tallyVotes reads off each vote, from the shipping region.
const { regions } = loadRegion(WORKFLOW, "PANEL-GATES", ["tallyVotes"]);
const regionSrc = regions.join("\n");
const start = regionSrc.indexOf("function tallyVotes");
const body = regionSrc.slice(start, regionSrc.indexOf("\n}", start));
const read = [...new Set([...body.matchAll(/\bv\.([A-Za-z_$][\w$]*)/g)].map((m) => m[1]))].sort();

// --- What the persisted row keeps per vote. Anchored on the mapping that carries
// `round:` — there is a second `votes: v.votes.map(...)` that builds a PROMPT and
// deliberately carries only three fields; matching that one would make this check
// assert the wrong thing.
const src = readFileSync(WORKFLOW, "utf8");
let persisted = [];
for (const m of src.matchAll(/votes: v\.votes\.map\(\(vote\) => \(\{([\s\S]*?)\}\)\)/g)) {
  const block = m[1];
  if (!/\bround:/.test(block)) continue;
  persisted = [...new Set([...block.matchAll(/^\s{6}([A-Za-z_$][\w$]*):/gm)].map((x) => x[1]))].sort();
}

// --- Fields the gate reads that legitimately do NOT belong in votes[].
// Both describe a slot that ABSTAINED, and an abstaining slot is filtered out of
// the persisted votes list by design — it is recorded in failedSlots /
// failedSlotsDetail instead. Listing them here rather than silently ignoring
// anything unknown is the difference between an exception and a hole.
const ELSEWHERE = {
  slotStatus: "abstaining slots are filtered out of votes[]; recorded in failedSlotsDetail",
  failureReason: "same — the WHY of an abstention lives in failedSlotsDetail",
};

console.log(`\n--- the check is not vacuous ---`);
check("the gate reads at least one vote field", read.length > 0, read.join(","));
check("the row persists at least one vote field", persisted.length > 0, persisted.join(","));
console.log(`        gate reads:  ${read.join(", ")}`);
console.log(`        row keeps:   ${persisted.join(", ")}`);

console.log(`\n--- every gate input survives into the row ---`);
for (const f of read) {
  if (ELSEWHERE[f]) {
    check(`${f} is accounted for elsewhere`, true);
    console.log(`        (${ELSEWHERE[f]})`);
    continue;
  }
  check(
    `${f} is persisted on the row's votes[]`,
    persisted.includes(f),
    "the gate decides on it and the row cannot reproduce that decision",
  );
}

console.log(`\n--- the exception list does not rot ---`);
// If a field on the list starts being persisted, the entry is stale and should be
// deleted. An exception nobody revisits is how the next gap hides.
for (const f of Object.keys(ELSEWHERE)) {
  check(
    `${f} is still genuinely absent from votes[]`,
    !persisted.includes(f),
    "it IS persisted now — remove it from ELSEWHERE rather than leaving a false exception",
  );
}

console.log(`\n--- the field that caused this file is present ---`);
// A named regression guard, not just a rule: refutationType is the one that was
// missing, and its absence is what made a row unable to explain a declined refine.
check("refutationType is persisted", persisted.includes("refutationType"));
check("coreStands is persisted", persisted.includes("coreStands"));

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nno gate input is dropped from the row");
