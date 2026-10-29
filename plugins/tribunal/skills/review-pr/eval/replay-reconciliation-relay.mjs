#!/usr/bin/env node
// replay-reconciliation-relay.mjs — a failed persist relay must not end the run.
//
// The defect (2026-10-17, BE #2094, wf_8cc5e7ce-846, installed v73). The
// reconcile-audit-persist mechanic copies metrics.json into a heredoc by hand.
// It dropped 503 characters from the middle of a 26.8 KB copy, the JSON did not parse,
// persist-run.sh never ran, and the relay returned `reconciliation: {error}`
// with no `checked`. The workflow only treated `checked === null` as a failure,
// went on to read `.counts["repeat-blocked"]` off undefined, and threw. The
// whole review was lost, and the worktree teardown after it never ran.
//
// This runs the shipping RECONCILIATION-RELAY region against that exact relay
// shape and the neighbours of it, then checks that the code downstream of the
// region reads nothing the region does not guarantee.
//
// Usage: node eval/replay-reconciliation-relay.mjs

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
  loaded = loadRegion(WORKFLOW, "RECONCILIATION-RELAY", [
    "resolveVoteReconciliation",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { resolveVoteReconciliation } = api;

console.log(`\nRECONCILIATION-RELAY region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

// What the code after the region reads, for every non-null result: `checked`
// compared to null, `counts[...]`, and `mismatches.filter`.
const usable = (r) =>
  r !== null &&
  (typeof r.checked === "number" || r.checked === null) &&
  r.counts !== null &&
  typeof r.counts === "object" &&
  Array.isArray(r.mismatches);

console.log(`\n--- the relay shape that crashed v73 ---`);
{
  const relayed = {
    error:
      "persist-run.sh input parse failed: JSONDecodeError Expecting ',' delimiter at line 1 column 24830 (char 24829), reported three times. No reconciliation output was printed.",
  };
  const r = resolveVoteReconciliation(relayed, true);
  check("an {error}-only relay becomes a usable result", usable(r), JSON.stringify(r));
  check("...that reads as a failed reconciliation", r && r.checked === null, JSON.stringify(r));
  check("...and keeps the relay's own error text", r && r.error === relayed.error, JSON.stringify(r));
}

console.log(`\n--- the other ways a relay can be empty ---`);
for (const [name, relayed] of [
  ["undefined (no bookkeeping)", undefined],
  ["null", null],
  ["an empty object", {}],
  ["a string", "FAILED"],
]) {
  const r = resolveVoteReconciliation(relayed, true);
  check(`${name}, with external votes, is a failed reconciliation`, usable(r) && r.checked === null, JSON.stringify(r));
  check(`${name}, without external votes, is null`, resolveVoteReconciliation(relayed, false) === null);
}

console.log(`\n--- a real reconciliation passes through ---`);
{
  const relayed = {
    checked: 4,
    ok: 4,
    counts: { match: 4 },
    mismatches: [],
    classifierFailed: 0,
  };
  const r = resolveVoteReconciliation(relayed, true);
  check("its fields are unchanged", JSON.stringify(r) === JSON.stringify(relayed), JSON.stringify(r));

  const partial = resolveVoteReconciliation({ checked: 2, ok: 2 }, true);
  check("a result missing counts and mismatches gets empty ones", usable(partial) && partial.checked === 2, JSON.stringify(partial));

  const scriptFailure = resolveVoteReconciliation(
    { checked: null, ok: null, counts: {}, mismatches: [], error: "no artifacts" },
    true,
  );
  check("the script's own failure shape stays a failure", usable(scriptFailure) && scriptFailure.error === "no artifacts", JSON.stringify(scriptFailure));
}

console.log(`\n--- the workflow uses the region, and nothing else builds the value ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "voteReconciliation is assigned from resolveVoteReconciliation",
    /const voteReconciliation = resolveVoteReconciliation\(/.test(src),
  );
  check(
    "no other assignment reads bookkeeping.reconciliation directly",
    (src.match(/bookkeeping\.reconciliation/g) || []).length === 1,
    `${(src.match(/bookkeeping\.reconciliation/g) || []).length} occurrence(s)`,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) failed`);
  process.exit(1);
}
console.log(`\nall checks passed`);
