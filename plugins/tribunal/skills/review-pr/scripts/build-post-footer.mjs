#!/usr/bin/env node
// build-post-footer.mjs — the SOLE authority for the posted-comment metadata
// footer. The orchestrator must never hand-compose this line: every number in
// it either comes off an already-persisted metrics.jsonl row (Workflow mode)
// or was computed by a script the orchestrator ran itself earlier this same
// review (in-context mode) — never recalled or retyped. Same discipline as
// every other number this harness reports (RELEASES.md: "a fix that actually
// held was a script, not a stronger sentence").
//
// Two invocation shapes:
//
//   --mode workflow --pr <n> --head-sha <sha> --agent-count <n>
//     Reads harnessVersion/reviewMode/codexModelUsed/wallClockSeconds/
//     agentCost.{inputTokens,outputTokens}/ts off the LAST metrics.jsonl row
//     (outputTokens is printed as a lower bound, "≥Y out")
//     matching pr+headSha (via locate-last-row.mjs — refuses, no output, on
//     mismatch or an empty log). By the time Step 4 runs, that row already
//     exists and is complete except agentsActual/posted/verdict — the
//     Workflow's own persist-metrics step wrote it before the Workflow tool
//     call returned. agentCount is NOT yet on that row at this point in the
//     sequence, hence the explicit flag.
//
//   --mode in-context --harness-version <id> --review-mode <full|incremental>
//     --agent-count <n> [--codex-model-json <json>] [--timestamp <iso>]
//     No row exists yet (in-context mode's row is appended AFTER posting) —
//     every field is an explicit flag, because the orchestrator already holds
//     all of them from earlier in this same review (harness-version.mjs,
//     Step 2.5's reconcile-votes.mjs call, Step 1's routing decision). This
//     mode deliberately has NO --input-tokens/--output-tokens/--wall-clock
//     flags at all — in-context has zero token-cost visibility, ever
//     (SKILL.md, metrics-schema.md), and the absence of the flag makes it
//     structurally impossible to wire up a fake value by accident.
//
// Usage/output:
//   node build-post-footer.mjs --mode workflow --pr 1947 --head-sha 3cf9542a... --agent-count 36
//   node build-post-footer.mjs --mode in-context --harness-version pr-harness-49366450a283 \
//     --review-mode incremental --agent-count 9
//   -> prints one line to stdout: <sub>...</sub>\n
//
// Exit codes: 0 on success (one line on stdout). 2 on a usage error. 1 when
// --mode workflow's row lookup refuses (RowMismatchError) or the assembled
// fields fail validation — nothing is printed to stdout in either case.

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { locateLastRow } from "./locate-last-row.mjs";

function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) out[key] = true;
    else {
      out[key] = next;
      i++;
    }
  }
  return out;
}

// --- pure formatting, independently testable ---------------------------

