#!/usr/bin/env node
// replay-strengthen-merge.mjs — applying a refine pass to the finding it was
// asked to improve.
//
// The defect (criterion TD-25, confirmed twice on 2026-09-07): a schema with no
// representation for the empty answer. STRENGTHEN_SCHEMA required title, file,
// severity and description, so a pass that could NOT strengthen the case still
// had to write all four — and `{ ...workingFinding, ...strengthened }`
// installed whatever it wrote over the original.
//
// That matters more here than it looks. The refine pass is where findings get
// their severity rewritten, and the file already records that 47 of 53 panels
// on `nit` findings were findings that entered as `important` and were degraded
// right here. With no way to say "I could not do this", a failed strengthen and
// a genuine demotion produced identical rows, so the one measurement the pass
// exists to support could not be taken.
//
// The second failure was quieter. A dead dispatch resolves to null, and
// spreading null in JavaScript is a silent no-op: the merge "worked", `refined`
// was set true, and the row recorded a refine that never happened.
//
// Usage: node eval/replay-strengthen-merge.mjs

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
  loaded = loadRegion(WORKFLOW, "STRENGTHEN-MERGE", ["applyStrengthen"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { applyStrengthen } = api;
const src = readFileSync(WORKFLOW, "utf8");

const ORIGINAL = {
  slotId: "s1",
  title: "negative amount creates a negative invoice",
  file: "src/controllers/v2/invoice.controller.ts",
  line: 88,
  severity: "important",
  description: "No positive check before create.",
  isBehaviorChanging: true,
  touchesHighRiskSurface: true,
};

console.log(`\nSTRENGTHEN-MERGE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- a successful strengthen still merges, exactly as before ---`);
{
  const r = applyStrengthen(ORIGINAL, {
    strengthened: true,
    unstrengthenableReason: null,
    title: "negative amount reaches prisma.invoice.create unchecked",
    file: ORIGINAL.file,
    line: 91,
    severity: "critical",
    description: "POST /api/v2/invoices with amount:-1 persists a negative row.",
    repro: "curl -X POST … -d '{\"amount\":-1}'",
  });
  check("the outcome is strengthened", r.outcome === "strengthened");
  check("the improved title wins", /reaches prisma/.test(r.finding.title));
  check("the raised severity wins", r.finding.severity === "critical");
  check("a new field is carried over", /amount/.test(r.finding.repro || ""));
  check(
    "fields the pass did not mention survive from the original",
    r.finding.slotId === "s1" && r.finding.touchesHighRiskSurface === true,
  );
  check(
    "the bookkeeping flags are not smuggled onto the finding",
    !("strengthened" in r.finding) && !("unstrengthenableReason" in r.finding),
    JSON.stringify(Object.keys(r.finding)),
  );
  check("no reason is attached to a success", r.reason === null);
}

console.log(`\n--- a strengthen never lowers the severity ---`);
{
  const r = applyStrengthen(ORIGINAL, {
    strengthened: true,
    unstrengthenableReason: null,
    title: "negative amount reaches prisma.invoice.create unchecked",
    file: ORIGINAL.file,
    severity: "nit",
    description: "Only an admin form sends this field.",
  });
  check("the original severity stays", r.finding.severity === "important", r.finding.severity);
  check("the lower proposal is recorded", r.finding.refineProposedSeverity === "nit", String(r.finding.refineProposedSeverity));
  check("the rest of the rewrite still applies", /reaches prisma/.test(r.finding.title));
  const same = applyStrengthen(ORIGINAL, { strengthened: true, unstrengthenableReason: null, title: "t", file: ORIGINAL.file, severity: "important", description: "d" });
  check("an equal severity records no proposal", !("refineProposedSeverity" in same.finding));
}

console.log(`\n--- an honest failure leaves the finding completely alone ---`);
{
  const r = applyStrengthen(ORIGINAL, {
    strengthened: false,
    unstrengthenableReason:
      "the guard is in a middleware I was not given; I cannot show the path is reachable",
    // The shape the schema still requires — an echo, per the prompt.
    title: ORIGINAL.title,
    file: ORIGINAL.file,
    severity: ORIGINAL.severity,
    description: ORIGINAL.description,
  });
  check("the outcome is unstrengthenable", r.outcome === "unstrengthenable");
  check("the finding object is the original, untouched", r.finding === ORIGINAL);
  check("the reason is carried for the log", /middleware/.test(r.reason));

  // The case the whole criterion is about: a pass that failed AND softened.
  const softened = applyStrengthen(ORIGINAL, {
    strengthened: false,
    unstrengthenableReason: "could not reach the evidence",
    title: "possible issue with amount handling",
    file: ORIGINAL.file,
    severity: "nit",
    description: "Might be worth a look.",
  });
  check(
    "a failed pass CANNOT demote the severity any more",
    softened.finding.severity === "important",
    softened.finding.severity,
  );
  check(
    "…nor water down the title",
    softened.finding.title === ORIGINAL.title,
  );
  check(
    "a failure with no reason still reports one rather than an empty string",
    /gave no reason/.test(
      applyStrengthen(ORIGINAL, { strengthened: false }).reason,
    ),
  );
}

console.log(`\n--- a dead dispatch is a third outcome, not a silent no-op ---`);
{
  for (const [label, value] of [
    ["null", null],
    ["undefined", undefined],
    ["the empty string", ""],
    ["a string instead of an object", "I could not do it"],
    ["a number", 0],
  ]) {
    const r = applyStrengthen(ORIGINAL, value);
    check(`${label} is reported as dispatch-failed`, r.outcome === "dispatch-failed", r.outcome);
    check(`${label} keeps the original finding`, r.finding === ORIGINAL);
  }
  check(
    "the three outcomes are three distinct values",
    new Set([
      applyStrengthen(ORIGINAL, null).outcome,
      applyStrengthen(ORIGINAL, { strengthened: false }).outcome,
      applyStrengthen(ORIGINAL, { strengthened: true, severity: "critical" }).outcome,
    ]).size === 3,
  );
  // Spreading null is a no-op, which is exactly why the old code looked fine.
  check(
    "the pre-fix shape really was silent: spreading null changes nothing",
    JSON.stringify({ ...ORIGINAL, ...null }) === JSON.stringify(ORIGINAL),
  );
  check(
    "…and the new shape distinguishes that from a real merge",
    applyStrengthen(ORIGINAL, null).outcome !== "strengthened",
  );
  check(
    "a missing `strengthened` flag is treated as failure, never as success",
    applyStrengthen(ORIGINAL, { title: "x", severity: "nit" }).outcome ===
      "unstrengthenable",
  );
  check(
    "a truthy-but-not-true flag does not count as success",
    applyStrengthen(ORIGINAL, { strengthened: "yes", severity: "nit" }).outcome ===
      "unstrengthenable",
  );
}

console.log(`\n--- wiring: schema, both call sites, and the row ---`);
{
  check(
    "the schema requires the flag, so the empty answer is expressible",
    /required: \["strengthened", "title", "file", "severity", "description"\]/.test(src),
  );
  check(
    "the prompt tells the model failing is allowed and how to say so",
    /You are allowed to fail at this/.test(src) &&
      /repeat the finding's original title, file, severity and description UNCHANGED/.test(src),
  );
  check(
    "…and forbids the specific dodge the old schema forced",
    /Do not paper over a failed strengthen by softening the claim or lowering the severity/.test(src),
  );
  // Comment lines are excluded deliberately. The region's own header quotes the
  // shape it replaced — `{ ...workingFinding, ...strengthened }` — and a naive
  // whole-file regex reads that prose as the defect still being present. Three
  // assertions in this branch of work made that mistake before this one.
  const codeLines = src
    .split("\n")
    .filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l))
    .join("\n");
  check(
    "neither call site spreads the raw return any more",
    !/\.\.\.r\.finding, \.\.\.strengthened/.test(codeLines) &&
      !/\.\.\.workingFinding, \.\.\.strengthened/.test(codeLines),
  );
  check(
    "both call sites go through applyStrengthen",
    (src.match(/applyStrengthen\(/g) || []).length >= 3,
    `${(src.match(/applyStrengthen\(/g) || []).length} occurrence(s) incl. the definition`,
  );
  check(
    "a non-strengthened outcome is logged at both call sites",
    (src.match(/Refine produced nothing for/g) || []).length === 2,
  );
  check(
    "refineOutcome reaches the per-finding result",
    /refined,\n\s*\/\/ `refined` says a refine pass was dispatched/.test(src),
  );
  check(
    "…and is persisted on the metrics row, defaulting for older shapes",
    (src.match(/refineOutcome: v\.refineOutcome \?\? null/g) || []).length === 2,
  );
  check(
    "every default result object carries the field, so absent never means 'strengthened'",
    (src.match(/refined: false,\n\s*refineOutcome: null,/g) || []).length === 3,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall strengthen-merge expectations hold");
