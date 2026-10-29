#!/usr/bin/env node
// replay-resolve-baseline.mjs — Tier 0 guard for scripts/resolve-baseline.mjs.
//
// Executes the REAL resolveBaseline (imported, not re-implemented) against
// synthetic and real-shaped rows/reviews. This predicate used to be Python
// embedded inside an agent prompt string in workflows/tribunal.js (the
// PYPRIOR heredoc) — applied by hand every run, reachable only by slicing it
// out with eval/replay-embedded-bash.mjs. Moving it into an imported function
// is what makes every branch below cheap to assert without ever launching an
// agent.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
  resolveBaseline,
  readMetricsRows,
  isBotAuthor,
  BOT_LOGIN_SUFFIX_RE,
  BOT_LOGINS,
} from "../scripts/resolve-baseline.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = resolve(HERE, "..", "scripts", "resolve-baseline.mjs");

let failures = 0;
const ok = (name) => console.log(`  ok    ${name}`);
const fail = (name, detail) => {
  failures++;
  console.log(`  FAIL  ${name}${detail ? `\n        ${detail}` : ""}`);
};

const eqArr = (a, b) =>
  Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => x === b[i]);

function check(name, got, expect) {
  const bad = [];
  for (const [k, v] of Object.entries(expect)) {
    const g = got[k];
    const equal = Array.isArray(v) ? eqArr(v, g) : g === v;
    if (!equal) bad.push(`${k}: expected ${JSON.stringify(v)}, got ${JSON.stringify(g)}`);
  }
  if (bad.length) fail(name, bad.join("\n        "));
  else ok(name);
}

const HEAD = "d047fa9fffffffffffffffffffffffffffffffff";

console.log("\n--- isBotAuthor() ---");
if (isBotAuthor("renovate[bot]") && isBotAuthor("dependabot[bot]") && isBotAuthor("coderabbitai")) {
  ok("bot suffix and the named bot login are both caught");
} else {
  fail("bot suffix and the named bot login are both caught");
}
if (!isBotAuthor("alice") && !isBotAuthor("bob") && !isBotAuthor("")) {
  ok("a human login, and an empty login, are not bots");
} else {
  fail("a human login, and an empty login, are not bots");
}
if (BOT_LOGIN_SUFFIX_RE.test("x[bot]") && BOT_LOGINS.has("coderabbitai")) {
  ok("the rule is a named, greppable constant");
} else {
  fail("the rule is a named, greppable constant");
}

console.log("\n--- source 1 (tribunal-row): behaviour-identical to the PYPRIOR predicate it replaces ---");

// PR #1968's real shape (see eval/replay-embedded-bash.mjs): 8 rows, 6 posted,
// two 2-lens ablations. The posted 0eedf726 row is the one that must win.
const rows1968 = [
  { pr: 1968, schemaVersion: 3, headSha: "aaaaaaaa", posted: true, findings: [{ file: "a.ts", title: "old", severity: "important" }] },
  { pr: 1968, schemaVersion: 3, headSha: "bbbbbbbb", posted: false, findings: [{ file: "b.ts", title: "ablation-only", severity: "critical" }] },
  { pr: 1968, schemaVersion: 3, headSha: "0eedf726", posted: true, ts: "2026-07-24T12:00:00Z", findings: [{ file: "c.ts", title: "real", severity: "critical" }] },
];
{
  const r = resolveBaseline({ rows: rows1968, reviews: [], headSha: HEAD, since: "" });
  check("the posted 0eedf726 row wins over an unposted later ablation", r, {
    found: true,
    sha: "0eedf726",
    source: "tribunal-row",
  });
}

