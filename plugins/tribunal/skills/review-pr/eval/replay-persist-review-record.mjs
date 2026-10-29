#!/usr/bin/env node
// replay-persist-review-record.mjs — Tier 0 guard for scripts/persist-review-record.mjs.
//
// Why this exists. The script had no replay at all: replay-post-footer.mjs and
// paths.mjs only reference its path. That was survivable while it did nothing
// but copy four scalars onto a located row, and stopped being survivable when
// --report-file gave it a second job — recomputing findings[].*MissingFromReport
// against the body that was actually POSTED.
//
// That recompute matters because in Workflow mode those fields are written by
// the workflow against ITS report, and the orchestrator then edits the report
// before posting it (continuity summary, author-refutation verdicts, template
// reshaping). Without the recompute the persisted row describes a document
// nobody published. Caught on PR #2082, 2026-09-04, where the same four checks
// flagged two real gaps only AFTER the review was public.
//
// The script writes to METRICS_PATH, so every case here runs it as a subprocess
// with TRIBUNAL_DATA_DIR pointed at a throwaway directory. Nothing in this file
// touches the real metrics.jsonl.

import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SKILL_DIR } from "./paths.mjs";

const SCRIPT = join(SKILL_DIR, "scripts", "persist-review-record.mjs");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`  using: ${SCRIPT}`);

// A row shaped like the real thing on every field the four checks read. The
// finding is deliberately non-compliant against NONCOMPLIANT_BODY: two merged
// locations (only the first is exempt), a REFUTED vote carrying a killReason, a
// two-lens mergedFrom, and a confirmed typecheck diagnostic.
const OTHER_ROW = { pr: 1111, headSha: "aaaaaaa", verdict: "APPROVE", posted: true, findings: [] };

function targetRow() {
  return {
    pr: 2082,
    headSha: "28fc4cc3",
    verdict: null,
    posted: false,
    agentsActual: null,
    findings: [
      {
        title: "a finding that was merged and refuted",
        file: "src/a.ts",
        line: 10,
        mergedLocations: [
          { file: "src/a.ts", line: 10 },
          { file: "src/b.ts", line: 99 },
        ],
        mergedFrom: ["data-contract", "control-flow"],
        votes: [{ source: "claude", verdict: "REFUTED", confidence: 0.7, killReason: "pre-existing" }],
        grounded: { verdict: "confirmed", diagnostic: "TS2345: nope" },
        mergedLocationsMissingFromReport: ["STALE"],
        killReasonMissingFromReport: ["STALE"],
        mergedFromAnnotationMissingFromReport: ["STALE"],
        groundedDiagnosticMissingFromReport: ["STALE"],
      },
      {
        title: "a finding the evidence gate demoted after its panel voted",
        file: "src/c.ts",
        line: 7,
        votes: [{ source: "claude", verdict: "NOT_REFUTED", confidence: 0.9 }],
        verifiedByGateNote: "Downgraded from Blocking: verifiedBy is missing or empty",
        unexaminedWordingInReport: ["STALE"],
      },
    ],
  };
}

const NONCOMPLIANT_BODY =
  "# Review\n\nA finding. Nothing cited here.\n\n**Demoted**\n`src/c.ts:7`\n\nNOT ADJUDICATED: no refuter examined this claim.\n";
const COMPLIANT_BODY = [
  "# Review",
  "",
  "A finding (found by: data-contract, control-flow), also at: `src/b.ts:99`.",
  "Killed as pre-existing. Typecheck said TS2345: nope.",
  "",
  "**Demoted**",
  "`src/c.ts:7`",
  "",
  "The panel ran (1 NOT_REFUTED vote) and the evidence gate demoted it.",
  "",
].join("\n");

function setup() {
  const dir = mkdtempSync(join(tmpdir(), "persist-review-record-"));
  writeFileSync(
    join(dir, "metrics.jsonl"),
    [JSON.stringify(OTHER_ROW), JSON.stringify(targetRow())].join("\n") + "\n"
  );
  return dir;
}

