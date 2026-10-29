#!/usr/bin/env node
// replay-report-fidelity-check.mjs — lib/check-report-fidelity.mjs, the
// in-context path's counterpart to the Workflow's inline
// mergedLocationsMissingFromReportText / killReasonMissingFromReportText /
// mergedFromAnnotationMissingFromReportText /
// groundedDiagnosticMissingFromReportText / unexaminedWordingInReportText
// calls, run against real inputs.
//
// Two separate things this file has to establish, because a script that
// slices shared regions out of another file can fail in two independent
// ways: the SLICE could be wrong (grabbed the wrong text, or none), or the
// LOGIC inside the slice could be wrong (computed the wrong answer once
// grabbed). Getting the second right while silently mis-extracting is exactly
// how a "single source of truth" claim rots into two sources that happen to
// agree today.
//
//   1. Fault injection: does the script actually flag a fact the report
//      dropped, and stay silent when nothing was dropped? (via the real CLI,
//      spawned as a subprocess — not by calling the exported functions
//      in-process, which would not exercise argv/stdin handling at all.)
//   2. Anti-drift: does this script's own private region-extractor
//      (duplicated rather than imported from eval/extract-pure-region.mjs —
//      see this file's header for why) still agree with the canonical one on
//      BOTH regions (MERGED-LOCATIONS-CHECK, REPORT-FIDELITY-CHECKS)? If
//      someone reshapes the `===== BEGIN/END =====` marker syntax and updates
//      one copy, this is what notices the other didn't move. Same
//      "two splitters must agree" shape replay-incremental-scope.mjs uses for
//      its own duplicated file-splitter.
//
// Usage: node eval/replay-report-fidelity-check.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { lib, WORKFLOW } from "./paths.mjs";
import { loadRegion } from "./extract-pure-region.mjs";
import { extractRegion as extractRegionDup, loadChecks } from "../../../lib/check-report-fidelity.mjs";

const SCRIPT = lib("check-report-fidelity.mjs");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const DIR = mkdtempSync(join(tmpdir(), "report-fidelity-check-"));

function run(reportText, metrics) {
  const reportPath = join(DIR, `report-${Math.floor(Math.random() * 1e9)}.md`);
  writeFileSync(reportPath, reportText);
  const out = execFileSync("node", [SCRIPT, reportPath], {
    input: JSON.stringify(metrics),
    encoding: "utf8",
  });
  return JSON.parse(out);
}

console.log(`  using: ${SCRIPT}`);

// --- fault injection: one finding, every rule broken at once ---------------
{
  const report = "### Blocking\n1. dup exit-0 bug — a.sh:5 — also at: b.sh:12";
  const metrics = {
    findings: [
      {
        title: "dup exit-0 bug",
        mergedLocations: [
          { file: "a.sh", line: 5 },
          { file: "b.sh", line: 12 },
          { file: "c.sh", line: 9 },
        ],
        mergedFrom: ["lensA", "lensB"],
        votes: [{ verdict: "REFUTED", killReason: "out-of-scope" }],
        grounded: { verdict: "confirmed", diagnostic: "TS2345: never printed" },
      },
    ],
  };
  const patched = run(report, metrics).findings[0];
  check(
    "flags the merged location the report actually dropped",
    JSON.stringify(patched.mergedLocationsMissingFromReport) === JSON.stringify(["c.sh:9"]),
    JSON.stringify(patched.mergedLocationsMissingFromReport),
  );
  check(
    "flags the killReason the report never cited",
    JSON.stringify(patched.killReasonMissingFromReport) === JSON.stringify(["out-of-scope"]),
    JSON.stringify(patched.killReasonMissingFromReport),
  );
  check(
    "flags the missing mergedFrom annotation and every lens name",
    JSON.stringify(patched.mergedFromAnnotationMissingFromReport.sort()) ===
      JSON.stringify(["(found by:", "lensA", "lensB"].sort()),
    JSON.stringify(patched.mergedFromAnnotationMissingFromReport),
  );
  check(
    "flags the typecheck diagnostic that never appeared verbatim",
    JSON.stringify(patched.groundedDiagnosticMissingFromReport) ===
      JSON.stringify(["TS2345: never printed"]),
    JSON.stringify(patched.groundedDiagnosticMissingFromReport),
  );
}

