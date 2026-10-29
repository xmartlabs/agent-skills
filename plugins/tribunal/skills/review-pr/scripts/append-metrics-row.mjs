#!/usr/bin/env node
// append-metrics-row.mjs — the only sanctioned way to add a row to
// metrics.jsonl, and the guard the in-context path never had.
//
// WHY THIS EXISTS. Workflow mode appends through `lib/persist-run.sh`, which
// serialises with `json.dumps(rec) + '\n'` and therefore cannot produce
// anything but one line. In-context mode had no script at all:
// `persist-review-record.mjs` says of itself that it is a "single-line rewrite
// of the located row only, never an append", so SKILL.md told the orchestrator
// to "append the fully-formed row directly". The orchestrator is a model, and
// the instruction was to hand-write JSON into a JSONL ledger with nothing
// checking the result. By the harness's own routing rule that is the path most
// PRs take.
//
// It failed on 2026-09-09, on BE PR #2084. The row was written twice: first as
// 102 lines of pretty-printed JSON truncated before its closing brace, then
// correctly on one line. The two parse to the identical object once the brace
// is restored, so no measurement was lost — but every consumer that reads the
// ledger line by line broke on the wreckage. `eval/replay-embedded-bash.mjs`
// went red with 6 failures and stayed red across two worktrees at two different
// content hashes, which reads exactly like a code regression and is not one.
// Nobody was looking for a corrupt ledger; the time went into diffing harness
// versions.
//
// THREE GUARDS, AND WHY EACH ONE IS SEPARATE.
//
//   1. THIS script serialises. The caller may hand over pretty-printed JSON,
//      trailing newlines, whatever it composed. `JSON.stringify` on a parsed
//      object is one line by construction, so the shape that broke the file is
//      not expressible through here. This is the guard that matters most: it
//      removes the failure rather than detecting it.
//
//   2. The ledger is scanned BEFORE the append, not only after. Appending a
//      valid row to a broken file produces a file that is still broken and now
//      also looks recently maintained. Refusing instead surfaces the damage at
//      the moment someone is present to fix it.
//
//   3. The whole file is re-scanned AFTER the append. Reading back the row you
//      just wrote is not verification, and this is the lesson the incident
//      actually taught: the Workflow's `persistVerified` did exactly that,
//      found its own row intact, and reported success while 102 lines of
//      garbage sat above it. A ledger check has to read the ledger.
//
// Nothing here is best-effort. A refusal writes nothing and exits non-zero,
// because a missing row is a known gap and a corrupt ledger is a silent one.
//
// WHAT THE REPLAY DOES NOT COVER, said here rather than left to look covered.
// Guard 3 cannot be turned red by `eval/replay-metrics-append.mjs`: guard 2
// leaves the file clean and guard 1 makes the appended line valid, so within a
// single process there is no reachable input that reaches the post-scan with
// damage. Its real failure mode is concurrent writers -- a Workflow run's
// python append landing between this script's read and its write -- and no
// single-process replay can produce that. Deleting it would keep the battery
// green, which is the argument for saying so out loud instead of trusting the
// green.
//
// Usage:
//   node scripts/append-metrics-row.mjs --row-file <path>
//   … | node scripts/append-metrics-row.mjs -
//
// Prints one line of JSON on success:
//   {"appended": true, "path": ..., "records": <count after>, "pr": ..., "headSha": ...}

import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { METRICS_PATH, isMainModule } from "../eval/paths.mjs";
import { withLedgerLock } from "./ledger-lock.mjs";

export class AppendRefused extends Error {}

// The fields whose absence makes a row unusable to every consumer that reads
// this file. Deliberately short: this is not a schema validator, and
// `references/metrics-schema.md` remains the spec. It exists to catch a row
// that was truncated or half-composed, which is the failure mode on this path,
// not to relitigate field-by-field completeness at write time.
export const REQUIRED_KEYS = ["schemaVersion", "pr", "repo", "headSha", "mode", "findings"];

export function validateRow(row) {
  const problems = [];
  if (row === null || typeof row !== "object" || Array.isArray(row))
    return ["the record is not a JSON object"];
  for (const k of REQUIRED_KEYS) if (!(k in row)) problems.push(`missing "${k}"`);
  if ("findings" in row && !Array.isArray(row.findings))
    problems.push('"findings" is present but is not an array');
  if ("schemaVersion" in row && !Number.isInteger(row.schemaVersion))
    problems.push('"schemaVersion" is not an integer');
  return problems;
}