function run(dir, extraArgs) {
  const args = [SCRIPT, "--pr", "2082", "--head-sha", "28fc4cc3", "--posted", "true", "--verdict", "REQUEST CHANGES", "--agent-count", "7", ...extraArgs];
  const stdout = execFileSync("node", args, {
    encoding: "utf8",
    env: { ...process.env, TRIBUNAL_DATA_DIR: dir },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const lines = readFileSync(join(dir, "metrics.jsonl"), "utf8").trim().split("\n");
  return { summary: JSON.parse(stdout), lines, rows: lines.map((l) => JSON.parse(l)) };
}

function body(dir, text) {
  const p = join(dir, `body-${Math.floor(Math.random() * 1e9)}.md`);
  writeFileSync(p, text);
  return p;
}

// 1. Non-compliant body: every rule reports what the report is missing.
{
  const dir = setup();
  const { rows } = run(dir, ["--report-file", body(dir, NONCOMPLIANT_BODY)]);
  const f = rows[1].findings[0];
  // The key is bare `file:line`, no backticks — MERGED-LOCATIONS-CHECK in
  // workflows/tribunal.js maps to `${loc.file}:${loc.line}` and substring-tests
  // that against the report, so any surrounding markup in the report still
  // matches. Asserted against the rule, not against how a report renders it.
  check(
    "non-compliant body: the dropped extra location is reported",
    JSON.stringify(f.mergedLocationsMissingFromReport) === JSON.stringify(["src/b.ts:99"]),
    JSON.stringify(f.mergedLocationsMissingFromReport)
  );
  check(
    "non-compliant body: the uncited killReason is reported",
    JSON.stringify(f.killReasonMissingFromReport) === JSON.stringify(["pre-existing"]),
    JSON.stringify(f.killReasonMissingFromReport)
  );
  check(
    "non-compliant body: the missing (found by: ...) annotation is reported",
    Array.isArray(f.mergedFromAnnotationMissingFromReport) &&
      f.mergedFromAnnotationMissingFromReport.includes("(found by:"),
    JSON.stringify(f.mergedFromAnnotationMissingFromReport)
  );
  check(
    "non-compliant body: the uncited typecheck diagnostic is reported",
    JSON.stringify(f.groundedDiagnosticMissingFromReport) === JSON.stringify(["TS2345: nope"]),
    JSON.stringify(f.groundedDiagnosticMissingFromReport)
  );
  const demoted = rows[1].findings[1];
  check(
    "non-compliant body: the unexamined marker on the demoted finding is reported",
    JSON.stringify(demoted.unexaminedWordingInReport) ===
      JSON.stringify(["NOT ADJUDICATED", "no refuter examined"]),
    JSON.stringify(demoted.unexaminedWordingInReport)
  );
}

// 2. Compliant body: all four clear, and the stale placeholders are gone. This
//    is the control — without it, case 1 could pass on a script that always
//    reports everything as missing.
{
  const dir = setup();
  const { rows, summary } = run(dir, ["--report-file", body(dir, COMPLIANT_BODY)]);
  const f = rows[1].findings[0];
  const allEmpty = [
    f.mergedLocationsMissingFromReport,
    f.killReasonMissingFromReport,
    f.mergedFromAnnotationMissingFromReport,
    f.groundedDiagnosticMissingFromReport,
  ].every((v) => Array.isArray(v) && v.length === 0);
  check("compliant body: all four fields clear to []", allEmpty, JSON.stringify(f));
  check(
    "compliant body: the demoted finding's marker clears to []",
    JSON.stringify(rows[1].findings[1].unexaminedWordingInReport) === "[]",
    JSON.stringify(rows[1].findings[1])
  );
  check("compliant body: summary reports the recompute ran", summary.fidelityRecomputed === 2, JSON.stringify(summary));
}

// 3. --report-file omitted: the recorded values are left exactly as found. This
//    is the --no-post branch, where there is no posted body to check against.
{
  const dir = setup();
  const { rows, summary } = run(dir, []);
  const f = rows[1].findings[0];
  check(
    "no --report-file: the recorded fields are left untouched",
    JSON.stringify(f.mergedLocationsMissingFromReport) === JSON.stringify(["STALE"]) &&
      JSON.stringify(f.killReasonMissingFromReport) === JSON.stringify(["STALE"]) &&
      JSON.stringify(rows[1].findings[1].unexaminedWordingInReport) === JSON.stringify(["STALE"]),
    JSON.stringify(f)
  );
  check("no --report-file: summary reports no recompute", summary.fidelityRecomputed === null, JSON.stringify(summary));
}

// 4. Unreadable body: warn, leave the audit fields alone, and STILL land the
//    primary patch. An audit field must never cost the row its verdict.
{
  const dir = setup();
  const { rows, summary } = run(dir, ["--report-file", join(dir, "does-not-exist.md")]);
  const f = rows[1].findings[0];
  check(
    "unreadable body: audit fields untouched",
    JSON.stringify(f.mergedLocationsMissingFromReport) === JSON.stringify(["STALE"]),
    JSON.stringify(f.mergedLocationsMissingFromReport)
  );
  check(
    "unreadable body: the primary patch still lands",
    rows[1].posted === true && rows[1].verdict === "REQUEST CHANGES" && rows[1].agentsActual === 7,
    JSON.stringify({ posted: rows[1].posted, verdict: rows[1].verdict, agentsActual: rows[1].agentsActual })
  );
  check("unreadable body: summary reports no recompute", summary.fidelityRecomputed === null, JSON.stringify(summary));
}

// 5. The row is rewritten in place. Never an append, and never a neighbour.
{
  const dir = setup();
  const before = readFileSync(join(dir, "metrics.jsonl"), "utf8").trim().split("\n");
  const { lines, rows } = run(dir, ["--report-file", body(dir, COMPLIANT_BODY)]);
  check("row count unchanged (no append)", lines.length === before.length, `${before.length} -> ${lines.length}`);
  check("the preceding row is byte-identical", lines[0] === before[0]);
  check("the patched row is the pr 2082 one", rows[1].pr === 2082 && rows[1].posted === true);
}

// 6. --run-id: agentCost lands with the exact inputUnits and outputExact: false,
//    on the row and on the agent-costs.jsonl backstop. Output in a subagent
//    transcript is a placeholder (2026-10-17), so the row must say so.
{
  const dir = setup();
  const project = mkdtempSync(join(tmpdir(), "persist-review-record-project-"));
  const runDir = join(project, "session-1", "subagents", "workflows", "wf_replay-persist");
  mkdirSync(runDir, { recursive: true });
  writeFileSync(join(runDir, "journal.jsonl"), JSON.stringify({ type: "started", agentId: "a1" }) + "\n");
  writeFileSync(
    join(runDir, "agent-a1.jsonl"),
    [
      JSON.stringify({ type: "user", message: { content: "Review PR #2082 in repo x." } }),
      JSON.stringify({
        type: "assistant",
        message: {
          id: "msg_1",
          role: "assistant",
          usage: { input_tokens: 100, cache_creation_input_tokens: 1000, cache_read_input_tokens: 10000, output_tokens: 50 },
        },
      }),
    ].join("\n") + "\n"
  );
  const { rows } = run(dir, ["--run-id", "wf_replay-persist", "--project-dir", project]);
  const cost = rows[1].agentCost || {};
  check("--run-id: agentCost.inputUnits is exact (100 + 1000*1.25 + 10000*0.1)", cost.inputUnits === 2350, JSON.stringify(cost));
  check("--run-id: agentCost.outputExact is false", cost.outputExact === false, JSON.stringify(cost.outputExact));
  check("--run-id: existing fields keep their values", cost.inputTokens === 11100 && cost.outputTokens === 50, JSON.stringify(cost));
  const backstop = readFileSync(join(dir, "agent-costs.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l));
  check(
    "--run-id: the agent-costs.jsonl backstop carries the same fields",
    backstop.length === 1 && backstop[0].inputUnits === 2350 && backstop[0].outputExact === false,
    JSON.stringify(backstop)
  );
}

console.log(
  failures.length === 0
    ? "\npersist-review-record: all checks pass"
    : `\n${failures.length} check(s) FAILED: ${failures.join(", ")}`
);
process.exit(failures.length === 0 ? 0 : 1);