console.log("\n--- one PR number in two repos: a run reads only its own repo's rows ---");
{
  // A row with no repo predates the field, so in a real log it is the oldest.
  const rows = [
    { pr: 7, schemaVersion: 3, headSha: "old00001", posted: true, findings: [{ file: "x.ts", title: "pre-repo row", severity: "nit" }] },
    { pr: 7, repo: "acme/api", schemaVersion: 3, headSha: "api00001", posted: true, findings: [{ file: "api.ts", title: "api finding", severity: "critical" }] },
    { pr: 7, repo: "acme/web", schemaVersion: 3, headSha: "web00001", posted: true, findings: [{ file: "web.tsx", title: "web finding", severity: "critical" }] },
  ];
  const apiOnly = resolveBaseline({ rows: readMetricsRows(7, "acme/api", rows), reviews: [], headSha: HEAD, since: "" });
  check("the api run's baseline is the api row, not the later web row", apiOnly, { found: true, sha: "api00001", source: "tribunal-row" });
  const api = readMetricsRows(7, "acme/api", rows);
  if (api.length === 2 && api.every((r) => r.repo !== "acme/web")) ok("the api run never sees the web repo's row; a row with no repo still matches");
  else fail("the api run never sees the web repo's row; a row with no repo still matches", JSON.stringify(api.map((r) => r.headSha)));
  const r = resolveBaseline({ rows: readMetricsRows(7, "acme/web", rows), reviews: [], headSha: HEAD, since: "" });
  check("the web run's baseline is the web row", r, { found: true, sha: "web00001", source: "tribunal-row" });
  if (readMetricsRows(7, null, rows).length === 3) ok("a caller that names no repo keeps the old PR-only match");
  else fail("a caller that names no repo keeps the old PR-only match");
}

console.log("\n--- posted: false rows are skipped, and --since reaches them by sha prefix ---");
{
  const r = resolveBaseline({ rows: rows1968, reviews: [], headSha: HEAD, since: "" });
  if (r.sha !== "bbbbbbbb") ok("an unposted row is skipped without --since");
  else fail("an unposted row is skipped without --since", JSON.stringify(r));
}
{
  const r = resolveBaseline({ rows: rows1968, reviews: [], headSha: HEAD, since: "bbbb" });
  check("--since <prefix> reaches an unposted row by sha prefix", r, { found: true, sha: "bbbbbbbb", source: "tribunal-row" });
}

console.log("\n--- rows with empty findings are skipped ---");
{
  const rows = [
    { pr: 1, schemaVersion: 3, headSha: "cccccccc", posted: true, findings: [] },
    { pr: 1, schemaVersion: 3, headSha: "dddddddd", posted: true, findings: [{ file: "x.ts", title: "t" }] },
  ];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  check("an empty-findings row never wins even when it's the last one on file", r, { found: true, sha: "dddddddd" });
}
{
  const rows = [{ pr: 1, schemaVersion: 3, headSha: "eeeeeeee", posted: true, findings: [] }];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  if (r.found === false) ok("empty findings on the only candidate row -> not found");
  else fail("empty findings on the only candidate row -> not found", JSON.stringify(r));
}

console.log("\n--- rows whose headSha equals the current head are skipped ---");
{
  const rows = [{ pr: 1, schemaVersion: 3, headSha: HEAD, posted: true, findings: [{ file: "x.ts", title: "t" }] }];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  if (r.found === false) ok("a row at the current head is not a baseline for itself");
  else fail("a row at the current head is not a baseline for itself", JSON.stringify(r));
}

console.log("\n--- schemaVersion < 3 rows are skipped ---");
{
  const rows = [
    { pr: 1, schemaVersion: 2, headSha: "ffffffff", posted: true, findings: [{ file: "x.ts", title: "t" }] },
    { pr: 1, schemaVersion: 3, headSha: "11111111", posted: true, findings: [{ file: "y.ts", title: "t2" }] },
  ];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  check("schemaVersion 2 is invisible to the predicate; schemaVersion 3 wins", r, { found: true, sha: "11111111" });
}

console.log("\n--- the last matching row in file order wins when several match ---");
{
  const rows = [
    { pr: 1, schemaVersion: 3, headSha: "22222222", posted: true, findings: [{ file: "a.ts", title: "first" }] },
    { pr: 1, schemaVersion: 3, headSha: "33333333", posted: true, findings: [{ file: "b.ts", title: "second" }] },
    { pr: 1, schemaVersion: 3, headSha: "44444444", posted: true, findings: [{ file: "c.ts", title: "third, last in file order" }] },
  ];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  check("the LAST matching row wins, not the first or the one with the most findings", r, { found: true, sha: "44444444" });
}

console.log("\n--- the findings projection carries operatorOverride verbatim ---");
{
  const rows = [
    {
      pr: 1,
      schemaVersion: 3,
      headSha: "55555555",
      posted: true,
      findings: [
        { file: "a.ts", title: "t", severity: "critical", blocking: true, survived: true, posted: true, operatorOverride: { direction: "reinstated" } },
      ],
    },
  ];
  const r = resolveBaseline({ rows, reviews: [], headSha: HEAD, since: "" });
  if (r.findings.length === 1 && r.findings[0].operatorOverride?.direction === "reinstated") {
    ok("operatorOverride survives the projection verbatim");
  } else {
    fail("operatorOverride survives the projection verbatim", JSON.stringify(r.findings));
  }
}

