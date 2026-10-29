// groundtruth-compare.mjs — score packaged vs unpackaged refuters against the
// corpus, not against each other.
//
// eval/tier1-compare.mjs measures a packaged refuter against the RECORDED
// baseline for the same finding, which answers the cost question and cannot
// answer the quality one: the baseline is another model, so agreement means the
// two models agree, not that either is right.
//
// This scores both arms against a finding whose truth is known from OUTSIDE the
// harness. For a `knownReal` finding the correct verdict is NOT_REFUTED; a
// REFUTED verdict is a false negative, which on a review harness is the
// expensive direction — the defect ships.
//
// The comparison is only meaningful because both arms received an IDENTICAL
// claim (see eval/build-groundtruth-arms.mjs) with no reference to the later
// fix. The package is the single variable.
//
// Read the disagreement rate against the harness's own noise floor before
// concluding anything from it: on PR #1935's getStateCode claim, eight recorded
// refuters split 4 NOT_REFUTED / 4 REFUTED with no package involved at all. Two
// arms differing once is inside that.
//
// Usage: node eval/groundtruth-compare.mjs <case-id> <truth> <control.jsonl> <package.jsonl> [...]

import { readFileSync } from "node:fs";
import { parseTranscript } from "./agent-cost.mjs";
import { finalVerdict } from "./tier1-compare.mjs";

const args = process.argv.slice(2);
if (args.length < 4 || args.length % 4 !== 0) {
  console.error(
    "usage: node eval/groundtruth-compare.mjs <case-id> <truth> <control.jsonl> <package.jsonl> [...]"
  );
  process.exit(2);
}

const rows = [];
for (let i = 0; i < args.length; i += 4) {
  const [id, truth, controlPath, packagePath] = args.slice(i, i + 4);
  const read = (p) => {
    const text = readFileSync(p, "utf8");
    return { v: finalVerdict(text), cost: parseTranscript(text) };
  };
  rows.push({ id, truth, control: read(controlPath), pkg: read(packagePath) });
}

const mark = (v, truth) => (!v ? "UNPARSEABLE" : v.verdict === truth ? "correct" : "WRONG");

console.log(`\nPACKAGED vs UNPACKAGED, scored against the corpus\n`);
for (const r of rows) {
  console.log(`${r.id}   truth = ${r.truth}`);
  for (const [name, arm] of [
    ["control", r.control],
    ["package", r.pkg]
  ]) {
    const v = arm.v;
    console.log(
      `  ${name.padEnd(8)} ${String(v ? v.verdict : "—").padEnd(13)} ${String(mark(v, r.truth)).padEnd(12)}` +
        ` conf ${v ? v.confidence : "—"}   ${arm.cost.turns} turns, ${(arm.cost.inputTokens / 1e6).toFixed(2)}M input`
    );
    if (name === "package" && v && Array.isArray(v.filesOpenedOutsidePackage))
      console.log(
        `           opened outside the package: ${v.filesOpenedOutsidePackage.length ? v.filesOpenedOutsidePackage.join(", ") : "none"}`
      );
  }
  console.log();
}

const scored = rows.filter((r) => r.control.v && r.pkg.v);
const cOk = scored.filter((r) => r.control.v.verdict === r.truth).length;
const pOk = scored.filter((r) => r.pkg.v.verdict === r.truth).length;
const regressions = scored.filter(
  (r) => r.control.v.verdict === r.truth && r.pkg.v.verdict !== r.truth
);
const rescues = scored.filter((r) => r.control.v.verdict !== r.truth && r.pkg.v.verdict === r.truth);

console.log(`TOTALS over ${scored.length} case(s) with ground truth`);
console.log(`  control correct : ${cOk}/${scored.length}`);
console.log(`  package correct : ${pOk}/${scored.length}`);
console.log(`  package WRONG where control was right : ${regressions.length}${regressions.length ? ` (${regressions.map((r) => r.id).join(", ")})` : ""}`);
console.log(`  package right where control was wrong : ${rescues.length}${rescues.length ? ` (${rescues.map((r) => r.id).join(", ")})` : ""}`);

const cT = scored.reduce((s, r) => s + r.control.cost.turns, 0);
const pT = scored.reduce((s, r) => s + r.pkg.cost.turns, 0);
const cI = scored.reduce((s, r) => s + r.control.cost.inputTokens, 0);
const pI = scored.reduce((s, r) => s + r.pkg.cost.inputTokens, 0);
console.log(`  turns  ${cT} -> ${pT}`);
console.log(`  input  ${(cI / 1e6).toFixed(2)}M -> ${(pI / 1e6).toFixed(2)}M`);

console.log(`\nBefore reading a difference as an effect: the measured noise floor on this`);
console.log(`harness is 4/4 — eight recorded refuters split evenly on one #1935 finding with`);
console.log(`no package involved. A single disagreement does not clear it.\n`);
