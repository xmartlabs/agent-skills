#!/usr/bin/env node
// replay-metrics-append.mjs — Tier 0 guard for scripts/append-metrics-row.mjs,
// the append the in-context path never had.
//
// The defect this covers is not a decision rule, it is an absence. Workflow
// mode appends through lib/persist-run.sh (`json.dumps(rec) + '\n'`, one line
// by construction); in-context mode had no script, because
// persist-review-record.mjs is a row PATCHER that says of itself "never an
// append". SKILL.md therefore told the orchestrator, a model, to hand-write
// JSON into a JSONL ledger. On 2026-09-09 that produced 102 lines of truncated
// pretty-printed JSON above a correct one-line record on BE PR #2084.
//
// The fixture is the REAL wreckage, not a reconstruction of it. It is the one
// input that is known to have broken the ledger, and a hand-built imitation
// would only prove the script rejects what I imagined the bug looked like.
//
// The last case is the one that matters most and is easiest to leave out: the
// file was corrupt for hours while the Workflow's own persistVerified reported
// success, because persistVerified reads back the row it wrote and that row was
// intact. So this asserts the ledger is read as a whole, in both directions:
// refuse to append onto damage, and refuse (rolling back) if the append itself
// leaves the file unparseable.
//
// Not covered, and the script's header says the same: the post-append scan
// cannot be turned red from here. The pre-scan leaves the file clean and the
// serialiser makes the new line valid, so nothing a single process can send
// reaches that scan with damage. Removing it leaves this file green. Its real
// failure mode is a concurrent writer, which needs a different rig.
//
// Every case runs the script as a subprocess with TRIBUNAL_DATA_DIR pointed at
// a throwaway directory. Nothing here touches the real metrics.jsonl.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FIXTURES_DIR, SKILL_DIR } from "./paths.mjs";

const SCRIPT = join(SKILL_DIR, "scripts", "append-metrics-row.mjs");
const FIXTURE = join(FIXTURES_DIR, "metrics-append", "truncated-2084.json");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`  using: ${SCRIPT}`);

const ROW = {
  schemaVersion: 3,
  pr: 9999,
  repo: "acme/api",
  headSha: "a".repeat(40),
  mode: "in-context",
  findings: [],
};

function bed(initial) {
  const dir = mkdtempSync(join(tmpdir(), "metrics-append-"));
  if (initial !== undefined) writeFileSync(join(dir, "metrics.jsonl"), initial);
  return dir;
}

// Returns {status, stdout, stderr} instead of throwing, so a refusal is data.
function run(dir, args, stdin) {
  try {
    const stdout = execFileSync("node", [SCRIPT, ...args], {
      env: { ...process.env, TRIBUNAL_DATA_DIR: dir },
      input: stdin,
      encoding: "utf8",
      stdio: ["pipe", "pipe", "pipe"],
    });
    return { status: 0, stdout, stderr: "" };
  } catch (e) {
    return { status: e.status ?? 1, stdout: String(e.stdout || ""), stderr: String(e.stderr || "") };
  }
}

const ledger = (dir) => {
  const p = join(dir, "metrics.jsonl");
  return existsSync(p) ? readFileSync(p, "utf8") : "";
};
const recordsIn = (text) => text.split("\n").filter((l) => l.trim()).length;
const allParse = (text) =>
  text
    .split("\n")
    .filter((l) => l.trim())
    .every((l) => {
      try {
        const o = JSON.parse(l);
        return o && typeof o === "object" && !Array.isArray(o);
      } catch {
        return false;
      }
    });

console.log("\nthe shape that broke the ledger");
{
  // This is the whole point of the script: the caller may compose pretty JSON,
  // and one line is what reaches the file regardless.
  const dir = bed("");
  const pretty = join(dir, "pretty.json");
  writeFileSync(pretty, JSON.stringify(ROW, null, 2));
  check(
    "the fixture really is multi-line, so this case is not vacuous",
    readFileSync(pretty, "utf8").split("\n").length > 1,
  );
  const r = run(dir, ["--row-file", pretty]);
  check("a pretty-printed record is accepted", r.status === 0, r.stderr.trim());
  check("and lands as exactly one line", recordsIn(ledger(dir)) === 1 && ledger(dir).trim().split("\n").length === 1);
  check("which still parses", allParse(ledger(dir)));
}

