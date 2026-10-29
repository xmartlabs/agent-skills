#!/usr/bin/env node
// replay-ledger-race.mjs — two runs finishing together must each find their own row.
//
// Every run appends one row to metrics.jsonl and then reads the ledger back to
// check it (the workflow's row check) and to patch agentCost onto it
// (persist-review-record.mjs). Both used to take "the last row" as theirs, so a
// run that appended in between made the other report METRICS ROW NOT VERIFIED,
// refuse its agentCost patch (the transcripts that patch reads are deleted over
// time), or, when the patch rewrote the whole file, erase the other run's row.
//
// This runs the REAL row-check python (sliced out of workflow-src), the real
// locateLastRow, and the real persist-review-record.mjs and append-metrics-row.mjs
// as subprocesses against a throwaway TRIBUNAL_DATA_DIR. Each case is also run
// with the fault put back, to show the case can go red.
//
// Usage: node eval/replay-ledger-race.mjs

import { execFileSync, spawn } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SKILL_DIR, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const workflow = readFileSync(WORKFLOW, "utf8");
const from = workflow.indexOf("const ROW_CHECK_BASH = (e) =>");
const to = workflow.indexOf("const rowCheckRun = ");
if (from === -1 || to === -1 || to < from) {
  console.error("could not slice ROW_CHECK_BASH out of workflow-src/tribunal.js");
  process.exit(1);
}
const shellQuote = (text) => `'${String(text).replace(/'/g, "'\\''")}'`;
const makeCheckBash = (src) => new Function("shellQuote", `${src}\nreturn ROW_CHECK_BASH;`)(shellQuote);
const rowCheckBash = makeCheckBash(workflow.slice(from, to));
const OLD_LOOKUP = "reversed(rows[-50:])";
check("the row check walks back over the newest rows", workflow.slice(from, to).includes(OLD_LOOKUP));
const lastRowOnly = makeCheckBash(workflow.slice(from, to).replace(OLD_LOOKUP, "reversed(rows[-1:])"));

const T0 = 1792470720;
const iso = (epoch) => new Date(epoch * 1000).toISOString().replace(/\.\d+Z$/, "Z");
const row = (pr, sha, ts, extra = {}) => ({ schemaVersion: 1, mode: "workflow", pr, repo: "o/be", headSha: sha, ts: iso(ts), findings: [{ title: "x" }], verdict: null, posted: false, ...extra });
const scratch = () => mkdtempSync(join(tmpdir(), "ledger-race-"));
const ledger = (dir, rows) => writeFileSync(join(dir, "metrics.jsonl"), rows.map((r) => JSON.stringify(r)).join("\n") + "\n");
const rowCheck = (bashFn, dir, expect) =>
  JSON.parse(execFileSync("bash", ["-c", bashFn(expect)], { env: { ...process.env, TRIBUNAL_DATA_DIR: dir }, encoding: "utf8" }).trim().split("\n").pop());
const mine = { pr: 2201, repo: "o/be", headSha: "aaa", findings: 1, startEpoch: T0 };

console.log("\n--- the workflow's row check ---");
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 + 300), row(2202, "bbb", T0 + 301)]);
  const r = rowCheck(rowCheckBash, dir, mine);
  check("another run's row after mine: mine is found", r.found === true && r.pr === 2201, JSON.stringify(r));
  const old = rowCheck(lastRowOnly, dir, mine);
  check("fault put back (last row only): the same ledger reports mine missing", old.found === false, JSON.stringify(old));
}
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 - 3600), row(2202, "bbb", T0 + 301)]);
  const r = rowCheck(rowCheckBash, dir, mine);
  check("an older run's row at the same commit is not mine", r.found === false, JSON.stringify(r));
}
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 - 3600), row(2201, "aaa", T0 + 300)]);
  const r = rowCheck(rowCheckBash, dir, mine);
  check("a rerun of the same commit: the new row is the one found", r.found === true, JSON.stringify(r));
}
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 + 300), row(2202, "bbb", T0 + 301)]);
  const r = rowCheck(rowCheckBash, dir, { ...mine, startEpoch: null });
  check("no start epoch: only the last row counts, as before", r.found === false, JSON.stringify(r));
  const own = rowCheck(rowCheckBash, dir, { ...mine, pr: 2202, repo: "o/be", headSha: "bbb", startEpoch: null });
  check("no start epoch: the last row is found when it is mine", own.found === true, JSON.stringify(own));
}
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 + 300, { repo: "o/fe" })]);
  const r = rowCheck(rowCheckBash, dir, mine);
  check("the same PR number in another repo is not mine", r.found === false, JSON.stringify(r));
}

