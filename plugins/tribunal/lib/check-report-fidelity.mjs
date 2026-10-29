#!/usr/bin/env node
// check-report-fidelity.mjs — deterministic patch step: for every finding,
// checks whether the report actually said what the structured data says it
// should have, and writes the answers onto findings[].*MissingFromReport so
// lib/audit-run.mjs's invariants can see them. Covers four grounding rules:
// mergedLocations' "also at: <file>:<line>", killReason's citation,
// mergedFrom's "(found by: ...)" note, and grounded.verdict's "(confirmed by
// typecheck: ...)" citation. Named after all four now that it covers more
// than the first one (was lib/check-merged-locations.mjs). A fifth rule,
// unexaminedWordingInReport, is the inverse: the unexamined marker printed on
// a gate-demoted finding whose panel voted.
//
// WHY THIS IS A SCRIPT, NOT A PROSE INSTRUCTION TO WHOEVER IS BUILDING THE
// ROW. The synthesis prompt (Workflow mode) and this skill's own report
// template (in-context mode, references/in-context-review.md) already TELL
// the reviewer to print each of these — see mergeCandidates()'s comment in
// workflows/tribunal.js: a merge that drops its extra locations is how the
// PR #1911 run lost four defects silently. Asking whoever assembles the
// metrics row to re-read the report and eyeball "is this string in there" is
// the SAME class of failure the check exists to catch — a long pass
// re-reading its own prior output under time pressure is exactly where a
// rule gets dropped. This makes it a mechanical string containment check
// instead, with no judgment call available to skip it.
//
// ONE IMPLEMENTATION, NOT TWO. The actual comparisons
// (mergedLocationsMissingFromReportText, killReasonMissingFromReportText,
// mergedFromAnnotationMissingFromReportText,
// groundedDiagnosticMissingFromReportText) are sliced out of
// workflows/tribunal.js by its `MERGED-LOCATIONS-CHECK` and
// `REPORT-FIDELITY-CHECKS` region markers — the exact trick
// eval/replay-gates.mjs uses for the panel-decision rules, for the same
// reason: a hand-copied second implementation is a copy that can drift
// silently and nothing would notice, on the Workflow-mode call site AND on
// this one. The marker-slicing primitive itself (extractRegion, below) is
// duplicated in miniature rather than imported from
// eval/extract-pure-region.mjs, because eval/ is deliberately excluded from
// the hashed harness surface (scripts/harness-version.mjs's own comment: "a
// replay is a test of the harness, not a part of it") while this file is
// part of it — importing across that boundary would make a hashed,
// behaviour-affecting script depend on code the hash does not cover.
// eval/replay-report-fidelity-check.mjs asserts this file's extractor and
// the canonical one agree, so a reshaped marker syntax cannot drift between
// them unnoticed either.
//
// Usage:
//   node lib/check-report-fidelity.mjs <report-file> < metrics.json > metrics.json
//
// Exit non-zero (and print nothing) if a shared region cannot be found or
// fails to evaluate, or if metrics on stdin are not valid JSON — a check that
// silently no-ops on a broken input is worse than one that fails loudly, the
// same rule lib/audit-run.mjs follows for an unreadable metrics payload.

import { readFileSync, realpathSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "workflows", "tribunal.js");

export function extractRegion(src, name) {
  const BEGIN = `// ===== BEGIN ${name} =====`;
  const END = `// ===== END ${name} =====`;
  const b = src.indexOf(BEGIN);
  if (b === -1) throw new Error(`no ${name} region found`);
  const e = src.indexOf(END, b);
  if (e === -1) throw new Error(`unterminated ${name} region`);
  return src.slice(b + BEGIN.length, e);
}

// Generalized from the original single-region loadCheck(): slice one named
// region and return the named exports it evaluates to.
function loadRegionExports(src, regionName, exportNames) {
  const body = extractRegion(src, regionName);
  const result = new Function(
    `${body}\nreturn { ${exportNames.join(", ")} };`,
  )();
  for (const name of exportNames) {
    if (typeof result[name] !== "function")
      throw new Error(`${regionName} region did not export ${name}`);
  }
  return result;
}

