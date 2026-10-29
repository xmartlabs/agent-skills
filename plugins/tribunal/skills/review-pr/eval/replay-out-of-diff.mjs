#!/usr/bin/env node
// replay-out-of-diff.mjs — the pre-panel gate's re-anchoring, run as the
// shipping OUT-OF-DIFF region.
//
// A lens that traces a caller the PR broke tends to file the finding at that
// caller, outside the diff, where the gate used to drop it unseen. These checks
// pin that such a finding is moved onto the changed file its own evidence
// names, that one tied to nothing is still dropped, and that an in-diff
// candidate passes through untouched.
//
// Usage: node eval/replay-out-of-diff.mjs

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

const { api, violations } = loadRegion(WORKFLOW, "OUT-OF-DIFF", ["fileInDiff", "reanchorOutOfDiff"]);
const { reanchorOutOfDiff } = api;
console.log(`\nOUT-OF-DIFF region extracted from ${WORKFLOW}`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const files = ["src/helpers/client.ts", "src/helpers/client.test.ts"];
const base = { title: "caller now throws", file: "src/services/accounts.ts", line: 344, severity: "important" };

console.log("\n--- in the diff: untouched ---");
{
  const c = { ...base, file: "src/helpers/client.ts" };
  check("the same object comes back", reanchorOutOfDiff(c, files, null) === c);
}

console.log("\n--- out of the diff, tied by verifiedBy ---");
{
  const c = { ...base, verifiedBy: [{ file: "src/services/accounts.ts", line: 344, quote: "if (isNotFound(e))" }, { file: "src/helpers/client.ts", line: 80, quote: "return typeof body === 'object'" }] };
  const r = reanchorOutOfDiff(c, files, null);
  check("moved onto the in-diff anchor", r && r.file === "src/helpers/client.ts" && r.line === 80, JSON.stringify(r));
  check("the original location is kept", r && r.reanchored.from.file === "src/services/accounts.ts" && r.reanchored.from.line === 344 && r.reanchored.via === "verifiedBy");
  check("severity and title survive", r && r.severity === "important" && r.title === base.title);
}

console.log("\n--- out of the diff, tied by a <dependents> entry ---");
{
  const br = {
    changed: [{ file: "src/helpers/client.ts", symbols: ["isNotFound"] }],
    callers: [{ file: "src/services/accounts.ts", line: 344, via: "isNotFound", call: true, awaited: false, text: "if (isNotFound(e))" }],
  };
  const r = reanchorOutOfDiff(base, files, br);
  check("moved onto the changed declaration's file", r && r.file === "src/helpers/client.ts" && r.reanchored.via === "dependents", JSON.stringify(r));
  const unrelated = { ...base, file: "src/other.ts" };
  check("a caller the block does not list is dropped", reanchorOutOfDiff(unrelated, files, br) === null);
}

console.log("\n--- out of the diff, tied to nothing ---");
{
  check("dropped", reanchorOutOfDiff(base, files, null) === null);
  check("a verifiedBy outside the diff does not save it", reanchorOutOfDiff({ ...base, verifiedBy: [{ file: "src/x.ts", line: 1, quote: "abcdefghijk" }] }, files, null) === null);
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall out-of-diff expectations hold");