// --- the unexamined marker on a gate-demoted finding (issue #46) ------------
// The report shape of the run in issue #46: a bold title, the
// location on the next line, and the marker the prompt reserves for findings no
// panel ran on. The other finding carries the same marker correctly, so a check
// that searched the whole report would flag both.
{
  const report =
    "### 🔵 Nits\n\n**Unexamined one**\n`src/a.ts:10`\n\nBody. — NOT ADJUDICATED: no refuter examined this claim\n\n" +
    "**Demoted one**\n`src/services/v2/logs.ts:62`\n\nNOT ADJUDICATED: no refuter examined this claim. Gate note: \"Downgraded from Blocking\".\n";
  const unexamined = { title: "Unexamined one", file: "src/a.ts", line: 10, votes: [], verifiedByGateNote: null };
  const demoted = {
    title: "Demoted one",
    file: "src/services/v2/logs.ts",
    line: 62,
    votes: [{ verdict: "NOT_REFUTED" }, { verdict: "NOT_REFUTED" }],
    verifiedByGateNote: "Downgraded from Blocking: verifiedBy is missing or empty",
  };
  const [u, d] = run(report, { findings: [unexamined, demoted] }).findings;
  check(
    "flags the unexamined marker on the gate-demoted finding whose panel voted",
    JSON.stringify(d.unexaminedWordingInReport) ===
      JSON.stringify(["NOT ADJUDICATED", "no refuter examined"]),
    JSON.stringify(d.unexaminedWordingInReport),
  );
  check(
    "leaves the same marker alone on a finding no panel ran on",
    JSON.stringify(u.unexaminedWordingInReport) === "[]",
    JSON.stringify(u.unexaminedWordingInReport),
  );
  const fixed = report.replace(
    "NOT ADJUDICATED: no refuter examined this claim. Gate",
    "The panel ran (2 NOT_REFUTED votes) and the evidence gate demoted it. Gate",
  );
  const [, d2] = run(fixed, { findings: [unexamined, demoted] }).findings;
  check(
    "stays silent once the demoted finding's block says the panel ran",
    JSON.stringify(d2.unexaminedWordingInReport) === "[]",
    JSON.stringify(d2.unexaminedWordingInReport),
  );
}

// --- control: everything compliant, plus not-applicable cases --------------
{
  const report =
    "### Blocking\n1. dup exit-0 bug — a.sh:5 — (found by: lensA, lensB) (also at: b.sh:12; also at: c.sh:9)\n\n" +
    "### Refuted / downgraded\n- other bug -> killed: out-of-scope\n\n" +
    "### Non-blocking / nits\n- typecheck bug -> confirmed by typecheck: TS2345: x\n";
  const metrics = {
    findings: [
      {
        title: "dup exit-0 bug",
        mergedLocations: [
          { file: "a.sh", line: 5 },
          { file: "b.sh", line: 12 },
          { file: "c.sh", line: 9 },
        ],
        mergedFrom: ["lensA", "lensB"],
        votes: [],
        grounded: { verdict: null, diagnostic: null },
      },
      {
        title: "other bug",
        mergedLocations: [],
        mergedFrom: [],
        votes: [{ verdict: "REFUTED", killReason: "out-of-scope" }],
        grounded: { verdict: null, diagnostic: null },
      },
      {
        title: "typecheck bug",
        mergedLocations: [],
        mergedFrom: [],
        votes: [],
        grounded: { verdict: "confirmed", diagnostic: "TS2345: x" },
      },
      { title: "never merged, never refuted, never grounded" },
      { title: "legacy row, fields absent entirely" },
    ],
  };
  const patched = run(report, metrics).findings;
  const allEmpty = (f) =>
    (f.mergedLocationsMissingFromReport || []).length === 0 &&
    (f.killReasonMissingFromReport || []).length === 0 &&
    (f.mergedFromAnnotationMissingFromReport || []).length === 0 &&
    (f.groundedDiagnosticMissingFromReport || []).length === 0 &&
    (f.unexaminedWordingInReport || []).length === 0;
  check(
    "every field stays empty across compliant and not-applicable findings",
    patched.every(allEmpty),
    JSON.stringify(patched.map((f) => ({ title: f.title, ...f }))),
  );
}

// --- error handling: never a silent no-op -----------------------------------
{
  const reportPath = join(DIR, "report-err.md");
  writeFileSync(reportPath, "irrelevant");
  let threw = false;
  try {
    execFileSync("node", [SCRIPT, reportPath], { input: "not json", encoding: "utf8" });
  } catch {
    threw = true;
  }
  check("bad stdin JSON exits non-zero rather than emitting a partial row", threw);

  let threwMissingReport = false;
  try {
    execFileSync("node", [SCRIPT, join(DIR, "does-not-exist.md")], {
      input: JSON.stringify({ findings: [] }),
      encoding: "utf8",
    });
  } catch {
    threwMissingReport = true;
  }
  check("a missing report file exits non-zero rather than treating it as empty", threwMissingReport);
}