console.log("\n--- source 2 (human-review): only tried when source 1 finds nothing ---");

const approvedByAlice = { author: "alice", state: "APPROVED", at: "2026-09-09T13:44:09Z", commit: "b548bfc3" };
const commentedByCoderabbit1 = { author: "coderabbitai", state: "COMMENTED", at: "2026-09-01T19:22:51Z", commit: "85bb25e8" };
const commentedByCoderabbit2 = { author: "coderabbitai", state: "COMMENTED", at: "2026-09-09T17:52:43Z", commit: "6f1a852f" };

{
  const r = resolveBaseline({ rows: [], reviews: [approvedByAlice, commentedByCoderabbit1, commentedByCoderabbit2], headSha: HEAD, since: "" });
  check("no tribunal row, one qualifying human review -> human-review baseline", r, {
    found: true,
    sha: "b548bfc3",
    source: "human-review",
    reviewer: "alice",
    reviewState: "APPROVED",
  });
  if (eqArr(r.findings, [])) ok("a human-review baseline carries no findings");
  else fail("a human-review baseline carries no findings", JSON.stringify(r.findings));
}

console.log("\n--- precedence: a matching tribunal row wins over an available human review ---");
{
  const r = resolveBaseline({
    rows: rows1968.filter((row) => row.pr === 1968),
    reviews: [approvedByAlice],
    headSha: HEAD,
    since: "",
  });
  check("tribunal-row wins even when a qualifying human review also exists", r, { found: true, sha: "0eedf726", source: "tribunal-row" });
}

console.log("\n--- COMMENTED reviews never produce a baseline ---");
{
  const r = resolveBaseline({ rows: [], reviews: [commentedByCoderabbit1, commentedByCoderabbit2], headSha: HEAD, since: "" });
  if (r.found === false) ok("COMMENTED-only reviews -> not found, even from a non-bot");
  else fail("COMMENTED-only reviews -> not found, even from a non-bot", JSON.stringify(r));
}
{
  const humanCommented = { author: "alice", state: "COMMENTED", at: "2026-09-09T13:33:02Z", commit: "b548bfc3" };
  const r = resolveBaseline({ rows: [], reviews: [humanCommented], headSha: HEAD, since: "" });
  if (r.found === false) ok("a COMMENTED review from a human is still not evidence of review");
  else fail("a COMMENTED review from a human is still not evidence of review", JSON.stringify(r));
}

console.log("\n--- bot reviews never produce a baseline ---");
{
  const botApproved = { author: "coderabbitai", state: "APPROVED", at: "2026-09-09T13:44:09Z", commit: "b548bfc3" };
  const r = resolveBaseline({ rows: [], reviews: [botApproved], headSha: HEAD, since: "" });
  if (r.found === false) ok("an APPROVED review from coderabbitai (named bot) does not count");
  else fail("an APPROVED review from coderabbitai (named bot) does not count", JSON.stringify(r));
}
{
  const botApproved = { author: "some-ci-runner[bot]", state: "CHANGES_REQUESTED", at: "2026-09-09T13:44:09Z", commit: "b548bfc3" };
  const r = resolveBaseline({ rows: [], reviews: [botApproved], headSha: HEAD, since: "" });
  if (r.found === false) ok("a CHANGES_REQUESTED review from a [bot]-suffixed login does not count");
  else fail("a CHANGES_REQUESTED review from a [bot]-suffixed login does not count", JSON.stringify(r));
}

console.log("\n--- a review whose commit equals the current head does not produce a baseline ---");
{
  const atHead = { author: "alice", state: "APPROVED", at: "2026-09-09T13:44:09Z", commit: HEAD };
  const r = resolveBaseline({ rows: [], reviews: [atHead], headSha: HEAD, since: "" });
  if (r.found === false) ok("a review at the current head is not a baseline for itself");
  else fail("a review at the current head is not a baseline for itself", JSON.stringify(r));
}

