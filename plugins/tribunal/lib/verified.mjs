#!/usr/bin/env node
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

// verified.mjs — the evidence gate on blocking findings, tribunal's copy.
//
// It exists because of a real incident (2026-08-19): a finding went out as
// Blocking claiming that a model "already holds the typed discriminator" that
// should replace a string-prefix check. The enum it named had no member for one
// of the two origins, and both origins produced the same two values, so the
// claim was false. The posted review said every finding had been
// "independently verified by hand against the code at HEAD". It had not. A
// second, structurally identical case surfaced on 2026-08-20 (a run claimed a
// discriminator distinguished two origins; the actual call site only ever
// produced one of the two values).
//
// `evidence` (already required on every tribunal finding) quotes the line
// the diff CHANGED. That is a locator, not proof. `verifiedBy` quotes the
// code that has to be true for the claim to hold — the enum body, the call
// site, the schema field. Transcribing it is the check: you cannot paste
// `enum upload_type { ... }` and not notice what is missing
// from it.
//
// The gate is mechanical and free. It cannot tell whether an anchor actually
// supports the claim; it only makes skipping the lookup impossible to do
// silently.
//
// The trigger: a finding is about to be published as Blocking when it is
// `adjudicated: true` and not a `nit`. See workflows/tribunal.js's VERIFIED-GATE region for the inlined
// copy this file is mirrored against (Workflow scripts have no filesystem/
// import access, so that copy cannot `import` this one — eval/
// replay-verified-gate.mjs runs both against the same fixtures and fails on
// disagreement).
//
// The gate does not hard-block posting on an unanchored finding: a tribunal
// run can cost up to ~1.1M output tokens. Blocking the whole post over one missing citation
// throws that spend away. Instead: an unanchored finding is DEMOTED (never
// dropped) — `adjudicated` flips to `false`, so it renders as an open
// question instead of Blocking, and a note explains why.

const MIN_QUOTE_CHARS = 10;

// A finding is about to be published as Blocking in tribunal's model when it
// survived adjudication (panel or grounded typecheck) and is not a nit.
export function isGatedFinding(f) {
  return !!(f && f.severity !== "nit" && f.adjudicated === true);
}

export function anchorProblems(anchor, i) {
  const where = `verifiedBy[${i}]`;
  if (!anchor || typeof anchor !== "object")
    return [`${where} is not an object`];
  const problems = [];
  if (!anchor.file || typeof anchor.file !== "string")
    problems.push(`${where}.file is missing`);
  if (!Number.isInteger(anchor.line) || anchor.line < 1)
    problems.push(`${where}.line is not a positive integer`);
  const quote = typeof anchor.quote === "string" ? anchor.quote.trim() : "";
  if (quote.length < MIN_QUOTE_CHARS) {
    problems.push(
      `${where}.quote is shorter than ${MIN_QUOTE_CHARS} characters (paste the code the claim rests on, not a label)`,
    );
  }
  return problems;
}

// Per-finding problem list, [] when the finding is not gated or is properly
// anchored. Distinct from checkVerifiedBy (which only reports failures)
// because the caller needs to know WHICH findings to demote, not just whether
// any failed.
export function findingProblems(f) {
  if (!isGatedFinding(f)) return [];
  const anchors = f.verifiedBy;
  if (!Array.isArray(anchors) || anchors.length === 0) {
    return [
      "verifiedBy is missing or empty — a blocking finding must cite the code its claim depends on",
    ];
  }
  return anchors.flatMap((a, i) => anchorProblems(a, i));
}

// Returns [] when every gated finding is anchored. Otherwise one entry per
// offending finding, each naming the finding and what is wrong with it —
// for reporting/logging.
export function checkVerifiedBy(findings) {
  const failures = [];
  for (const f of findings || []) {
    const problems = findingProblems(f);
    if (!problems.length) continue;
    const label = `${f.file || "?"}:${f.line ?? "?"} "${f.title || "untitled"}"`;
    failures.push({ finding: label, problems });
  }
  return failures;
}

// Demotes, never drops: a gated finding without a usable verifiedBy anchor
// loses `adjudicated` (the existing "no Blocking without adjudicated" rule
// then covers it downstream, in both the Workflow prompt and the in-context
// Synthesize step) and carries a note explaining why, so synthesis reports
// it as an open question instead of silently dropping it.
export function demoteUnanchored(findings) {
  return (findings || []).map((f) => {
    const problems = findingProblems(f);
    if (!problems.length) return f;
    return {
      ...f,
      adjudicated: false,
      verifiedByGateNote: `Downgraded from Blocking: ${problems.join("; ")}`,
    };
  });
}

export function formatFailures(failures) {
  const lines = [
    `${failures.length} blocking finding${failures.length === 1 ? "" : "s"} demoted for missing a usable verifiedBy anchor.`,
    "",
    "A finding about to be published as Blocking must carry",
    "verifiedBy: [{file, line, quote}] pointing at the code that must be true",
    "for the claim to hold — not the changed line (that is `evidence`).",
    "See lib/verified.mjs for why.",
    "",
  ];
  for (const f of failures) {
    lines.push(`  ${f.finding}`);
    for (const p of f.problems) lines.push(`    - ${p}`);
  }
  return lines.join("\n") + "\n";
}

// CLI: `node lib/verified.mjs -` reads a JSON array of findings from stdin,
// writes the demoted array to stdout, and prints a summary to stderr. Used
// by the in-context review path (Workflow mode inlines the same logic
// directly in tribunal.js instead — see the module header). Mirrors the
// stdin-JSON-in/stdout-JSON-out shape of resolve-evidence-line.mjs so the
// orchestrator's invocation pattern stays consistent across gates.
async function main() {
  const arg = process.argv[2];
  if (arg !== "-") {
    process.stderr.write(
      "Usage: node lib/verified.mjs -   (reads findings JSON array from stdin)\n",
    );
    process.exit(1);
  }
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const findings = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const failures = checkVerifiedBy(findings);
  const demoted = demoteUnanchored(findings);
  if (failures.length) process.stderr.write(formatFailures(failures));
  process.stdout.write(JSON.stringify(demoted, null, 2) + "\n");
}

// Same isMainModule guard as every other script here — resolving through a
// symlink (the ~/.claude/skills/... link into this plugin's real cache
// directory) breaks a literal argv[1]-vs-import.meta.url comparison. See
// eval/paths.mjs for the full rationale; duplicated in miniature for the same
// reason extractRegion is (see the header comment on the eval/lib boundary).
function isMainModule() {
  try {
    return (
      realpathSync(process.argv[1]) ===
      realpathSync(fileURLToPath(import.meta.url))
    );
  } catch {
    return false;
  }
}

if (isMainModule()) main();
