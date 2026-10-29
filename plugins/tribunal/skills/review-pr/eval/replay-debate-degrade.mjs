#!/usr/bin/env node
// replay-debate-degrade.mjs — what the bounded debate does when one of its two
// dispatches comes back dead.
//
// The defect this closes (criterion SO-15, confirmed twice on 2026-09-07).
// Neither half of the debate checked its return.
//
//   · The defense dispatch carries no schema, so a dead call resolves to null
//     and was interpolated straight into the adjudicator's prompt. The judge
//     was shown the four characters `null` under "Defense:" and weighed an
//     argument nobody wrote.
//   · `ruling.decision` was read off a possibly-null return. That throws inside
//     the per-finding thunk; `parallel()` turns a thrown thunk into a null slot
//     and `verified.filter(Boolean)` drops it. The finding then vanished from
//     `results`, `surviving`, `refuted` and every metric on the row — raised by
//     a lens, killed by nothing, absent from the published review, with no
//     field anywhere recording that it had existed.
//
// The second one is why this file is not optional. A silent deletion is
// invisible in exactly the place you would look for it: a shorter findings list
// is what a clean PR looks like too.
//
// Like replay-gates.mjs, this executes the REAL code sliced out of
// workflows/tribunal.js by its markers, never a copy.
//
// Usage: node eval/replay-debate-degrade.mjs

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