console.log("\n--- the most recent qualifying human review wins over an older one ---");
{
  const older = { author: "alice", state: "CHANGES_REQUESTED", at: "2026-08-01T00:00:00Z", commit: "aaaa1111" };
  const newer = { author: "alice", state: "APPROVED", at: "2026-09-01T00:00:00Z", commit: "bbbb2222" };
  const r1 = resolveBaseline({ rows: [], reviews: [older, newer], headSha: HEAD, since: "" });
  const r2 = resolveBaseline({ rows: [], reviews: [newer, older], headSha: HEAD, since: "" });
  check("newer review wins regardless of array order (older, newer)", r1, { sha: "bbbb2222", reviewState: "APPROVED" });
  check("newer review wins regardless of array order (newer, older)", r2, { sha: "bbbb2222", reviewState: "APPROVED" });
}

console.log("\n--- nothing at all matches: found:false, caller falls back to full review ---");
{
  const r = resolveBaseline({ rows: [], reviews: [], headSha: HEAD, since: "" });
  if (r.found === false && Array.isArray(r.findings) && r.findings.length === 0) {
    ok("no rows, no reviews -> found:false with an empty findings array");
  } else {
    fail("no rows, no reviews -> found:false with an empty findings array", JSON.stringify(r));
  }
}
{
  // rows/reviews missing entirely (not just empty arrays) must not throw.
  const r = resolveBaseline({ headSha: HEAD, since: "" });
  if (r.found === false) ok("missing rows/reviews keys do not throw and resolve to not-found");
  else fail("missing rows/reviews keys do not throw and resolve to not-found", JSON.stringify(r));
}

console.log("\n--- real-shaped PR #2025 data (see the task's own real-data check) ---");
{
  // The live metrics file has exactly one row for PR 2025: headSha 1aa17ad9,
  // posted: false, 9 findings — not usable as source 1 without --since. Real
  // reviews: an APPROVED by alice at b548bfc3, plus two COMMENTED by
  // coderabbitai.
  const pr2025Row = { pr: 2025, schemaVersion: 3, headSha: "1aa17ad9", posted: false, findings: new Array(9).fill({ file: "x.ts", title: "t" }) };
  const pr2025Reviews = [commentedByCoderabbit1, commentedByCoderabbit2, approvedByAlice];
  const currentHead = "9999999999999999999999999999999999999999";
  const r = resolveBaseline({ rows: [pr2025Row], reviews: pr2025Reviews, headSha: currentHead, since: "" });
  check("PR #2025 shape: unposted row is skipped, falls through to the human APPROVED review", r, {
    found: true,
    source: "human-review",
    sha: "b548bfc3",
  });
}

console.log("\n--- CLI behaviour ---");
{
  const input = JSON.stringify({ pr: 999999999, headSha: HEAD, since: "", reviews: [approvedByAlice] });
  const out = execFileSync(process.execPath, [SCRIPT], { input, encoding: "utf8" });
  const lines = out.trim().split("\n");
  if (lines.length !== 1) {
    fail("CLI: exactly one JSON line on stdout", `${lines.length} lines: ${out}`);
  } else {
    try {
      const parsed = JSON.parse(lines[0]);
      if (parsed.found === true && parsed.source === "human-review") {
        ok("CLI: exactly one JSON line on stdout, exit 0, human-review source reached via stdin reviews");
      } else {
        fail("CLI: one JSON line on stdout", JSON.stringify(parsed));
      }
    } catch (e) {
      fail("CLI: exactly one JSON line on stdout", `not valid JSON: ${e.message}`);
    }
  }
}
{
  let exitCode = 0;
  try {
    execFileSync(process.execPath, [SCRIPT], { input: "{ this is not json", encoding: "utf8" });
  } catch (e) {
    exitCode = e.status;
  }
  if (exitCode !== 0) ok(`CLI: garbage stdin -> non-zero exit (${exitCode})`);
  else fail("CLI: garbage stdin -> non-zero exit", "exited 0");
}
{
  let exitCode = 0;
  try {
    execFileSync(process.execPath, [SCRIPT], { input: JSON.stringify({ headSha: HEAD }), encoding: "utf8" });
  } catch (e) {
    exitCode = e.status;
  }
  if (exitCode !== 0) ok(`CLI: missing numeric "pr" -> non-zero exit (${exitCode})`);
  else fail('CLI: missing numeric "pr" -> non-zero exit', "exited 0");
}

if (failures) {
  console.log(`\n${failures} case(s) FAILED`);
  process.exit(1);
}
console.log("\nall resolve-baseline expectations hold");