console.log("\nthe real 2026-09-09 wreckage from BE PR #2084");
{
  const dir = bed(JSON.stringify({ ...ROW, pr: 1 }) + "\n");
  const raw = readFileSync(FIXTURE, "utf8");
  // Prove the fixture is the damaged thing before asserting anything about the
  // reaction to it. A fixture that quietly parses would make the refusal below
  // meaningless.
  let fixtureParses = true;
  try {
    JSON.parse(raw);
  } catch {
    fixtureParses = false;
  }
  check("the fixture is genuinely unparseable", !fixtureParses);
  check("and genuinely multi-line", raw.split("\n").length > 50, `${raw.split("\n").length} lines`);

  const r = run(dir, ["--row-file", FIXTURE]);
  check("appending it is refused", r.status !== 0);
  check("the refusal names JSON parsing as the reason", /did not parse as JSON/.test(r.stderr), r.stderr.trim().slice(0, 120));
  check("and the ledger is untouched", recordsIn(ledger(dir)) === 1 && allParse(ledger(dir)));
}

console.log("\na ledger that is already corrupt");
{
  // Appending a valid row onto damage produces a file that is still broken and
  // now also looks recently maintained. This is the guard persistVerified did
  // not have: it read back its own row, found it intact, and said success.
  const dir = bed(JSON.stringify({ ...ROW, pr: 1 }) + "\n{\n  \"pr\": 2,\n");
  const before = ledger(dir);
  const r = run(dir, ["-"], JSON.stringify(ROW));
  check("the append is refused", r.status !== 0);
  check("the refusal points at the offending line number", /line 2/.test(r.stderr), r.stderr.trim().slice(0, 160));
  check("nothing is written", ledger(dir) === before);
  // Pin WHICH guard fired. Without this the post-append scan alone satisfies
  // every assertion above: it writes, finds the file still broken, rolls back,
  // and refuses. Same exit code, same untouched file, different behaviour --
  // the rollback path modifies the ledger and undoes it, so a crash in that
  // window leaves damage the pre-scan would never have created. Found by
  // injection: disabling the pre-scan left this whole section green.
  check(
    "and it is the PRE-append scan that refuses, not a rollback after writing",
    /is already corrupt and appending would bury it/.test(r.stderr),
    r.stderr.trim().slice(0, 160),
  );
}

console.log("\nthe duplicate write");
{
  const dir = bed("");
  const first = run(dir, ["-"], JSON.stringify(ROW));
  check("the first append succeeds", first.status === 0, first.stderr.trim());
  const second = run(dir, ["-"], JSON.stringify(ROW));
  check("an identical second append is refused", second.status !== 0);
  check("and the ledger still holds one record", recordsIn(ledger(dir)) === 1);
  const forced = run(dir, ["-", "--allow-duplicate"], JSON.stringify(ROW));
  check("--allow-duplicate is the way to mean it", forced.status === 0 && recordsIn(ledger(dir)) === 2, forced.stderr.trim());
}

console.log("\na half-composed row");
{
  // The failure on this path is a truncated or half-written record, so the
  // required-key check exists to catch that, not to relitigate the schema.
  for (const missing of ["pr", "headSha", "findings", "schemaVersion"]) {
    const dir = bed("");
    const row = { ...ROW };
    delete row[missing];
    const r = run(dir, ["-"], JSON.stringify(row));
    check(`a row with no "${missing}" is refused`, r.status !== 0 && recordsIn(ledger(dir)) === 0);
  }
  const dir = bed("");
  const r = run(dir, ["-"], JSON.stringify({ ...ROW, findings: "none" }));
  check('"findings" that is not an array is refused', r.status !== 0 && recordsIn(ledger(dir)) === 0);
}

console.log("\nthe ordinary path still works");
{
  const dir = bed("");
  check("append to a file that does not exist yet", run(dir, ["-"], JSON.stringify(ROW)).status === 0);
  const r = run(dir, ["-"], JSON.stringify({ ...ROW, pr: 1234 }));
  check("append to an existing ledger", r.status === 0, r.stderr.trim());
  check("both records are there and parse", recordsIn(ledger(dir)) === 2 && allParse(ledger(dir)));
  check("the result line reports the count", /"records":2/.test(r.stdout), r.stdout.trim());

  // A file whose last line lost its newline must not swallow the next record.
  const dir2 = bed(JSON.stringify({ ...ROW, pr: 7 }));
  check("a ledger with no trailing newline is joined correctly", run(dir2, ["-"], JSON.stringify(ROW)).status === 0);
  check("and yields two parseable records", recordsIn(ledger(dir2)) === 2 && allParse(ledger(dir2)));
}

console.log(
  failures.length === 0
    ? "\nthe ledger cannot be corrupted through this script"
    : `\n${failures.length} check(s) FAILED: ${failures.join(", ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