export function loadChecks(workflowPath = WORKFLOW) {
  const src = readFileSync(workflowPath, "utf8");
  const merged = loadRegionExports(src, "MERGED-LOCATIONS-CHECK", [
    "mergedLocationsMissingFromReportText",
  ]);
  // The body that will actually be posted, not the file as drafted. Sliced from
  // the same workflow regions as the checks themselves so both paths strip
  // identically; skills/review-pr/scripts/strip-draft-comments.mjs remains the
  // authority for the posting strip, and eval/replay-draft-comments-strip.mjs
  // fails if the two ever disagree.
  const strip = loadRegionExports(src, "DRAFT-COMMENTS-STRIP", [
    "stripDraftCommentsFromBody",
  ]);
  const fidelity = loadRegionExports(src, "REPORT-FIDELITY-CHECKS", [
    "killReasonMissingFromReportText",
    "mergedFromAnnotationMissingFromReportText",
    "groundedDiagnosticMissingFromReportText",
    "unexaminedWordingInReportText",
  ]);
  return {
    stripDraftCommentsFromBody: strip.stripDraftCommentsFromBody,
    mergedLocationsMissingFromReportText: merged.mergedLocationsMissingFromReportText,
    killReasonMissingFromReportText: fidelity.killReasonMissingFromReportText,
    mergedFromAnnotationMissingFromReportText: fidelity.mergedFromAnnotationMissingFromReportText,
    groundedDiagnosticMissingFromReportText: fidelity.groundedDiagnosticMissingFromReportText,
    unexaminedWordingInReportText: fidelity.unexaminedWordingInReportText,
  };
}

// The loop that applies the five rules, factored out so the CLI below and
// scripts/persist-review-record.mjs (which recomputes these against the body
// that was actually POSTED, not the one the Workflow generated) share one
// definition. Mutates `findings` in place and returns it.
export function applyFidelityChecks(findings, reportText, checks) {
  const list = Array.isArray(findings) ? findings : [];
  for (const f of list) {
    if (!f || typeof f !== "object") continue;
    f.mergedLocationsMissingFromReport = checks.mergedLocationsMissingFromReportText(
      f.mergedLocations || [],
      reportText,
    );
    const refutedVote = Array.isArray(f.votes)
      ? f.votes.find((vote) => vote && vote.verdict === "REFUTED")
      : null;
    f.killReasonMissingFromReport = refutedVote
      ? checks.killReasonMissingFromReportText(refutedVote.killReason, reportText)
      : [];
    f.mergedFromAnnotationMissingFromReport = checks.mergedFromAnnotationMissingFromReportText(
      f.mergedFrom || [],
      reportText,
    );
    f.groundedDiagnosticMissingFromReport = checks.groundedDiagnosticMissingFromReportText(
      f.grounded || { verdict: null, diagnostic: null },
      reportText,
    );
    f.unexaminedWordingInReport = checks.unexaminedWordingInReportText(f, reportText);
  }
  return list;
}

function main() {
  const reportPath = process.argv[2];
  if (!reportPath) {
    console.error(
      "usage: check-report-fidelity.mjs <report-file> < metrics.json > metrics.json",
    );
    process.exit(1);
  }

  let checks;
  try {
    checks = loadChecks();
  } catch (err) {
    console.error(`check-report-fidelity: could not load the shared checks — ${err.message}`);
    process.exit(1);
  }

  let reportText;
  try {
    reportText = readFileSync(reportPath, "utf8");
  } catch (err) {
    console.error(`check-report-fidelity: could not read ${reportPath} — ${err.message}`);
    process.exit(1);
  }

  let metrics;
  try {
    metrics = JSON.parse(readFileSync(0, "utf8"));
  } catch (err) {
    console.error(`check-report-fidelity: metrics on stdin were not valid JSON — ${err.message}`);
    process.exit(1);
  }

  // Check the body GitHub will receive, never the file as drafted. The two
  // differ by the "### Draft comments" section, and an `also at:` location or a
  // killReason drafted only there used to satisfy this gate and then be
  // stripped on the way out — the gate reporting clean on text no reader would
  // ever see, which is the PR #2082 failure it exists to prevent.
  const postedText = checks.stripDraftCommentsFromBody
    ? checks.stripDraftCommentsFromBody(reportText)
    : reportText;

  applyFidelityChecks(metrics.findings, postedText, checks);

  process.stdout.write(JSON.stringify(metrics));
}

// Same isMainModule guard as every other script here — resolving through a
// symlink (the ~/.claude/skills/... link into this plugin's real cache
// directory) breaks a literal argv[1]-vs-import.meta.url comparison. See
// eval/paths.mjs for the full rationale; duplicated in miniature for the same
// reason extractRegion is (see the header comment on the eval/lib boundary).
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isMainModule()) main();
