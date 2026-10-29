#!/usr/bin/env node
// locate-last-row.mjs — the one guard for "read the LAST row of metrics.jsonl
// and refuse (never write, never fabricate) if it doesn't belong to this run."
//
// Why this exists. The guard used to live inline in persist-review-record.mjs
// (read + parse + refuse, then patch in place). build-post-footer.mjs needs
// the exact same guard — same file, same "last row", same pr/headSha
// refusal — to read a Workflow run's already-persisted harnessVersion/
// codexModelUsed/reviewMode/wallClockSeconds/agentCost before composing the
// post. Two independent copies of "does the last row belong to this run" is
// exactly the kind of drift eval/check-mirror.mjs exists to catch between
// PROSE surfaces — the fix for two SCRIPTS agreeing is not a mirror check,
// it's one function both import.
//
// Never writes. Only persist-review-record.mjs writes; this module only
// reads and refuses.

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { METRICS_PATH } from "../eval/rows.mjs";

export class RowMismatchError extends Error {}

const SCAN_ROWS = 50;

// Read metrics.jsonl and return the newest of the last SCAN_ROWS rows that
// belongs to the run identified by pr/headSha, refusing (throw RowMismatchError)
// when none does.
// headSha and repo are optional — omitting them checks pr only. A row with no
// `repo` predates the field and is not refused for lacking one.
//
// Returns { row, lines, lineIndex } — `lines` is the full split file (blank
// lines dropped) and `lineIndex` is where `row` sits in it, so a caller that
// needs to WRITE the row back (persist-review-record.mjs) can mutate
// `lines[lineIndex]` and rejoin, without re-reading or re-splitting the file
// a second time.
export function locateLastRow({ pr, headSha, repo } = {}) {
  let text;
  try {
    text = readFileSync(METRICS_PATH, "utf8");
  } catch (e) {
    throw new RowMismatchError(`refusing: could not read ${METRICS_PATH}: ${e.message}`);
  }
  const lines = text.split("\n").filter(Boolean);
  if (lines.length === 0) {
    throw new RowMismatchError(`refusing: ${METRICS_PATH} is empty`);
  }
  // Another run can append between this run's row and this read, so the newest
  // row is not necessarily this run's. Walk back over the newest rows and take
  // the first that belongs to the run; only when none does, refuse with the
  // reason the newest row gives.
  let newest;
  for (let i = lines.length - 1; i >= Math.max(0, lines.length - SCAN_ROWS); i--) {
    let row;
    try {
      row = JSON.parse(lines[i]);
    } catch (e) {
      if (i === lines.length - 1) {
        throw new RowMismatchError(`refusing: last line of ${METRICS_PATH} does not parse: ${e.message}`);
      }
      continue;
    }
    const why = mismatch(row, { pr, headSha, repo });
    if (!why) return { row, lines, lineIndex: i };
    if (!newest) newest = why;
  }
  throw new RowMismatchError(newest);
}

function mismatch(row, { pr, headSha, repo }) {
  if (pr !== undefined && String(row.pr) !== String(pr)) {
    return `refusing: last row is pr ${row.pr}, expected ${pr}`;
  }
  if (repo && row.repo && row.repo !== repo) {
    return `refusing: last row is repo ${row.repo}, expected ${repo}`;
  }
  if (headSha && row.headSha !== headSha) {
    return `refusing: last row headSha ${row.headSha} does not match --head-sha ${headSha}`;
  }
  return null;
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

function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

// Standalone debugging use: prints the located row's JSON, or refuses non-zero.
if (isMainModule()) {
  const args = parseArgs(process.argv.slice(2));
  try {
    const { row } = locateLastRow({ pr: args.pr, headSha: args["head-sha"] });
    process.stdout.write(JSON.stringify(row) + "\n");
  } catch (e) {
    process.stderr.write(`${e.message}\n`);
    process.exit(1);
  }
}