let loaded;
try {
  loaded = loadRegion(WORKFLOW, "DEBATE-DEGRADE", [
    "debateDefenseBlock",
    "debateRuling",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { debateDefenseBlock, debateRuling } = api;
const src = readFileSync(WORKFLOW, "utf8");

console.log(`\nDEBATE-DEGRADE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const KEPT = { survived: true, valid: [], borderline: true };
const KILLED = { survived: false, valid: [], borderline: true };

console.log(`\n--- the defense half: a dead dispatch is never shown as an argument ---`);
{
  check(
    "a real defense passes through untouched",
    debateDefenseBlock("The guard is absent on the retry path; see line 40.") ===
      "The guard is absent on the retry path; see line 40.",
  );
  check("surrounding whitespace is trimmed", debateDefenseBlock("  hi  ") === "hi");

  for (const [label, value] of [
    ["null (a dead agent() call)", null],
    ["undefined", undefined],
    ["the empty string", ""],
    ["whitespace only", "   \n  "],
    ["a non-string the schema never constrained", { decision: "SURVIVES" }],
    ["a number", 0],
  ]) {
    const out = debateDefenseBlock(value);
    check(
      `${label} becomes an explicit statement of absence`,
      out.startsWith("NO DEFENSE WAS PRODUCED"),
      out.slice(0, 40),
    );
  }
  check(
    "the literal string `null` can never reach the judge as the defense",
    !/^null$/.test(debateDefenseBlock(null)),
  );
  check(
    "and the judge is told absence is not a concession",
    /not as a concession/i.test(debateDefenseBlock(null)) &&
      /do not read the absence of a defense as evidence against it/i.test(
        debateDefenseBlock(null),
      ),
  );
}

console.log(`\n--- the adjudicator half: a live ruling still overrides the panel ---`);
{
  const flip = debateRuling(
    { decision: "REFUTED", rationale: "The path is unreachable.", changedFromPanel: true },
    KEPT,
  );
  check("a live REFUTED overrides a surviving panel", flip.survived === false);
  check("changedFromPanel true records a flip", flip.outcome === "flipped");
  check("the adjudicator's own rationale is carried", flip.rationale === "The path is unreachable.");
  check("and it is marked adjudicated", flip.adjudicated === true);

  const hold = debateRuling(
    { decision: "SURVIVES", rationale: "Defense holds.", changedFromPanel: false },
    KEPT,
  );
  check("a live SURVIVES agreeing with the panel records a hold", hold.outcome === "held" && hold.survived === true);

  const flipUp = debateRuling(
    { decision: "SURVIVES", rationale: "The refuters misread the guard.", changedFromPanel: true },
    KILLED,
  );
  check(
    "a ruling can also revive a finding the panel killed",
    flipUp.survived === true && flipUp.outcome === "flipped",
  );
  check(
    "a missing rationale becomes null, not the string 'undefined'",
    debateRuling({ decision: "SURVIVES", changedFromPanel: false }, KEPT).rationale === null,
  );
}

console.log(`\n--- the adjudicator half: a dead ruling degrades, it does not delete ---`);
{
  for (const [label, value] of [
    ["null (the shape a dead schema'd call returns)", null],
    ["undefined", undefined],
    ["an object with no decision", { rationale: "x", changedFromPanel: false }],
    ["a decision outside the enum", { decision: "MAYBE", changedFromPanel: false }],
    ["a decision of the wrong type", { decision: 1, changedFromPanel: false }],
    ["the empty string as a decision", { decision: "", changedFromPanel: false }],
  ]) {
    const r = debateRuling(value, KEPT);
    check(`${label} does not throw`, r && typeof r === "object");
    check(`${label} keeps the panel's own outcome`, r.survived === KEPT.survived);
    check(`${label} is recorded as unadjudicated`, r.outcome === "unadjudicated");
    check(`${label} is not marked adjudicated`, r.adjudicated === false);
  }
  check(
    "a dead ruling over a KILLED panel keeps it killed, not revived",
    debateRuling(null, KILLED).survived === false,
  );
  check(
    "the degrade says what happened rather than leaving the reason blank",
    /never judged on argument/.test(debateRuling(null, KEPT).rationale),
  );
  check(
    "'unadjudicated' is distinguishable from 'held' — the whole point of the value",
    debateRuling(null, KEPT).outcome !==
      debateRuling({ decision: "SURVIVES", changedFromPanel: false }, KEPT).outcome,
  );
}

console.log(`\n--- the deletion this replaces is genuinely reachable ---`);
{
  // Not a claim about the fix — a demonstration that the pre-fix shape really
  // did lose the finding, so the value of the guard is not taken on trust.
  const preFix = (ruling) => ruling.decision === "SURVIVES";
  let threw = false;
  try {
    preFix(null);
  } catch {
    threw = true;
  }
  check("reading .decision off a dead ruling throws", threw);

  const thunk = (ruling) => {
    try {
      return { survived: preFix(ruling) };
    } catch {
      return null; // what parallel() does with a thrown thunk
    }
  };
  const verified = [thunk({ decision: "SURVIVES" }), thunk(null), thunk({ decision: "REFUTED" })];
  check(
    "…and the thrown slot is dropped by filter(Boolean), losing the finding entirely",
    verified.length === 3 && verified.filter(Boolean).length === 2,
  );
  check(
    "the fixed path keeps all three",
    [
      debateRuling({ decision: "SURVIVES", changedFromPanel: false }, KEPT),
      debateRuling(null, KEPT),
      debateRuling({ decision: "REFUTED", changedFromPanel: true }, KEPT),
    ].filter(Boolean).length === 3,
  );
}

console.log(`\n--- wiring: the region is actually used, and the degrade is loud ---`);
{
  check(
    "the adjudicator's prompt interpolates the guarded defense, not the raw return",
    /Defense:\n\$\{debateDefenseBlock\(defense\)\}/.test(src),
  );
  // Written first as `!/ruling\.decision/`, which was red on a correct tree:
  // the guarded read inside debateRuling is `ruling.decision` too, so the
  // assertion forbade its own fix. What matters is that the UNGUARDED
  // assignment is gone, and that every surviving read sits behind a type check.
  check(
    "the unguarded assignment that threw is gone",
    !/survived = ruling\.decision/.test(src),
  );
  check(
    "every surviving read of .decision is behind a guard",
    src
      .split("\n")
      .filter((l) => /ruling\.decision/.test(l) && !/^\s*\/\//.test(l))
      .every((l) => /typeof ruling\.decision === "string"/.test(l)),
  );
  check("the call site goes through debateRuling", /const ruled = debateRuling\(ruling, tally\)/.test(src));
  check(
    "an unadjudicated debate is logged, not just recorded",
    /the debate fired and produced no ruling/.test(src),
  );
  check(
    "the log names the outcome the finding fell back to (VR-4)",
    /keeps the panel's mechanical outcome \(\$\{tally\.survived \? "SURVIVES" : "REFUTED"\}\)/.test(src),
  );
  check(
    "the file's own vocabulary comment lists the new value",
    /null \| "held" \| "flipped" \| "unadjudicated"/.test(src),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall debate-degrade expectations hold");