// One line, always. The caller's formatting never survives this.
export function serialiseRow(row) {
  const line = JSON.stringify(row);
  // A string, number or null round-trips through JSON.stringify without ever
  // containing a newline, so this cannot fire on a valid object — which is
  // exactly why it is worth asserting rather than assuming.
  if (line.includes("\n"))
    throw new AppendRefused("refusing: the serialised record contains a newline");
  return line;
}

// Every line of the ledger, parsed. Returns the offending line numbers rather
// than a boolean, because "the file is broken" is not actionable and "line 126
// is not an object" is.
export function scanLedger(text) {
  const bad = [];
  let records = 0;
  const lines = text.split("\n");
  lines.forEach((l, i) => {
    if (!l.trim()) return;
    records += 1;
    try {
      const o = JSON.parse(l);
      if (o === null || typeof o !== "object" || Array.isArray(o))
        bad.push({ line: i + 1, why: "not a JSON object" });
    } catch {
      bad.push({ line: i + 1, why: "unparseable", head: l.slice(0, 60) });
    }
  });
  return { records, bad };
}

function describe(bad) {
  const shown = bad.slice(0, 5).map((b) => `  line ${b.line}: ${b.why}${b.head ? ` — ${b.head}` : ""}`);
  const more = bad.length > shown.length ? `\n  … and ${bad.length - shown.length} more` : "";
  return shown.join("\n") + more;
}

export function appendRow(row, { path = METRICS_PATH, allowDuplicate = false } = {}) {
  return withLedgerLock(path, () => appendRowLocked(row, { path, allowDuplicate }));
}

function appendRowLocked(row, { path, allowDuplicate }) {
  const problems = validateRow(row);
  if (problems.length)
    throw new AppendRefused(`refusing: the record is not a usable row:\n  ${problems.join("\n  ")}`);

  const line = serialiseRow(row);

  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch (e) {
    if (e.code !== "ENOENT") throw new AppendRefused(`refusing: could not read ${path}: ${e.message}`);
    text = "";
  }

  const before = scanLedger(text);
  if (before.bad.length)
    throw new AppendRefused(
      `refusing: ${path} is already corrupt and appending would bury it.\n${describe(before.bad)}\n` +
        `  Nothing was written. Repair the file first — a backup, then remove or re-join the broken lines.`,
    );

  // The 2026-09-09 shape: the same record written twice. A re-review at the
  // same head legitimately produces another row, but never a byte-identical
  // one — `ts` alone differs. Identical means a retry that did not know the
  // first attempt landed.
  const lastLine = text.split("\n").filter(Boolean).pop();
  if (!allowDuplicate && lastLine === line)
    throw new AppendRefused(
      "refusing: this record is byte-identical to the last row already in the ledger, " +
        "which means the previous append succeeded. Pass --allow-duplicate if you really mean two.",
    );

  // Never let a missing trailing newline join two records into one line.
  if (text && !text.endsWith("\n")) appendFileSync(path, "\n");
  appendFileSync(path, line + "\n");

  const after = scanLedger(readFileSync(path, "utf8"));
  if (after.bad.length) {
    // Roll back to exactly what was there before rather than leaving the file
    // in a state this script created and refused.
    writeFileSync(path, text);
    throw new AppendRefused(
      `refusing: the ledger did not parse after the append, so the write was rolled back.\n${describe(after.bad)}`,
    );
  }
  if (after.records !== before.records + 1)
    throw new AppendRefused(
      `refusing: expected ${before.records + 1} records after the append, found ${after.records}. The file was left as written; inspect it.`,
    );

  return { appended: true, path, records: after.records, pr: row.pr, headSha: row.headSha };
}

if (isMainModule(import.meta.url)) {
  const args = process.argv.slice(2);
  const idx = args.indexOf("--row-file");
  const src = idx >= 0 ? args[idx + 1] : args.find((a) => a === "-");
  if (!src) {
    console.error("usage: append-metrics-row.mjs --row-file <path>   (or - to read stdin)");
    process.exit(2);
  }
  let row;
  try {
    row = JSON.parse(readFileSync(src === "-" ? 0 : src, "utf8"));
  } catch (e) {
    console.error(`refusing: the record did not parse as JSON — ${e.message}`);
    process.exit(2);
  }
  try {
    const result = appendRow(row, { allowDuplicate: args.includes("--allow-duplicate") });
    process.stdout.write(JSON.stringify(result) + "\n");
  } catch (e) {
    console.error(e instanceof AppendRefused ? e.message : `refusing: ${e.message}`);
    process.exit(1);
  }
}
