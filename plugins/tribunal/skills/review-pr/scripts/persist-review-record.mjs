#!/usr/bin/env node
// persist-review-record.mjs — Step 4's metrics-persistence half, consolidated.
//
// Why this exists. SKILL.md used to teach this as three separate `node -e`
// one-liners (patch posted/verdict/agentsActual, patch agentCost, and two
// separate backstop-append commands) plus a `find` for the run's transcript
// dir — five-to-six top-level tool calls for what is conceptually one
// operation. Every one of those calls runs in the ORCHESTRATOR's own session
// turn, not inside a subagent — avoidable top-level tool-call volume that
// this script collapses into one call, with the same guards.
//
// Safety invariants preserved from the prose it replaces (references/metrics-schema.md):
//   - Refuses (non-zero exit, no write) if the LAST row's `pr` doesn't match
//     --pr, or (when given) its `headSha` doesn't match --head-sha.
//   - Never appends a second row — only rewrites the located line in place.
//   - Never invents an agentCost value: if --run-id is given but no matching
//     run directory is found, agentCost is left untouched and a warning is
//     printed to stderr.
//   - agent-counts.jsonl / agent-costs.jsonl backstops are independently
//     wrapped so one failing never rolls back the primary metrics.jsonl patch
//     or the other backstop.
//   - --report-file never aborts the patch: an unreadable body or a check that
//     will not load leaves the four fidelity fields untouched and warns, the
//     same posture --run-id already takes for agentCost.
//
// Why --report-file exists. In Workflow mode the row is persisted by the script
// with findings[].*MissingFromReport computed against the WORKFLOW's report,
// and the orchestrator then edits that report before posting it (continuity
// summary, author-refutation verdicts, template reshaping). So the persisted
// fields describe a document nobody published. Recomputing here, against the
// body that actually went to GitHub, is the only point where both exist.
//
// Usage:
//   node persist-review-record.mjs --pr <n> [--repo <owner/name>] --posted <true|false> --verdict <string> \
//     [--head-sha <sha>] [--run-id <wf_...>] [--agent-count <n>] [--project-dir <path>] \
//     [--report-file <path>]

import { readFileSync, writeFileSync, appendFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { METRICS_PATH } from "../eval/rows.mjs";
import { locateLastRow } from "./locate-last-row.mjs";
import { withLedgerLock } from "./ledger-lock.mjs";
import { readRunDir, toMetricsShape, findRunDirs } from "../eval/agent-cost.mjs";
import { projectDir, AGENT_COUNTS_PATH, AGENT_COSTS_PATH } from "../eval/paths.mjs";
import { loadChecks, applyFidelityChecks } from "../../../lib/check-report-fidelity.mjs";

// A strict `import.meta.url === file://${process.argv[1]}` comparison breaks
// when the script is invoked through a symlink (e.g. the ~/.claude/skills/...
// symlink into this plugin's real, version-keyed cache directory): argv[1]
// keeps the symlink path while import.meta.url resolves to the real one, so
// they never match, main() silently never runs, and the script exits 0 having
// done nothing. Resolve both sides to their real path before comparing.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}
const DEFAULT_PROJECT_DIR =
  projectDir();

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) {
      out[key] = true;
    } else {
      out[key] = next;
      i++;
    }
  }
  return out;
}

function findRunDir(projectDir, runId) {
  for (const dir of findRunDirs(projectDir)) {
    if (dir.endsWith(`/${runId}`) || dir.endsWith(runId)) return dir;
  }
  return null;
}

