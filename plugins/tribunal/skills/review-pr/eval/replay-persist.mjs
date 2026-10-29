#!/usr/bin/env node
// replay-persist.mjs — regression test for the metrics-row truncation guard.
//
// On the 2026-07-23 acceptance run the `persist-metrics` agent was handed a
// 30-finding record (29 KB of JSON in a heredoc) and wrote a row carrying 17 of
// them. Nothing noticed: the row parsed, every top-level field was present, and
// every per-finding field was present on the 17 that survived. The 13 it
// dropped happened to include the single finding the whole run existed to
// produce, so `eval/score.mjs` read the row and reported the bug as MISSED —
// the harness had found it and the measurement said otherwise.
//
// The fix is a count assert that rides inside the same bash the agent runs.
// Truncation eats the tail, and `findings` is the tail, so a short copy trips
// the assert and writes NOTHING rather than writing a plausible partial row.
//
// This replays that exact payload — full, and truncated the way the agent
// truncated it — through the guard, and asserts write / refuse. The guard is
// mirrored here because the workflow script cannot be imported (Workflow-runtime
// module, top-level await, injected globals); eval/check-mirror.mjs pins the
// two copies to the same text.
//
// Usage: node eval/replay-persist.mjs [--verbose]

import { readFileSync, mkdtempSync, existsSync, readFileSync as read } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { FIXTURES_DIR } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const verbose = process.argv.includes("--verbose");
const full = JSON.parse(
  readFileSync(join(FIXTURES_DIR, "persist", "pr-1911-metrics.json"), "utf8"),
);

// --- mirrored from peer-review.js (persist-metrics) ---
const guard = (path, expected) => `
import sys, json, datetime, os
rec = json.load(sys.stdin)
path = ${JSON.stringify(path)}
got = len(rec.get('findings') or [])
if got != ${expected}:
    sys.exit('REFUSED: record carries %d findings, expected ${expected} — the JSON was truncated in transit, nothing written' % got)
rec['ts'] = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
with open(path, 'a') as fh:
    fh.write(json.dumps(rec) + '\\n')
print('ok')
`;
// --- end mirror ---

function attempt(record, expected, path) {
  try {
    const out = execFileSync("python3", ["-c", guard(path, expected)], {
      input: JSON.stringify(record),
      encoding: "utf8",
      stdio: ["pipe", "pipe", "pipe"],
    });
    return { wrote: true, out: out.trim(), err: "" };
  } catch (e) {
    return { wrote: false, out: "", err: String(e.stderr || e.message).trim() };
  }
}

const dir = mkdtempSync(join(tmpdir(), "peer-review-persist-"));
const expected = full.findings.length;
const failures = [];

// 1 · the complete record is written.
const okPath = join(dir, "ok.jsonl");
const good = attempt(full, expected, okPath);
if (!good.wrote) failures.push(`complete record was refused: ${good.err}`);
else {
  const rows = read(okPath, "utf8").trim().split("\n");
  if (rows.length !== 1) failures.push(`expected 1 row, got ${rows.length}`);
  const parsed = JSON.parse(rows[0]);
  if (parsed.findings.length !== expected)
    failures.push(`written row carries ${parsed.findings.length} of ${expected} findings`);
  if (!parsed.ts) failures.push("written row has no ts");
}

// 2 · the record truncated exactly as the agent truncated it is REFUSED, and
//     nothing is written — a partial row must not exist at all.
const truncPath = join(dir, "trunc.jsonl");
const truncated = { ...full, findings: full.findings.slice(0, 17) };
const bad = attempt(truncated, expected, truncPath);
if (bad.wrote) failures.push("a 17-of-30 record was accepted — this is the PR #1911 measurement bug");
if (!/REFUSED/.test(bad.err)) failures.push(`refusal did not name itself: ${bad.err.slice(0, 120)}`);
if (existsSync(truncPath))
  failures.push("the refused record still created a file — the write is not all-or-nothing");

// 3 · the guard must not fire on a legitimately empty finding set (a clean PR).
const emptyPath = join(dir, "empty.jsonl");
const empty = attempt({ ...full, findings: [] }, 0, emptyPath);
if (!empty.wrote) failures.push(`a clean run with 0 findings was refused: ${empty.err}`);

// 4 · a record with MORE findings than expected is refused too — the count is
//     an equality check, not a floor, so a duplicated block cannot slip past.
const dupePath = join(dir, "dupe.jsonl");
const dupe = attempt({ ...full, findings: [...full.findings, full.findings[0]] }, expected, dupePath);
if (dupe.wrote) failures.push("a record with an extra finding was accepted");

console.log("metrics persistence guard replay — PR #1911 payload\n");
console.log(`  fixture findings        ${expected}`);
console.log(`  complete record         ${good.wrote ? "written" : "REFUSED"}`);
console.log(`  truncated to 17         ${bad.wrote ? "written" : "refused"}`);
console.log(`  empty finding set       ${empty.wrote ? "written" : "REFUSED"}`);
console.log(`  extra finding           ${dupe.wrote ? "written" : "refused"}`);
if (verbose) console.log(`\n  refusal: ${bad.err.slice(0, 200)}`);

// --- the relay contract: a dropped field must fail validation, not become null
// The row is written by the persist python; the RETURNED metrics object is patched
// from what the reconcile-audit-persist agent relays back. On PR #1923 the row
// carried wallClockSeconds 619 / wallClockSource "inflight-exact" and the return
// carried null for all three, because an omitted key becomes `undefined` and the
// JS reads that as null. Requiring the keys turns a silent omission into a schema
// retry. Nullable, so an honest relay pays nothing.
{
  const WF = new URL("../../../workflow-src/tribunal.js", import.meta.url);
  const src = readFileSync(WF, "utf8");
  const m = src.match(/required: \[\s*"selfAudit",\s*"persisted",([\s\S]*?)\]/);
  const req = m ? [...m[0].matchAll(/"([A-Za-z]+)"/g)].map((x) => x[1]) : [];
  for (const key of ["wallClockSeconds", "wallClockNote", "wallClockSource"]) {
    const ok = req.includes(key);
    if (!ok) failures.push(`bookkeeping schema does not REQUIRE ${key} — a relay that omits it silently becomes null and contradicts the row`);
    console.log(`  ${ok ? "ok   " : "FAIL "} bookkeeping schema requires ${key}`);
  }
}

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nthe guard writes complete records and refuses partial ones");
