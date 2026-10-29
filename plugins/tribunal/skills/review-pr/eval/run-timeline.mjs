#!/usr/bin/env node
// run-timeline.mjs — where a run's WALL CLOCK went, per agent.
//
// WHY THIS EXISTS. Every cost instrument in this harness measures tokens or
// agent counts, and on 2026-07-28 that turned out to be the wrong quantity. A
// 3-file PR was reported as "17 agents, 55% of them plumbing" and the fix
// removed one agent — which is a real saving and answers nothing, because the
// complaint was that the review took ten minutes. Agents and seconds are not
// the same axis: the biggest single agent in that run cost 8k output tokens and
// 160 SECONDS, a quarter of the wall clock, and no existing script could see it.
//
// What it shows, from the `timestamp` on every transcript line:
//   · each agent's start, end and duration, as a bar
//   · the PARALLELISM FACTOR — sum of durations over wall clock. At 1.0 the run
//     is a queue: nothing overlaps, and every dispatch is pure added latency.
//   · the gaps between agents, which are scheduling overhead nobody's prompt
//     controls and which scale with the number of dispatches
//
// Usage:
//   node eval/run-timeline.mjs                  — the most recent run on disk
//   node eval/run-timeline.mjs <runId-prefix>   — one run
//
// It reads the same transcript directories as eval/agent-cost.mjs, and those
// are DELETED over time. A timeline that matters should be pasted into
// RELEASES.md, not left to be re-derived.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readRunDir, findRunDirs } from "./agent-cost.mjs";
import { PROJECTS_DIR } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const PROJECTS = PROJECTS_DIR;

const wanted = process.argv[2] || "";

function candidateDirs() {
  const out = [];
  for (const project of readdirSync(PROJECTS)) {
    let dirs = [];
    try {
      dirs = findRunDirs(join(PROJECTS, project));
    } catch {
      continue;
    }
    out.push(...dirs);
  }
  return out;
}

const dirs = candidateDirs().filter((d) => !wanted || d.includes(wanted));
if (!dirs.length) {
  console.log(`no run transcript directory matches ${wanted || "(any)"}`);
  process.exit(1);
}
// Most recent by mtime — "the run I just did" is what you almost always want.
dirs.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
const dir = dirs[0];

const run = readRunDir(dir);
const label = Object.fromEntries(
  run.agents.map((a) => [a.agentId, `${a.kind}/${a.name}`]),
);

const rows = [];
for (const f of readdirSync(dir).filter((f) => /^agent-.*\.jsonl$/.test(f))) {
  const id = f.replace(/^agent-/, "").replace(/\.jsonl$/, "");
  let min = null;
  let max = null;
  for (const line of readFileSync(join(dir, f), "utf8").split("\n")) {
    if (!line.trim()) continue;
    let r;
    try {
      r = JSON.parse(line);
    } catch {
      continue;
    }
    if (!r.timestamp) continue;
    const t = Date.parse(r.timestamp);
    if (Number.isNaN(t)) continue;
    if (min === null || t < min) min = t;
    if (max === null || t > max) max = t;
  }
  // An agent with no parseable timestamp is not a zero-duration agent — it is
  // an agent this script cannot see. Counted separately rather than shown as 0s.
  if (min !== null) rows.push({ id, label: label[id] || "?", start: min, end: max });
}
const blind = run.agents.length - rows.length;

rows.sort((a, b) => a.start - b.start);
const t0 = Math.min(...rows.map((r) => r.start));
const tN = Math.max(...rows.map((r) => r.end));
const wall = (tN - t0) / 1000;
const SCALE = Math.max(1, Math.round(wall / 60)); // ~60 columns regardless of length

console.log(`\n${dir.split("/").pop()}  —  ${rows.length} agent(s), ${wall.toFixed(0)}s wall\n`);
console.log("start    end    dur  agent");
let prevEnd = null;
let gapTotal = 0;
for (const r of rows) {
  const s = (r.start - t0) / 1000;
  const e = (r.end - t0) / 1000;
  if (prevEnd !== null && s > prevEnd) gapTotal += s - prevEnd;
  prevEnd = Math.max(prevEnd ?? 0, e);
  const bar = " ".repeat(Math.round(s / SCALE)) + "#".repeat(Math.max(1, Math.round((e - s) / SCALE)));
  console.log(
    `${String(Math.round(s)).padStart(5)}s ${String(Math.round(e)).padStart(5)}s ${String(Math.round(e - s)).padStart(5)}s  ${r.label.padEnd(30)}${bar}`,
  );
}

const sum = rows.reduce((a, r) => a + (r.end - r.start) / 1000, 0);
const factor = sum / wall;
console.log(`\n  sum of agent durations  ${sum.toFixed(0)}s`);
console.log(`  wall clock              ${wall.toFixed(0)}s`);
console.log(`  parallelism factor      ${factor.toFixed(2)}x`);
if (factor < 1.15)
  console.log(
    `  -> the run is effectively a QUEUE. Every dispatch is added latency, so the\n     lever is what runs CONCURRENTLY, not how many agents there are.`,
  );
console.log(`  time in gaps between agents  ${gapTotal.toFixed(0)}s  (~${(gapTotal / Math.max(1, rows.length)).toFixed(0)}s per dispatch)`);
if (blind)
  console.log(`  ${blind} agent(s) had no parseable timestamp and are NOT in the numbers above`);

// The split that decides where to spend effort: agents that read the code and
// form an opinion, versus everything else.
const REVIEW = new Set(["lens", "refuter", "critic", "synthesize", "strengthen", "defense", "adjudicator", "external-heads", "bot-vetting", "verifier"]);
const isReview = (l) => REVIEW.has(l.split("/")[0]);
const reviewSum = rows.filter((r) => isReview(r.label)).reduce((a, r) => a + (r.end - r.start) / 1000, 0);
const plumbingSum = sum - reviewSum;
console.log(`\n  review work    ${reviewSum.toFixed(0)}s`);
console.log(`  plumbing       ${plumbingSum.toFixed(0)}s${plumbingSum > reviewSum ? "   <- more than the review itself" : ""}`);