// --- anti-drift: this script's private extractor vs the canonical one ------
// Both slice the SAME regions out of the SAME file. If they ever disagree,
// one of them stopped tracking the marker syntax — this is the only thing
// that would notice.
{
  const src = readFileSync(WORKFLOW, "utf8");
  // Literal region names, not variables — replay-region-coverage.mjs greps
  // for loadRegion(<var>, "<literal>" ...) to prove each region is actually
  // exercised, and a variable in that slot is invisible to it.
  const canonicalMerged = loadRegion(WORKFLOW, "MERGED-LOCATIONS-CHECK", [
    "mergedLocationsMissingFromReportText",
  ]);
  const canonicalFidelity = loadRegion(WORKFLOW, "REPORT-FIDELITY-CHECKS", [
    "killReasonMissingFromReportText",
    "mergedFromAnnotationMissingFromReportText",
    "groundedDiagnosticMissingFromReportText",
    "unexaminedWordingInReportText",
  ]);
  const dupMerged = extractRegionDup(src, "MERGED-LOCATIONS-CHECK");
  const dupFidelity = extractRegionDup(src, "REPORT-FIDELITY-CHECKS");

  check(
    "the two extractors slice byte-identical MERGED-LOCATIONS-CHECK text",
    canonicalMerged.body === dupMerged,
    canonicalMerged.body === dupMerged
      ? ""
      : `canonical=${canonicalMerged.body.length} chars, duplicate=${dupMerged.length} chars`,
  );
  check(
    "the two extractors slice byte-identical REPORT-FIDELITY-CHECKS text",
    canonicalFidelity.body === dupFidelity,
    canonicalFidelity.body === dupFidelity
      ? ""
      : `canonical=${canonicalFidelity.body.length} chars, duplicate=${dupFidelity.length} chars`,
  );

  // Behavioural agreement too, not just textual — evaluate both and compare
  // outputs on cases that exercise every branch (missing, present, empty,
  // not-applicable).
  const dup = loadChecks(WORKFLOW);
  const mergedCases = [
    [[{ file: "a.sh", line: 5 }, { file: "b.sh", line: 12 }], "also at: b.sh:12"],
    [[{ file: "a.sh", line: 5 }, { file: "b.sh", line: 12 }], "no mention of the second one"],
    [[{ file: "a.sh", line: 5 }], "single location, nothing to check"],
    [[], "empty mergedLocations"],
  ];
  const mergedAgree = mergedCases.every(([locs, text]) => {
    const a = JSON.stringify(canonicalMerged.api.mergedLocationsMissingFromReportText(locs, text));
    const b = JSON.stringify(dup.mergedLocationsMissingFromReportText(locs, text));
    return a === b;
  });
  check("mergedLocations: the two extractors' functions agree on every case", mergedAgree);

  const killReasonAgree = [
    ["out-of-scope", "no mention"],
    ["out-of-scope", "killed: out-of-scope"],
    [null, "no mention of the fallback"],
    [null, "killed: uncategorized"],
  ].every(([reason, text]) => {
    const a = JSON.stringify(canonicalFidelity.api.killReasonMissingFromReportText(reason, text));
    const b = JSON.stringify(dup.killReasonMissingFromReportText(reason, text));
    return a === b;
  });
  check("killReason: the two extractors' functions agree on every case", killReasonAgree);

  const mergedFromAgree = [
    [["lensA"], "irrelevant, only one lens"],
    [["lensA", "lensB"], "no annotation here"],
    [["lensA", "lensB"], "(found by: lensA, lensB)"],
  ].every(([lenses, text]) => {
    const a = JSON.stringify(canonicalFidelity.api.mergedFromAnnotationMissingFromReportText(lenses, text));
    const b = JSON.stringify(dup.mergedFromAnnotationMissingFromReportText(lenses, text));
    return a === b;
  });
  check("mergedFrom: the two extractors' functions agree on every case", mergedFromAgree);

  const groundedAgree = [
    [{ verdict: null, diagnostic: null }, "irrelevant"],
    [{ verdict: "confirmed", diagnostic: "TS2345: x" }, "nothing here"],
    [{ verdict: "confirmed", diagnostic: "TS2345: x" }, "confirmed by typecheck: TS2345: x"],
  ].every(([grounded, text]) => {
    const a = JSON.stringify(canonicalFidelity.api.groundedDiagnosticMissingFromReportText(grounded, text));
    const b = JSON.stringify(dup.groundedDiagnosticMissingFromReportText(grounded, text));
    return a === b;
  });
  check("grounded: the two extractors' functions agree on every case", groundedAgree);

  const gated = { file: "a.ts", line: 5, votes: [{}], verifiedByGateNote: "x" };
  const unexaminedAgree = [
    [gated, "**T**\n`a.ts:5`\nNOT ADJUDICATED"],
    [gated, "1. T — a.ts:5 — no refuter examined"],
    [gated, "**T**\n`b.ts:9`\nNOT ADJUDICATED\n**U**\n`a.ts:5`\nfine"],
    [{ ...gated, votes: [] }, "**T**\n`a.ts:5`\nNOT ADJUDICATED"],
  ].every(([finding, text]) => {
    const a = JSON.stringify(canonicalFidelity.api.unexaminedWordingInReportText(finding, text));
    const b = JSON.stringify(dup.unexaminedWordingInReportText(finding, text));
    return a === b;
  });
  check("unexamined wording: the two extractors' functions agree on every case", unexaminedAgree);
}

console.log(
  `\nreport-fidelity-check: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`,
);
process.exit(failures.length ? 1 : 0);
