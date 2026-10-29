#!/usr/bin/env node
// replay-external-head-pinned.mjs — the row field that says whether the external
// refuters were actually pointed at the pinned tree.
//
// WHY IT EXISTS. From v66 to v68 every run with a prepared checkout recorded
// `headPin: {pinned: true}` while the external CLIs ran in the ambient directory,
// because the path never reached `externalCwd`. Nothing in the row could show the
// disagreement: `headPin` says a tree was PREPARED, not that the CLI was pointed
// at it. It was found by reading a stderr file. `externalHeadPinned` closes that.
//
// This evaluates the REAL expression out of the shipping workflow, not a copy,
// the way replay-gates.mjs does for the panel rules.
//
// Usage: node eval/replay-external-head-pinned.mjs

import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");
const SKILL = resolve(HERE, "..", "SKILL.md");
const SCHEMA = resolve(HERE, "..", "references", "metrics-schema.md");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nexternalHeadPinned — the row expression in tribunal.js`);

const src = readFileSync(WORKFLOW, "utf8");
const m = src.match(/^\s*externalHeadPinned:\s*(.+?),\s*$/m);
check(
  "the workflow writes externalHeadPinned into the row",
  Boolean(m),
  "no `externalHeadPinned:` entry in the metrics object",
);

let evaluate = null;
if (m) {
  try {
    evaluate = new Function(
      "useCodexHead",
      "useGeminiHead",
      "externalCwd",
      `return (${m[1]});`,
    );
  } catch (err) {
    check("the row expression is evaluable", false, String(err));
  }
}

if (evaluate) {
  const PIN = "/tmp/peer-review-wt-2101-AbCdEf";
  check(
    "codex enabled and a tree resolved: true",
    evaluate(true, false, PIN) === true,
  );
  check(
    "gemini alone enabled and a tree resolved: true",
    evaluate(false, true, PIN) === true,
  );
  // THE REGRESSION. This is the v66 to v68 state: a head enabled, no path.
  check(
    "codex enabled and NO tree resolved: false, the ambient case",
    evaluate(true, false, null) === false,
  );
  check(
    "no external head enabled: null, there was nothing to pin",
    evaluate(false, false, null) === null &&
      evaluate(false, false, PIN) === null,
  );
  // Controls: an expression that ignores the heads, or ignores the path, must
  // not satisfy the group above.
  const alwaysFalse = () => false;
  const ignoresHeads = (c, g, cwd) => Boolean(cwd);
  const wrong = (fn) =>
    [
      [true, false, PIN, true],
      [false, true, PIN, true],
      [true, false, null, false],
      [false, false, null, null],
      [false, false, PIN, null],
    ].filter(([c, g, cwd, want]) => fn(c, g, cwd) !== want).length;
  check(
    "control: an expression that ignores the heads, or the path, fails this group",
    wrong(alwaysFalse) > 0 && wrong(ignoresHeads) > 0 && wrong(evaluate) === 0,
    `alwaysFalse=${wrong(alwaysFalse)} ignoresHeads=${wrong(ignoresHeads)} shipping=${wrong(evaluate)}`,
  );
}

console.log(`\n--- the places that have to agree ---`);
check(
  "metrics-schema.md documents the field and how it differs from headPin",
  /\| `externalHeadPinned` \|/.test(readFileSync(SCHEMA, "utf8")) &&
    /different fact from `headPin`/.test(readFileSync(SCHEMA, "utf8")),
  "the field exists in the row but not in the spec",
);

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall external-head-pinned expectations hold");
