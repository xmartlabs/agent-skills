#!/usr/bin/env node
// replay-tsc-oracle.mjs: the grounded typecheck's decision rules, run on the
// shipping code (the TSC-ORACLE regions of workflows/tribunal.js).
//
// The oracle can CONFIRM a finding with no panel and REFUTE a compile-level
// claim with no panel, so a wrong rule here moves a verdict with nothing
// downstream to catch it. Until 2026-10-19 it had no replay.
//
// Usage: node eval/replay-tsc-oracle.mjs

import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = join(HERE, "..", "..", "..", "workflow-src", "tribunal.js");
const { api: O } = loadRegion(WORKFLOW, "TSC-ORACLE", [
  "parseTscDiagnostics",
  "classifyTypecheckText",
  "filterDiagnosticsToPR",
  "groupDiagnosticsByFile",
  "GROUNDED_LINE_WINDOW",
  "adjudicateFinding",
  "resolveDiagFile",
]);

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log("\n--- tsc output, parsed and classified ---");
const TSC = "src/a.ts(42,10): error TS2345: Argument of type 'string' is not assignable.\n    continuation that tsc indents\nsrc/b.ts(7,1): error TS2304: Cannot find name 'x'.";
const diags = O.parseTscDiagnostics(TSC);
check("one diagnostic per head line, continuation lines ignored", diags.length === 2 && diags[0].file === "src/a.ts" && diags[0].line === 42 && diags[0].code === "TS2345", JSON.stringify(diags));
check("no output parses to nothing", O.parseTscDiagnostics("").length === 0 && O.parseTscDiagnostics(null).length === 0);
for (const [text, want] of [["NO_TSCONFIG", "no-tsconfig"], ["DEGRADED_TIMEOUT", "degraded"], [TSC, "diagnostics"], ["NO_DIAGNOSTICS", "clean"], ["", "degraded"], ["something else", "degraded"]])
  check(`${JSON.stringify(text.slice(0, 20))} classifies as ${want}`, O.classifyTypecheckText(text) === want, O.classifyTypecheckText(text));

console.log("\n--- a finding against the diagnostics ---");
const by = O.groupDiagnosticsByFile(O.filterDiagnosticsToPR(diags, ["src/a.ts", "src/c.ts"]));
check("diagnostics outside the PR's files are dropped", !by["src/b.ts"] && by["src/a.ts"].length === 1);
const covered = new Set(["src/a.ts", "src/c.ts"]);
const W = O.GROUNDED_LINE_WINDOW;
check(`a diagnostic within ${W} lines confirms`, O.adjudicateFinding({ file: "src/a.ts", line: 42 + W }, by, covered).verdict === "confirmed");
check(`one ${W + 1} lines away does not`, O.adjudicateFinding({ file: "src/a.ts", line: 42 + W + 1 }, by, covered).verdict === null);
check("line 0 means no line, so nothing confirms on position", O.adjudicateFinding({ file: "src/a.ts", line: 0 }, by, covered).verdict === null);
check("a compile-level claim in a compiled, clean file is refuted", O.adjudicateFinding({ file: "src/c.ts", line: 3, mechanicalClaim: true }, by, covered).verdict === "refuted");
check("…not when the compiled set is unknown", O.adjudicateFinding({ file: "src/c.ts", line: 3, mechanicalClaim: true }, by, null).verdict === null);
check("…not when the file was never compiled", O.adjudicateFinding({ file: "src/d.ts", line: 3, mechanicalClaim: true }, by, covered).verdict === null);
check("a logic finding in a clean file falls through to the panel", O.adjudicateFinding({ file: "src/c.ts", line: 3 }, by, covered).verdict === null);

console.log("\n--- a finding's file, resolved to the PR's path ---");
check("an exact path passes through", O.resolveDiagFile("src/a.ts", ["src/a.ts"]) === "src/a.ts");
check("a basename one PR file ends with resolves to it", O.resolveDiagFile("accounts.ts", ["src/services/v2/accounts.ts", "src/b.ts"]) === "src/services/v2/accounts.ts");
check("a basename two PR files share stays as it was", O.resolveDiagFile("index.ts", ["src/a/index.ts", "src/b/index.ts"]) === "index.ts", O.resolveDiagFile("index.ts", ["src/a/index.ts", "src/b/index.ts"]));
check("no match stays as it was", O.resolveDiagFile("zzz.ts", ["src/a.ts"]) === "zzz.ts");

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall tsc-oracle expectations hold");