console.log("\n--- locateLastRow ---");
const LOCATE = join(SKILL_DIR, "scripts", "locate-last-row.mjs");
const locate = (dir, ...args) => {
  try {
    return { ok: true, out: execFileSync("node", [LOCATE, ...args], { env: { ...process.env, TRIBUNAL_DATA_DIR: dir }, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }) };
  } catch (e) {
    return { ok: false, err: String(e.stderr) };
  }
};
{
  const dir = scratch();
  ledger(dir, [row(2201, "aaa", T0 + 300), row(2202, "bbb", T0 + 301)]);
  const r = locate(dir, "--pr", "2201", "--head-sha", "aaa");
  check("another run's row is last: mine is still located", r.ok && JSON.parse(r.out).pr === 2201, r.err);
  const none = locate(dir, "--pr", "2203", "--head-sha", "ccc");
  check("no recent row is mine: refuses with the newest row's reason", !none.ok && /refusing: last row is pr 2202, expected 2203/.test(none.err), none.err);
}

console.log("\n--- concurrent writers on one ledger ---");
const copyScripts = (lockless) => {
  const plugin = scratch();
  const root = join(plugin, "skills", "review-pr");
  cpSync(join(SKILL_DIR, "scripts"), join(root, "scripts"), { recursive: true });
  cpSync(join(SKILL_DIR, "eval"), join(root, "eval"), { recursive: true });
  cpSync(join(SKILL_DIR, "..", "..", "lib"), join(plugin, "lib"), { recursive: true });
  if (lockless) {
    const f = join(root, "scripts", "ledger-lock.mjs");
    writeFileSync(f, "export function withLedgerLock(path, fn) { return fn(); }\n");
  }
  return root;
};
const run = (cmd, args, env) =>
  new Promise((resolve) => {
    const c = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: "ignore" });
    c.on("close", resolve);
  });
async function race(root) {
  const dir = scratch();
  const PATCHERS = 12;
  ledger(dir, Array.from({ length: PATCHERS }, (_, i) => row(3000 + i, `sha${i}`, T0 + i)));
  const env = { TRIBUNAL_DATA_DIR: dir, CLAUDE_PLUGIN_ROOT: "" };
  const jobs = [];
  for (let i = 0; i < PATCHERS; i++) {
    jobs.push(run("node", [join(root, "scripts", "persist-review-record.mjs"), "--pr", String(3000 + i), "--head-sha", `sha${i}`, "--posted", "true", "--verdict", `v${i}`], env));
    const rowFile = join(dir, `new-${i}.json`);
    writeFileSync(rowFile, JSON.stringify(row(4000 + i, `new${i}`, T0 + 500 + i)));
    jobs.push(run("node", [join(root, "scripts", "append-metrics-row.mjs"), "--row-file", rowFile], env));
  }
  await Promise.all(jobs);
  const rows = readFileSync(join(dir, "metrics.jsonl"), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
  const appended = rows.filter((r) => r.pr >= 4000).length;
  const patched = rows.filter((r) => r.pr < 4000 && r.verdict === `v${r.pr - 3000}`).length;
  return { total: rows.length, appended, patched, PATCHERS };
}
{
  const locked = await race(copyScripts(false));
  check(
    "with the lock, every append lands and every patch applies",
    locked.appended === locked.PATCHERS && locked.patched === locked.PATCHERS && locked.total === 2 * locked.PATCHERS,
    JSON.stringify(locked),
  );
  let lost = null;
  for (let attempt = 0; attempt < 40 && !lost; attempt++) {
    const bare = await race(copyScripts(true));
    if (bare.appended < bare.PATCHERS || bare.patched < bare.PATCHERS) lost = bare;
  }
  check("fault put back (lock removed): the same race loses rows or patches", lost !== null, lost ? JSON.stringify(lost) : "40 attempts, nothing lost; this machine did not hit the window");
}

console.log("\n--- persist-run.sh takes the same lock ---");
{
  const sh = readFileSync(join(SKILL_DIR, "..", "..", "lib", "persist-run.sh"), "utf8");
  check("the append is under the ledger lock", sh.includes("os.mkdir(_lock)") && sh.includes("path + '.lock'") && /_unlock\(\)\nprint\('ok'\)/.test(sh));
  check("and the lock path is the one ledger-lock.mjs makes", /`\$\{path\}\.lock`/.test(readFileSync(join(SKILL_DIR, "scripts", "ledger-lock.mjs"), "utf8")));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s)`);
  process.exit(1);
}
console.log("\nreplay-ledger-race: all checks passed");
