#!/usr/bin/env node
// replay-operator-override.mjs — reading a hand-recorded panel error.
//
// Why this needs its own replay. `operatorOverride` is the only ground truth in
// this harness that is not another model's opinion, so it is also the only one
// with no second source to catch a mistake in it. Everything downstream — a
// T_KILL move, a DELTA move, an effort-tier decision on refuters — would rest
// on a handful of these annotations. A single one that contradicts its own row
// and gets counted anyway is fabricated evidence for a threshold change, and it
// would look exactly like real evidence.
//
// So the rules that matter are the refusals, and they are what this tests:
// an override only means something relative to what the panel decided, and one
// with no stated verification is just another unrationalized verdict.
//
// Usage: node eval/replay-operator-override.mjs

import { readOverride, collectOverrides, OVERRIDE_BUCKET } from "./operator-override.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const OK = {
  direction: "reinstated",
  by: "alice",
  at: "2026-07-22T14:00:00Z",
  verifiedAt: "272732ddcf691f3ecf7dffbe9e2509ae20b918cf",
  reason: "read the worker's failed listener at that sha: isFinalAttempt is never set"
};
const killed = (over) => ({ title: "t", file: "a.ts", survived: false, refuteFraction: 0.5, threshold: 0.45, operatorOverride: over });
const passed = (over) => ({ title: "t", file: "a.ts", survived: true, refuteFraction: 0.2, threshold: 0.45, operatorOverride: over });

console.log(`\n--- absence is not an error ---`);
{
  const v = readOverride(killed(null));
  check("a null override is absent, not invalid", v.present === false && v.errors.length === 0);
  check("a missing key is absent too", readOverride({ survived: false }).present === false);
  check("a null finding does not throw", readOverride(null).present === false);
  check("undefined does not throw", readOverride(undefined).present === false);
}

console.log(`\n--- the three valid directions ---`);
{
  const v = readOverride(killed(OK));
  check("reinstated on a killed finding is valid", v.ok === true, JSON.stringify(v.errors));
  check("...and buckets as a panel false negative", v.bucket === "panel-false-negative");
  check("no warnings on a fully recorded override", v.warnings.length === 0, JSON.stringify(v.warnings));
}
{
  const v = readOverride(passed({ ...OK, direction: "rejected" }));
  check("rejected on a surviving finding is valid", v.ok === true, JSON.stringify(v.errors));
  check("...and buckets as a panel false positive", v.bucket === "panel-false-positive");
}
{
  // The case the vocabulary was missing until 2026-07-29. PR #1968: the panel
  // killed it, the operator reinstated it, then re-verified and took that back.
  // The panel was right, so this is neither a false negative nor a false
  // positive — and it was recorded as `dropped`, which no list ever accepted, so
  // score.mjs reported it INVALID and the corpus silently ran on two labels.
  const v = readOverride(killed({ ...OK, direction: "withdrawn" }));
  check("withdrawn on a killed finding is valid", v.ok === true, JSON.stringify(v.errors));
  check("...and buckets as panel-confirmed-correct", v.bucket === "panel-confirmed-correct");
  check(
    "...and is NOT counted as either kind of panel error",
    v.bucket !== "panel-false-negative" && v.bucket !== "panel-false-positive",
  );
}
{
  // A withdrawal only exists where a reinstatement did. On a finding the panel
  // PASSED there was nothing to retract.
  const v = readOverride(passed({ ...OK, direction: "withdrawn" }));
  check("withdrawn on a SURVIVING finding is refused", v.ok === false);
  check("...and says why", /requires survived === false/.test(v.errors.join(" ")), JSON.stringify(v.errors));
}
{
  check(
    "the bucket table covers exactly the three directions",
    Object.keys(OVERRIDE_BUCKET).sort().join(",") === "reinstated,rejected,withdrawn",
    Object.keys(OVERRIDE_BUCKET).sort().join(","),
  );
}
{
  // The value the corpus actually carried. It must stay invalid — accepting it
  // as an alias would leave two spellings for one event, which is the fork this
  // change exists to close.
  check(
    '"dropped" is still refused — the row gets relabelled, not aliased',
    readOverride(killed({ ...OK, direction: "dropped" })).ok === false,
  );
}

