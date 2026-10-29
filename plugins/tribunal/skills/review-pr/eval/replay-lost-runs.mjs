#!/usr/bin/env node
// replay-lost-runs.mjs: the recent-lost-runs warning the skill prints before a
// review. A run that starts and never completes is the one symptom of a persist
// step that fails on every run; findLostRuns has always counted it, and nobody
// looked until a teammate on Windows asked why metrics.jsonl had stopped.
//
// Usage: node eval/replay-lost-runs.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { findLostRuns, recentLost } from "./lost-runs.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

const NOW = Date.parse("2026-10-20T12:00:00Z");
const ev = (event, pr, ts, headSha = "abc") => JSON.stringify({ event, pr, repo: "o/r", headSha, ts });
const lines = [
  ev("started", 1, "2026-10-19T10:00:00Z"),
  ev("started", 2, "2026-09-30T11:00:00Z"),
  ev("completed", 2, "2026-09-30T11:20:00Z"),
  ev("started", 3, "2026-10-20T11:40:00Z"),
  ev("started", 4, "2026-10-01T09:00:00Z"),
  "not json",
];

console.log("\n--- which lost runs count as recent ---");
const recent = recentLost(lines, NOW, 7);
check("a start with no completion inside the window is reported", recent.map((e) => e.pr).join() === "1", recent.map((e) => e.pr).join());
check("a completed run is not", !recent.some((e) => e.pr === 2));
check("a run started 20 minutes ago may still be running, so it is not", !recent.some((e) => e.pr === 3));
check("a loss older than the window is not", !recent.some((e) => e.pr === 4));
check("a longer window brings the old one back", recentLost(lines, NOW, 30).map((e) => e.pr).sort().join() === "1,4");
check("findLostRuns still sees all three", findLostRuns(lines, NOW).lost.length === 3);

console.log("\n--- a loss that a later run completed after ---");
{
  const relaunch = [
    ev("started", 5, "2026-10-19T21:00:00Z"),
    ev("started", 5, "2026-10-19T21:02:00Z"),
    ev("completed", 5, "2026-10-19T21:07:00Z"),
  ];
  check("a relaunch at the same commit that completed hides the aborted start", recentLost(relaunch, NOW, 7).length === 0);
  check("the full report still lists the aborted start", findLostRuns(relaunch, NOW).lost.length === 1);
  const otherPr = [
    ev("started", 6, "2026-10-18T04:00:00Z", "x"),
    ev("started", 7, "2026-10-19T09:00:00Z"),
    ev("completed", 7, "2026-10-19T09:20:00Z"),
  ];
  check("a later completion of another PR hides it too", recentLost(otherPr, NOW, 7).length === 0);
  const broken = [
    ev("started", 8, "2026-10-17T09:00:00Z"),
    ev("completed", 8, "2026-10-17T09:20:00Z"),
    ev("started", 9, "2026-10-18T09:00:00Z"),
    ev("started", 10, "2026-10-19T09:00:00Z"),
  ];
  const b = recentLost(broken, NOW, 7).map((e) => e.pr).join();
  check("when nothing completed after the losses, every one is reported", b === "9,10", b);
}

console.log("\n--- the command line ---");
{
  const dir = mkdtempSync(join(tmpdir(), "lost-runs-"));
  const run = (rows) => {
    writeFileSync(join(dir, "inflight-runs.jsonl"), rows.join("\n") + "\n");
    return execFileSync("node", [fileURLToPath(new URL("./lost-runs.mjs", import.meta.url)), "--recent", "7"], {
      encoding: "utf8",
      env: { ...process.env, TRIBUNAL_DATA_DIR: dir },
    });
  };
  const now = new Date();
  const ago = (h) => new Date(now.getTime() - h * 3600000).toISOString().replace(/\.\d+Z$/, "Z");
  const bad = run([ev("started", 7, ago(30)), ev("started", 8, ago(26))]);
  check("it prints one WARNING line naming the PRs", /^WARNING: 2 run\(s\) in the last 7 day\(s\)/.test(bad) && /PR 7, 8/.test(bad) && bad.trim().split("\n").length === 1, bad);
  const clean = run([ev("started", 7, ago(30)), ev("completed", 7, ago(29.5))]);
  check("it prints nothing when every start has a completion", clean === "", JSON.stringify(clean));
  rmSync(dir, { recursive: true, force: true });
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe recent-lost-runs warning holds");