// Recompute findings[].*MissingFromReport against the body that was actually
// posted. Best-effort by design: a missing body or a checks module that will
// not load leaves the recorded values alone and warns, because the primary
// patch (posted/verdict/agentsActual/agentCost) is the thing this script exists
// to land and must not be lost to an audit field. Returns how many findings
// were recomputed, or null when the recompute did not run.
function recomputeFidelity(row, reportPath) {
  if (!Array.isArray(row.findings) || row.findings.length === 0) return null;

  let checks;
  try {
    checks = loadChecks();
  } catch (e) {
    process.stderr.write(
      `warning: could not load the report-fidelity checks (${e.message}); *MissingFromReport left untouched\n`
    );
    return null;
  }

  let reportText;
  try {
    reportText = readFileSync(reportPath, "utf8");
  } catch (e) {
    process.stderr.write(
      `warning: could not read --report-file ${reportPath} (${e.message}); *MissingFromReport left untouched\n`
    );
    return null;
  }

  applyFidelityChecks(row.findings, reportText, checks);
  return row.findings.length;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.pr) {
    process.stderr.write("usage: --pr <n> --posted <true|false> --verdict <string> [--repo <owner/name>] [--head-sha] [--run-id] [--agent-count] [--project-dir] [--report-file]\n");
    process.exit(2);
  }
  const pr = String(args.pr);
  const projectDir = args["project-dir"] || DEFAULT_PROJECT_DIR;

  let patched;
  try {
    patched = withLedgerLock(METRICS_PATH, () => {
      const { row: last, lines, lineIndex } = locateLastRow({ pr, headSha: args["head-sha"], repo: typeof args.repo === "string" ? args.repo : undefined });

      if (args.posted !== undefined) last.posted = args.posted === "true" || args.posted === true;
      if (args.verdict !== undefined) last.verdict = args.verdict;
      if (Array.isArray(last.findings) && args.posted !== undefined) {
        const postedBool = args.posted === "true" || args.posted === true;
        last.findings.forEach((f) => {
          f.posted = postedBool;
        });
      }

      let agentCount = null;
      if (args["agent-count"] !== undefined) {
        agentCount = Number(args["agent-count"]);
        last.agentsActual = agentCount;
      }

      let agentCost = null;
      if (args["run-id"]) {
        const runDir = findRunDir(projectDir, args["run-id"]);
        if (!runDir) {
          process.stderr.write(
            `warning: no run directory found for --run-id ${args["run-id"]} under ${projectDir}; agentCost left untouched\n`
          );
        } else {
          agentCost = toMetricsShape(readRunDir(runDir));
          last.agentCost = agentCost;
        }
      }

      let fidelityRecomputed = null;
      if (args["report-file"] && args["report-file"] !== true) {
        fidelityRecomputed = recomputeFidelity(last, args["report-file"]);
      }

      // Primary patch: single-line rewrite of the located row only, never an append.
      // The read above and this write happen under the ledger lock, so a row another
      // run appends in between is not overwritten.
      lines[lineIndex] = JSON.stringify(last);
      writeFileSync(METRICS_PATH, lines.join("\n") + "\n");
      return { last, agentCount, agentCost, fidelityRecomputed };
    });
  } catch (e) {
    process.stderr.write(`${e.message}\n`);
    process.exit(1);
  }
  const { last, agentCount, agentCost, fidelityRecomputed } = patched;

  // Backstops — independently wrapped so one failing doesn't roll back the
  // primary patch or the other backstop.
  if (agentCount !== null) {
    try {
      appendFileSync(
        AGENT_COUNTS_PATH,
        JSON.stringify({ pr: last.pr, headSha: last.headSha, agentCount, ts: null }) + "\n"
      );
    } catch (e) {
      process.stderr.write(`warning: failed to append agent-counts.jsonl: ${e.message}\n`);
    }
  }
  if (agentCost !== null) {
    try {
      appendFileSync(
        AGENT_COSTS_PATH,
        JSON.stringify({ pr: last.pr, headSha: last.headSha, ...agentCost }) + "\n"
      );
    } catch (e) {
      process.stderr.write(`warning: failed to append agent-costs.jsonl: ${e.message}\n`);
    }
  }

  const summary = {
    pr: last.pr,
    headSha: last.headSha,
    posted: last.posted,
    verdict: last.verdict,
    agentsActual: last.agentsActual,
    agentCostTurns: agentCost ? agentCost.turns : null,
    fidelityRecomputed
  };
  process.stdout.write(JSON.stringify(summary) + "\n");
}

if (isMainModule()) {
  main();
}

export { parseArgs, findRunDir, recomputeFidelity };