console.log(`\n--- the consistency rule: an override is relative to the panel ---`);
{
  // Reinstating something that already survived reinstates nothing. Counting it
  // would invent a panel error that never happened — and inflate exactly the
  // rate a T_KILL change would be justified by.
  const v = readOverride(passed(OK));
  check("reinstated on a SURVIVING finding is refused", v.ok === false);
  check("...and says why", /requires survived === false/.test(v.errors.join(" ")), JSON.stringify(v.errors));
  check("...and yields no bucket", v.bucket === null);
}
{
  const v = readOverride(killed({ ...OK, direction: "rejected" }));
  check("rejected on a KILLED finding is refused", v.ok === false);
  check("...and says why", /requires survived === true/.test(v.errors.join(" ")));
}
{
  // `survived` absent or non-boolean is not "false". A row that never recorded
  // the outcome cannot support a claim about the outcome.
  const v = readOverride({ title: "t", operatorOverride: OK });
  check("an override on a row with no `survived` is refused", v.ok === false);
  const v2 = readOverride({ title: "t", survived: null, operatorOverride: OK });
  check("survived: null is not treated as false", v2.ok === false);
}

console.log(`\n--- malformed annotations ---`);
{
  check("an unknown direction is refused", readOverride(killed({ ...OK, direction: "flipped" })).ok === false);
  check("a missing direction is refused", readOverride(killed({ ...OK, direction: undefined })).ok === false);
  check("direction is case-sensitive", readOverride(killed({ ...OK, direction: "Reinstated" })).ok === false);
  check("a non-object override is refused", readOverride(killed("reinstated")).ok === false);
  check("an array override is refused", readOverride(killed(["reinstated"])).ok === false);
  check(
    "a non-object override still reports an error rather than passing silently",
    readOverride(killed("reinstated")).errors.length > 0,
  );
}
{
  // The point of the whole field: replacing an unrationalized kill with an
  // unrationalized reinstatement is not an improvement.
  check("a missing reason is refused", readOverride(killed({ ...OK, reason: undefined })).ok === false);
  check("a blank reason is refused", readOverride(killed({ ...OK, reason: "   " })).ok === false);
  check(
    "...and the message says what is wanted",
    /how the finding was verified/.test(readOverride(killed({ ...OK, reason: "" })).errors.join(" ")),
  );
}

console.log(`\n--- warnings: usable but weaker ---`);
{
  const v = readOverride(killed({ ...OK, verifiedAt: undefined }));
  check("a missing sha warns but still scores", v.ok === true && v.warnings.length === 1);
  check("...and names the consequence", /re-checked against a tree/.test(v.warnings.join(" ")));
  const v2 = readOverride(killed({ ...OK, by: "", at: "" }));
  check("missing by/at warn, do not refuse", v2.ok === true && v2.warnings.length === 2);
}

console.log(`\n--- collectOverrides across rows ---`);
{
  const rows = [
    { pr: 1968, headSha: "abc", ts: "2026-07-22T00:00:00Z", findings: [killed(OK), killed(null), passed(null)] },
    { pr: 1935, headSha: "def", ts: "2026-07-24T00:00:00Z", findings: [passed({ ...OK, direction: "rejected" })] },
    { pr: 1911, headSha: "ghi", findings: [passed(OK)] },
  ];
  const all = collectOverrides(rows);
  check("only annotated findings are collected", all.length === 3, `${all.length}`);
  check("valid ones carry their bucket", all.filter((o) => o.ok).length === 2);
  check("the inconsistent one is collected as INVALID, not dropped", all.some((o) => !o.ok && o.pr === 1911));
  check("each entry keeps its PR", all.map((o) => o.pr).join(",") === "1968,1935,1911");
  check("panel stats travel with the override", all[0].refuteFraction === 0.5 && all[0].threshold === 0.45);
  check("a row with no findings does not throw", collectOverrides([{ pr: 1 }]).length === 0);
  check("null rows do not throw", collectOverrides(null).length === 0);
}

console.log(`
What this does NOT establish:
  · That any override is TRUE. It checks that an annotation is internally
    consistent with the row it annotates — not that the operator read the code
    correctly. Nothing offline can check that; the \`verifiedAt\` sha exists so a
    second person can.
  · That the overrides are representative. These are hand-recorded on the runs
    somebody happened to check by hand, so the sample is biased toward findings
    that looked wrong. A rate computed over them is not the panel's error rate.
  · That anyone records them. The field defaults to null and nothing forces it;
    score.mjs printing "none recorded" is the only pressure that exists.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