// Rounds to 1 decimal, K under 1M, M at/above. The 999_950 boundary exists so
// a value that would print as "1000.0K" (e.g. 999,975) prints as "1.0M"
// instead — toFixed(1) rounds the K-string up past 999.9 right at that edge.
export function formatTokenCount(n) {
  if (!Number.isFinite(n)) return String(n);
  if (Math.abs(n) >= 999_950) return `${(n / 1_000_000).toFixed(1)}M`;
  if (Math.abs(n) >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export function formatWallClock(seconds) {
  if (!Number.isFinite(seconds)) return String(seconds);
  if (seconds < 60) return `${seconds}s`;
  return `${Math.round(seconds / 60)}m`;
}

// null | string | string[] -> display string | null (never collapse an array
// to one value — a mid-run config.toml change is exactly what this field
// exists to surface, per metrics-schema.md's "Field added 2026-07-31").
export function formatCodexModel(value) {
  if (value === null || value === undefined) return null;
  if (Array.isArray(value)) return value.length ? value.join(", ") : null;
  return String(value);
}

export function isoTimestamp(d = new Date()) {
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

// The version in the plugin.json of the plugin this script ships in, for rows
// that do not record their own (every row the Workflow writes).
export function ownPluginVersion(root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..")) {
  try {
    return JSON.parse(readFileSync(join(root, ".claude-plugin", "plugin.json"), "utf8")).version ?? null;
  } catch {
    return null;
  }
}

// Which engine ran the review and from which plugin version: "v105 (skill)"
// for the Workflow, "v105 (rust rust-<hash>)" for the Rust binary. A row with
// no `engine` was written by the Workflow.
export function engineLabel(row, fallbackVersion) {
  const version = row.pluginVersion ?? fallbackVersion;
  const major = typeof version === "string" && version ? `v${version.split(".")[0]}` : null;
  const kind = row.engine === "rust" ? (row.engineVersion ? `rust ${row.engineVersion}` : "rust") : "skill";
  return major ? `${major} (${kind})` : `(${kind})`;
}

// Pure: known fields -> the exact footer text (including the <sub> wrapper).
// Every omission rule lives here and only here.
export function buildFooterLine(fields) {
  const {
    engine = null,
    harnessVersion,
    reviewMode,
    codexModelUsed = null,
    agentCount,
    inputTokens = null,
    outputTokens = null,
    wallClockSeconds = null,
    timestamp,
  } = fields;

  if (!harnessVersion) throw new Error("buildFooterLine: harnessVersion is required");
  if (reviewMode !== "full" && reviewMode !== "incremental")
    throw new Error(
      `buildFooterLine: reviewMode must be "full" or "incremental", got ${JSON.stringify(reviewMode)}`
    );
  if (!Number.isInteger(agentCount) || agentCount < 0)
    throw new Error(`buildFooterLine: agentCount must be a non-negative integer, got ${JSON.stringify(agentCount)}`);
  if (!timestamp) throw new Error("buildFooterLine: timestamp is required");

  const segments = [engine ? `tribunal ${engine}` : "tribunal", harnessVersion, `${reviewMode} review`];

  const codexText = formatCodexModel(codexModelUsed);
  if (codexText) segments.push(`codex ${codexText}`);

  segments.push(agentCount === 1 ? "1 agent" : `${agentCount} agents`);

  // The only source of outputTokens is agentCost, read from subagent
  // transcripts whose output_tokens is a placeholder: a lower bound, so the
  // footer says so. Input is exact.
  if (Number.isFinite(inputTokens) && Number.isFinite(outputTokens)) {
    segments.push(`${formatTokenCount(inputTokens)} in / ≥${formatTokenCount(outputTokens)} out tokens`);
  }

  if (Number.isFinite(wallClockSeconds)) {
    segments.push(`${formatWallClock(wallClockSeconds)} wall`);
  }

  segments.push(timestamp);

  return `<sub>${segments.join(" · ")}</sub>`;
}

// --- CLI -----------------------------------------------------------------

// The in-context plan (in-context-plan.mjs, capped by build-lens-prompts.mjs at
// the profile's distinct foci) is the contract for the run, and until v100 it
// was only compared with what ran after the fact, in --review-health. A footer
// is required to post, so refusing it here stops an under-planned review from
// going out unless the orchestrator declares why.
export function planShortfall({ lensesPlanned, lensesDispatched, criticPlanned, criticRan }) {
  const short = [];
  if (lensesDispatched < lensesPlanned)
    short.push(`${lensesDispatched} lens(es) dispatched, ${lensesPlanned} planned`);
  if (criticPlanned && !criticRan) short.push("the plan runs the completeness critic and it did not run");
  return short.length ? short.join("; ") : null;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const mode = args.mode;

  if (mode !== "workflow" && mode !== "in-context") {
    process.stderr.write(
      "usage: --mode workflow --pr <n> --head-sha <sha> --agent-count <n>\n" +
        "   or: --mode in-context --harness-version <id> --review-mode <full|incremental> " +
        "--agent-count <n> --lenses-planned <n> --lenses-dispatched <n> --critic-planned <bool> " +
        "--critic-ran <bool> [--plan-deviation <reason>] [--codex-model-json <json>] [--timestamp <iso>]\n"
    );
    process.exit(2);
  }

  let fields;
  if (mode === "workflow") {
    if (!args.pr || !args["head-sha"] || args["agent-count"] === undefined) {
      process.stderr.write("--mode workflow requires --pr, --head-sha and --agent-count\n");
      process.exit(2);
    }
    let located;
    try {
      located = locateLastRow({ pr: args.pr, headSha: args["head-sha"] });
    } catch (e) {
      process.stderr.write(`${e.message}\n`);
      process.exit(1);
    }
    const row = located.row;
    const agentCost = row.agentCost || {};
    fields = {
      engine: engineLabel(row, ownPluginVersion()),
      harnessVersion: row.harnessVersion,
      reviewMode: row.reviewMode,
      codexModelUsed: row.codexModelUsed ?? null,
      agentCount: Number(args["agent-count"]),
      inputTokens: Number.isFinite(agentCost.inputTokens) ? agentCost.inputTokens : null,
      outputTokens: Number.isFinite(agentCost.outputTokens) ? agentCost.outputTokens : null,
      wallClockSeconds: Number.isFinite(row.wallClockSeconds) ? row.wallClockSeconds : null,
      timestamp: row.ts || isoTimestamp(),
    };
  } else {
    if (!args["harness-version"] || !args["review-mode"] || args["agent-count"] === undefined) {
      process.stderr.write("--mode in-context requires --harness-version, --review-mode, --agent-count\n");
      process.exit(2);
    }
    const planFlags = ["lenses-planned", "lenses-dispatched", "critic-planned", "critic-ran"];
    const missingPlan = planFlags.filter((k) => args[k] === undefined);
    if (missingPlan.length) {
      process.stderr.write(`--mode in-context requires ${missingPlan.map((k) => `--${k}`).join(", ")}\n`);
      process.exit(2);
    }
    const shortfall = planShortfall({
      lensesPlanned: Number(args["lenses-planned"]),
      lensesDispatched: Number(args["lenses-dispatched"]),
      criticPlanned: String(args["critic-planned"]) === "true",
      criticRan: String(args["critic-ran"]) === "true",
    });
    if (shortfall && !(typeof args["plan-deviation"] === "string" && args["plan-deviation"].trim())) {
      process.stderr.write(
        `refusing the footer: ${shortfall}. Run what the plan says, or pass --plan-deviation "<reason>" and record the same reason as lensPlanDeviation in the row.\n`,
      );
      process.exit(1);
    }
    let codexModelUsed = null;
    if (args["codex-model-json"] !== undefined) {
      try {
        codexModelUsed = JSON.parse(args["codex-model-json"]);
      } catch {
        process.stderr.write("--codex-model-json did not parse as JSON\n");
        process.exit(2);
      }
    }
    fields = {
      engine: engineLabel({}, ownPluginVersion()),
      harnessVersion: args["harness-version"],
      reviewMode: args["review-mode"],
      codexModelUsed,
      agentCount: Number(args["agent-count"]),
      inputTokens: null,
      outputTokens: null,
      wallClockSeconds: null,
      timestamp: args.timestamp || isoTimestamp(),
    };
  }

  let line;
  try {
    line = buildFooterLine(fields);
  } catch (e) {
    process.stderr.write(`${e.message}\n`);
    process.exit(1);
  }
  process.stdout.write(line + "\n");
}

if (isMainModule()) {
  main();
}
