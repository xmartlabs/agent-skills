#!/usr/bin/env node
// replay-report-verdict.mjs — the report's verdict line is the harness verdict.
//
// The defect (2026-10-17, BE #2171, wf_9357d96b-553, installed v78). The clean
// sweep raised one candidate, the panel killed it, and verdictFor returned
// APPROVE, which the row recorded. The report writer chose its own verdict
// line, read the sweep's `clean: false` as an unconfirmed sweep, and wrote
// "Verdict suggestion: COMMENT ... The review is unfinished, not clean". --post
// submits that line as the review state, so a clean PR would have been posted
// as COMMENT while its row said APPROVE.
//
// This runs the shipping REPORT-VERDICT region against that exact report, then
// checks the wiring around it and that audit-run.mjs fails a corrected run.
//
// Usage: node eval/replay-report-verdict.mjs

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { FIXTURES_DIR, lib, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
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
  loaded = loadRegion(WORKFLOW, "REPORT-VERDICT", ["pinReportVerdict"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
console.log(`\nREPORT-VERDICT region extracted from ${WORKFLOW}`);
check("the region is pure", violations.length === 0, violations.join("; "));

const VERDICT_LINE = /^#{1,6}[ \t]*Verdict suggestion:.*$/gim;
const verdictLines = (t) => t.match(VERDICT_LINE) || [];

console.log(`\n--- BE #2171: the report said COMMENT, the harness said APPROVE ---`);
{
  const real = readFileSync(
    join(FIXTURES_DIR, "report-verdict", "be-2171-wf_9357d96b-553.md"),
    "utf8",
  );
  check(
    "the fixture carries the split as recorded",
    verdictLines(real).length === 1 && /COMMENT/.test(verdictLines(real)[0]),
  );
  const out = api.pinReportVerdict(real, "APPROVE");
  check(
    "the verdict line is rewritten to APPROVE",
    verdictLines(out.text).length === 1 &&
      verdictLines(out.text)[0] === "### Verdict suggestion: APPROVE",
    verdictLines(out.text).join(" | "),
  );
  check(
    "the correction is recorded as a mismatch from COMMENT",
    out.corrected &&
      out.corrected.from === "COMMENT" &&
      out.corrected.to === "APPROVE" &&
      out.corrected.reason === "mismatch",
    JSON.stringify(out.corrected),
  );
  check(
    "nothing but the verdict line changed",
    out.text.replace(VERDICT_LINE, "") === real.replace(VERDICT_LINE, ""),
  );
}

console.log(`\n--- the other shapes a writer can return ---`);
{
  const title = "## Peer review: PR #1 — t";
  const agree = `${title}\n\n### Verdict suggestion: REQUEST CHANGES\n\nbody`;
  const a = api.pinReportVerdict(agree, "REQUEST CHANGES");
  check("a matching line is left alone", a.corrected === null && a.text === agree);

  const m = api.pinReportVerdict(`${title}\n\nbody $& $1`, "COMMENT");
  check(
    "a missing line is inserted under the title",
    m.text === `${title}\n\n### Verdict suggestion: COMMENT\n\nbody $& $1` &&
      m.corrected.reason === "missing" &&
      m.corrected.from === null,
    JSON.stringify(m.text),
  );

  const tpl = `${title}\n\n### Verdict suggestion: APPROVE | REQUEST CHANGES | COMMENT\n`;
  const t = api.pinReportVerdict(tpl, "APPROVE");
  check(
    "the unfilled template line counts as unreadable and is replaced",
    t.corrected?.reason === "unreadable" &&
      verdictLines(t.text)[0] === "### Verdict suggestion: APPROVE",
  );

  const two = `${title}\n### Verdict suggestion: APPROVE\nx\n## Verdict suggestion: COMMENT\n`;
  const w = api.pinReportVerdict(two, "APPROVE");
  check(
    "two verdict lines both become the harness verdict",
    w.corrected?.reason === "multiple" &&
      w.corrected.from === "COMMENT" &&
      verdictLines(w.text).every((l) => l === "### Verdict suggestion: APPROVE"),
  );

  const lower = `${title}\n\n### verdict suggestion: approve\n`;
  check(
    "case does not matter when reading the stated verdict",
    api.pinReportVerdict(lower, "APPROVE").corrected === null,
  );
}

console.log(`\n--- wiring in the workflow ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const at = (s) => src.indexOf(s);
  check(
    "the verdict is decided before the report is written",
    at("const verdictGuess =") !== -1 &&
      at("const verdictGuess =") < at("let report = await agent("),
  );
  check(
    "the synthesis template prints that verdict",
    src.includes("### Verdict suggestion: ${verdictGuess}") &&
      !src.includes("### Verdict suggestion: APPROVE | REQUEST CHANGES | COMMENT"),
  );
  const pinAt = at("pinReportVerdict(report, verdictGuess)");
  check(
    "the report is pinned right after synthesis, before anything reads it",
    pinAt > at('label: "synthesize"') &&
      pinAt < at("stripDraftCommentsFromBody(report)") &&
      /report = reportVerdictPin\.text;/.test(src),
  );
  check(
    "the row records the correction",
    /reportVerdictCorrected: reportVerdictPin\.corrected,/.test(src),
  );
}

console.log(`\n--- the self-audit fails a corrected run ---`);
{
  const audit = (metrics) =>
    JSON.parse(
      execFileSync("node", [lib("audit-run.mjs"), ""], {
        input: JSON.stringify(metrics),
        encoding: "utf8",
      }).trim(),
    );
  const corrected = audit({
    findings: [],
    reportVerdictCorrected: { from: "COMMENT", to: "APPROVE", reason: "mismatch" },
  });
  check(
    "a corrected verdict line is a violation",
    corrected.passed === false &&
      corrected.violations.some((x) => x.id === "report-verdict-corrected"),
    JSON.stringify(corrected.violations),
  );
  const clean = audit({ findings: [], reportVerdictCorrected: null });
  check(
    "an untouched verdict line is not",
    !clean.violations.some((x) => x.id === "report-verdict-corrected"),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall report-verdict expectations hold");
