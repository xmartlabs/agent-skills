#!/usr/bin/env node
// replay-refresh-ledger-snapshot.mjs — the snapshot refresh accepts an
// extension of the frozen ledger and refuses everything else.
//
// The frozen ledger is what replay-gates, replay-fidelity-payload, replay-rows,
// replay-wallclock and replay-embedded-bash re-decide. A refresh that dropped,
// reordered or corrupted a row would change their answers for a reason that is
// not in the code, so each refusal below is a way that could happen. Everything
// runs in temp directories; the real snapshot is never read or written.
//
// Usage: node eval/replay-refresh-ledger-snapshot.mjs

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { applyRefresh, planRefresh } from "./refresh-ledger-snapshot.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const row = (pr, sha, extra = {}) => JSON.stringify({ schemaVersion: 3, pr, headSha: sha, mode: "workflow", findings: [], ...extra });
const jsonl = (lines) => lines.join("\n") + "\n";
const OLD = [row(1, "a"), row(2, "b"), row(3, "c")];
const INFLIGHT = [JSON.stringify({ event: "started", pr: 1, headSha: "a" })];

function setup({ metrics, inflight = INFLIGHT }) {
  const root = mkdtempSync(join(tmpdir(), "pr-ledger-refresh-"));
  const from = join(root, "data");
  const harness = join(root, "harness");
  const to = join(harness, "tribunal", "ledger");
  mkdirSync(from, { recursive: true });
  mkdirSync(to, { recursive: true });
  writeFileSync(join(to, "metrics.jsonl"), jsonl(OLD));
  writeFileSync(join(to, "inflight-runs.jsonl"), jsonl(INFLIGHT));
  if (metrics !== null) writeFileSync(join(from, "metrics.jsonl"), metrics);
  if (inflight !== null) writeFileSync(join(from, "inflight-runs.jsonl"), jsonl(inflight));
  return { root, from, harness, to };
}

console.log("\nrefresh-ledger-snapshot — only an extension of the frozen ledger comes in");

console.log("\n--- accepted ---");
{
  const t = setup({ metrics: jsonl([...OLD, row(4, "d", { ts: "2026-10-19T00:00:00Z" })]) });
  const plan = planRefresh({ fromDir: t.from, toDir: t.to });
  const m = plan.files.find((f) => f.name === "metrics.jsonl");
  check("a ledger with one new run at the end is accepted", plan.ok, plan.errors.join("; "));
  check("the plan counts exactly that run", m?.added === 1 && m.addedRuns[0]?.pr === 4, JSON.stringify(m?.addedRuns));
  check("an identical file is reported unchanged", plan.files.find((f) => f.name === "inflight-runs.jsonl")?.unchanged === true);
  const copied = applyRefresh(plan);
  check("applying copies the changed file and only that one", copied.join() === "metrics.jsonl", copied.join());
  check("the snapshot now holds the source bytes", readFileSync(join(t.to, "metrics.jsonl"), "utf8") === readFileSync(join(t.from, "metrics.jsonl"), "utf8"));
}
{
  const patched = row(3, "c", { posted: true, verdict: "APPROVE" });
  const t = setup({ metrics: jsonl([OLD[0], OLD[1], patched]) });
  const plan = planRefresh({ fromDir: t.from, toDir: t.to });
  const m = plan.files.find((f) => f.name === "metrics.jsonl");
  check("the last metrics row patched in place (same pr and headSha) is accepted", plan.ok && m?.patchedLast === true, plan.errors.join("; "));
}

console.log("\n--- refused ---");
const refused = (name, metrics, expect, opts = {}) => {
  const t = setup({ metrics, ...opts });
  const before = readFileSync(join(t.to, "metrics.jsonl"), "utf8");
  const plan = planRefresh({ fromDir: t.from, toDir: t.to });
  check(name, !plan.ok && plan.errors.some((e) => e.includes(expect)), plan.errors.join("; ") || "accepted");
  let threw = false;
  try {
    applyRefresh(plan);
  } catch {
    threw = true;
  }
  check(`${name}: applying it throws and writes nothing`, threw && readFileSync(join(t.to, "metrics.jsonl"), "utf8") === before);
};
refused("a row missing from the middle", jsonl([OLD[0], OLD[2], row(4, "d")]), "line 2 differs");
refused("two rows in another order", jsonl([OLD[1], OLD[0], OLD[2]]), "line 1 differs");
refused("a shorter ledger", jsonl([OLD[0], OLD[1]]), "fewer than the snapshot");
refused("a last row replaced by a different run", jsonl([OLD[0], OLD[1], row(9, "z")]), "line 3 differs");
refused("a corrupt line", jsonl([...OLD, "{ not json"]), "line 4 is not JSON");
refused("a ledger file the source does not have", jsonl([...OLD]), "inflight-runs.jsonl: missing", { inflight: null });
refused(
  "an inflight line changed in place (only metrics rows are patched after a run)",
  jsonl([...OLD]),
  "inflight-runs.jsonl: line 1 differs",
  { inflight: [JSON.stringify({ event: "started", pr: 1, headSha: "a", extra: 1 })] },
);

console.log("\n--- the command line ---");
{
  const t = setup({ metrics: jsonl([...OLD, row(4, "d")]) });
  const env = { ...process.env, TRIBUNAL_DATA_DIR: t.from, TRIBUNAL_HARNESS_DATA: t.harness };
  const script = join(HERE, "refresh-ledger-snapshot.mjs");
  const before = readFileSync(join(t.to, "metrics.jsonl"), "utf8");
  const dry = spawnSync(process.execPath, [script], { env, encoding: "utf8" });
  check("without --write it exits 0 and says it is a dry run", dry.status === 0 && dry.stdout.includes("dry run"), dry.stdout + dry.stderr);
  check("without --write the snapshot is untouched", readFileSync(join(t.to, "metrics.jsonl"), "utf8") === before);
  check("it names the new run", dry.stdout.includes("new run: #4"), dry.stdout);
  const wet = spawnSync(process.execPath, [script, "--write"], { env, encoding: "utf8" });
  check("with --write it copies", wet.status === 0 && readFileSync(join(t.to, "metrics.jsonl"), "utf8") === readFileSync(join(t.from, "metrics.jsonl"), "utf8"), wet.stdout + wet.stderr);
}
{
  const t = setup({ metrics: jsonl([OLD[1], OLD[0], OLD[2]]) });
  const env = { ...process.env, TRIBUNAL_DATA_DIR: t.from, TRIBUNAL_HARNESS_DATA: t.harness };
  const out = spawnSync(process.execPath, [join(HERE, "refresh-ledger-snapshot.mjs"), "--write"], { env, encoding: "utf8" });
  check("a refused plan exits 1 even with --write", out.status === 1 && out.stderr.includes("REFUSED"), out.stdout + out.stderr);
}

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all passed"}`);
process.exit(failures.length ? 1 : 0);
