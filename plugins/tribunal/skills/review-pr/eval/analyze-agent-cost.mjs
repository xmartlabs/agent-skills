#!/usr/bin/env node
// analyze-agent-cost.mjs — where a peer-review run's money actually goes.
//
//   node eval/analyze-agent-cost.mjs              — every run still on disk
//   node eval/analyze-agent-cost.mjs --pr 1968    — only runs for one PR
//   node eval/analyze-agent-cost.mjs --dir <wf_…>  — one run directory
//
// Costs 0 tokens: it reads transcripts that already exist.
//
// The denominators this round commits to, stated here BEFORE the numbers, per
// specs/2026-07-24-cost-measurement-protocol.md:
//
//   inputTokens / turns    — how much context a turn re-reads (the multiplicand)
//   turns / agent, by kind — how many times it re-reads it (the multiplier)
//
// Those two, not the output-token total, are what a cost change in this harness
// has to move. Output is ~16% of the dollar cost and ~0.7% of the token volume.
//
// Output here is a LOWER BOUND (2026-10-17): subagent transcripts log a
// placeholder output_tokens, measured at 6-48% of the real figure. Every output
// column, ratio, share and dollar total built from it is labelled as a bound;
// `inputUnits` is the exact figure to compare runs on.
//
// Scope note: reads only this workspace's project subtree. The global
// ~/.claude/projects pool mixes sibling workspaces and is never swept.

import { resolve, join, basename } from "node:path";
import { existsSync, readFileSync, statSync } from "node:fs";
import { canonicalRows, METRICS_PATH } from "./rows.mjs";
import {
  findRunDirs,
  readRunDir,
  priceRun,
  isPeerReviewRun,
  inputUnitsOf,
  UNIT_WEIGHTS,
  DEFAULT_PRICES
} from "./agent-cost.mjs";
import { projectDir } from "./paths.mjs";

const PROJECT_DIR = projectDir();
// METRICS_PATH, not a locally rebuilt path: the log lives outside the plugin
// directory (which updates replace wholesale). Rebuilding it here pointed at a
// file that no longer exists — found 2026-07-30 by widening replay-rows.mjs to
// treat CONSTRUCTING the path as reading it.
const METRICS = METRICS_PATH;

const argv = process.argv.slice(2);
const argOf = (flag) => {
  const i = argv.indexOf(flag);
  return i === -1 ? null : argv[i + 1];
};
const onlyPr = argOf("--pr");
const onlyDir = argOf("--dir");

const n = (x) => Number(x || 0).toLocaleString("en-US");
const pct = (a, b) => (b > 0 ? `${((100 * a) / b).toFixed(0)}%` : "—");

// --- metrics rows, for pairing --------------------------------------------
// A run directory's mtime is when it was last written, i.e. run end; the row's
// `ts` is stamped in the persist step, seconds earlier. Pair on PR + nearest ts
// within a tolerance rather than assuming a fixed clock offset.
// canonicalRows(): 5 of 48 rows predate the canonical shape and are documented
// as not comparable — see eval/rows.mjs. This file feeds the cost analyses, so
// mixing shapes here is how an incomparable row reaches a published average.
const rows = canonicalRows().filter((r) => r.ts && r.pr);
const PAIR_TOLERANCE_MS = 20 * 60 * 1000;
const pairRow = (pr, dirMs) => {
  let best = null;
  let bestGap = Infinity;
  for (const r of rows) {
    if (String(r.pr) !== String(pr)) continue;
    const gap = Math.abs(new Date(r.ts).getTime() - dirMs);
    if (gap < bestGap) {
      bestGap = gap;
      best = r;
    }
  }
  return bestGap <= PAIR_TOLERANCE_MS ? { row: best, gapMs: bestGap } : null;
};

const prOf = (run) => {
  for (const a of run.agents) {
    const m = /PR #(\d+)/.exec(a.promptHead || "");
    if (m) return m[1];
  }
  return null;
};

// --- collect ---------------------------------------------------------------
let dirs = onlyDir ? [resolve(onlyDir)] : findRunDirs(PROJECT_DIR);
const runs = [];
const foreign = [];
for (const d of dirs) {
  let run;
  try {
    run = readRunDir(d);
  } catch {
    continue;
  }
  if (!run.agents.length && !run.missingTranscripts) continue;
  run.pr = prOf(run);
  run.mtimeMs = statSync(d).mtimeMs;
  if (onlyPr && String(run.pr) !== String(onlyPr)) continue;
  // Other workflows share the session tree. Excluded from every peer-review
  // statistic below, and counted out loud — folding a harness-audit run into a
  // per-lens average is how a number stops meaning what its label says.
  if (!isPeerReviewRun(run)) {
    foreign.push(run);
    continue;
  }
  runs.push(run);
}
runs.sort((a, b) => a.mtimeMs - b.mtimeMs);

if (foreign.length) {
  const fIn = foreign.reduce((s, r) => s + r.totals.inputTokens, 0);
  const fOut = foreign.reduce((s, r) => s + r.totals.outputTokens, 0);
  console.log(
    `\n(excluded: ${foreign.length} non-peer-review workflow run(s) in the same session tree — ${n(fIn)} input / ≥${n(fOut)} output tokens. Another workflow's cost, not this harness's.)`
  );
}

if (!runs.length) {
  console.log(`\nNo workflow run directories with transcripts found under\n  ${PROJECT_DIR}\n`);
  console.log(`They are deleted over time — that is why metrics.jsonl carries agentCost.\n`);
  process.exit(0);
}

// --- per run ---------------------------------------------------------------
console.log(`\n=== runs on disk (${runs.length}) ===\n`);
console.log(
  `${"when".padEnd(17)} ${"pr".padEnd(6)} ${"agents".padStart(6)} ${"turns".padStart(6)} ${"input".padStart(13)} ${"inputUnits".padStart(13)} ${"output≥".padStart(9)} ratio≤  in/turn  row?`
);
let anyMissing = 0;
for (const r of runs) {
  const t = r.totals;
  const ratio = t.outputTokens > 0 ? `${(t.inputTokens / t.outputTokens).toFixed(0)}x` : "—";
  const perTurn = t.turns > 0 ? n(Math.round(t.inputTokens / t.turns)) : "—";
  const paired = r.pr ? pairRow(r.pr, r.mtimeMs) : null;
  anyMissing += r.missingTranscripts;
  console.log(
    `${new Date(r.mtimeMs).toISOString().slice(0, 16).replace("T", " ").padEnd(17)} ${String(r.pr || "?").padEnd(6)} ${String(t.agents).padStart(6)} ${String(t.turns).padStart(6)} ${n(t.inputTokens).padStart(13)} ${n(Math.round(inputUnitsOf(t))).padStart(13)} ${n(t.outputTokens).padStart(9)} ${ratio.padStart(6)}  ${perTurn.padStart(7)}  ${paired ? "yes" : "no"}${r.missingTranscripts ? `  (+${r.missingTranscripts} transcript(s) gone)` : ""}`
  );
}

// --- aggregate by kind ------------------------------------------------------
const agg = {};
for (const r of runs)
  for (const a of r.agents) {
    const k = (agg[a.kind] = agg[a.kind] || {
      agents: 0,
      turns: 0,
      inputTokens: 0,
      outputTokens: 0,
      freshInput: 0,
      cacheCreation: 0,
      cacheRead: 0,
      maxTurns: 0,
      peak: 0
    });
    k.agents++;
    k.turns += a.turns;
    k.inputTokens += a.inputTokens;
    k.outputTokens += a.outputTokens;
    k.freshInput += a.freshInput;
    k.cacheCreation += a.cacheCreation;
    k.cacheRead += a.cacheRead;
    if (a.turns > k.maxTurns) k.maxTurns = a.turns;
    if (a.peakContext > k.peak) k.peak = a.peakContext;
  }
const totalIn = Object.values(agg).reduce((s, k) => s + k.inputTokens, 0);
const totalOut = Object.values(agg).reduce((s, k) => s + k.outputTokens, 0);

console.log(`\n=== by agent kind, all runs pooled ===\n`);
console.log(
  `${"kind".padEnd(15)} ${"n".padStart(4)} ${"turns".padStart(6)} ${"turns/agent".padStart(11)} ${"input".padStart(13)} ${"inputUnits".padStart(13)} ${"in share".padStart(8)} ${"in/turn".padStart(8)} ${"output≥".padStart(9)} ${"out share*".padStart(10)} ${"maxTurns".padStart(8)}`
);
for (const [kind, k] of Object.entries(agg).sort((a, b) => b[1].inputTokens - a[1].inputTokens)) {
  console.log(
    `${kind.padEnd(15)} ${String(k.agents).padStart(4)} ${String(k.turns).padStart(6)} ${(k.turns / k.agents).toFixed(1).padStart(11)} ${n(k.inputTokens).padStart(13)} ${n(Math.round(inputUnitsOf(k))).padStart(13)} ${pct(k.inputTokens, totalIn).padStart(8)} ${n(Math.round(k.inputTokens / Math.max(1, k.turns))).padStart(8)} ${n(k.outputTokens).padStart(9)} ${pct(k.outputTokens, totalOut).padStart(10)} ${String(k.maxTurns).padStart(8)}`
  );
}
console.log(
  `\n  output≥ is a lower bound (subagent transcripts log a placeholder). * out share is a share of\n  lower bounds, which can understate unevenly across kinds: neither a bound nor a measurement.`
);

// The classifier's own error bar. Never presented as zero by omission: a big
// unclassified share invalidates every per-kind line above it.
const u = agg.unclassified;
console.log(
  `\nunclassified: ${u ? u.agents : 0} agent(s), ${pct(u ? u.inputTokens : 0, totalIn)} of input.` +
    (u && u.inputTokens / totalIn > 0.05
      ? `  <-- ABOVE 5%: treat the per-kind breakdown as unreliable until these are matched.`
      : `  Per-kind breakdown is usable at this level.`)
);
if (!u)
  console.log(
    `  0% is COVERAGE, not generalisation: the patterns in agent-cost.mjs were derived from\n  this corpus, so a changed prompt will show up here as a new unclassified bucket — which\n  is the point. eval/replay-agent-cost.mjs pins them so the change is loud, not silent.`
  );
if (u) {
  const samples = [];
  for (const r of runs)
    for (const a of r.agents)
      if (a.kind === "unclassified" && samples.length < 5) samples.push(a.promptHead.slice(0, 90));
  for (const s of samples) console.log(`    ? ${s}`);
}

// --- turn distribution: the multiplier --------------------------------------
const all = runs.flatMap((r) => r.agents);
const byTurns = [...all].sort((a, b) => b.inputTokens - a.inputTokens);
const longTail = all.filter((a) => a.turns > 40);
console.log(`\n=== the multiplier: turns ===\n`);
console.log(
  `agents with >40 turns: ${longTail.length}/${all.length} — ${pct(longTail.reduce((s, a) => s + a.inputTokens, 0), totalIn)} of all input`
);
console.log(`\ntop 8 agents by input:`);
for (const a of byTurns.slice(0, 8))
  console.log(
    `  ${(a.inputTokens / 1e6).toFixed(2).padStart(6)}M  ${String(a.turns).padStart(3)} turns  peak ${n(a.peakContext).padStart(8)}  ${a.model.padEnd(7)} ${a.name.padEnd(18)} ${a.promptHead.slice(0, 44)}`
  );

// --- effort actually observed ----------------------------------------------
// Round 2 set effort:"low" on the mechanical dispatches. Whether that took is a
// question about runtime, not source, and this is the only place it can be
// answered: the transcripts record the effort each turn ran at.
console.log(`\n=== effort observed per kind (turn counts) ===\n`);
const effByKind = {};
for (const a of all) {
  const e = (effByKind[a.kind] = effByKind[a.kind] || {});
  const keys = Object.keys(a.efforts);
  if (!keys.length) e["(not recorded)"] = (e["(not recorded)"] || 0) + a.turns;
  for (const k of keys) e[k] = (e[k] || 0) + a.efforts[k];
}
for (const [kind, e] of Object.entries(effByKind))
  console.log(`  ${kind.padEnd(15)} ${Object.entries(e).map(([k, v]) => `${k}=${v}`).join("  ")}`);
console.log(
  `\n  "(not recorded)" is not the same as low. If the mechanical rows show no effort value,\n  whether round 2's effort:"low" reaches the API is UNSETTLED by this data.`
);

// --- money, as an illustration ---------------------------------------------
const pooledByModel = {};
for (const a of all) {
  const m = (pooledByModel[a.model] = pooledByModel[a.model] || {
    agents: 0,
    turns: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheRead: 0,
    cacheCreation: 0,
    freshInput: 0
  });
  m.agents++;
  m.turns += a.turns;
  m.inputTokens += a.inputTokens;
  m.outputTokens += a.outputTokens;
  m.cacheRead += a.cacheRead;
  m.cacheCreation += a.cacheCreation;
  m.freshInput += a.freshInput;
}
const priced = priceRun(pooledByModel, DEFAULT_PRICES);
const pooledInputUnits = Object.values(pooledByModel).reduce((s, m) => s + inputUnitsOf(m), 0);
const pooledOutputUnits = Object.values(pooledByModel).reduce((s, m) => s + m.outputTokens * UNIT_WEIGHTS.output, 0);
console.log(`\n=== units (tier-free weights; input exact, output a lower bound) ===\n`);
console.log(
  `  input units ${n(Math.round(pooledInputUnits))} (exact)   output units ≥${n(Math.round(pooledOutputUnits))}   total units ≥${n(Math.round(pooledInputUnits + pooledOutputUnits))}`
);
console.log(`\n=== dollars (ASSUMED list prices, mixed tiers — an illustration, not a bill) ===\n`);
for (const [m, v] of Object.entries(priced.per))
  console.log(`  ${m.padEnd(8)} input $${v.inputUsd.toFixed(2).padStart(8)}   output ≥$${v.outputUsdLowerBound.toFixed(2).padStart(7)}   total ≥$${v.totalUsd.toFixed(2)}`);
console.log(
  `  ${"TOTAL".padEnd(8)} input $${priced.inputUsd.toFixed(2).padStart(8)}   output ≥$${priced.outputUsdLowerBound.toFixed(2).padStart(7)}   total ≥$${priced.totalUsd.toFixed(2)}   output share ≥${(100 * priced.outputShare).toFixed(0)}%`
);
if (priced.unpriced.length) console.log(`  (no price for: ${priced.unpriced.join(", ")})`);
// Older runs have no meta.json sidecar, so their tier is a guess. Say how much
// of the total rests on that guess rather than letting the table imply it does not.
const unk = pooledByModel.unknown;
if (unk) {
  const unkIn = unk.inputTokens;
  console.log(
    `  ${unk.agents} agent(s) have no meta.json and no known model — ${pct(unkIn, totalIn)} of input, priced at sonnet rates by assumption.`
  );
}

// --- sectioning before/after, and why it is not a measurement ---------------
const withRow = runs
  .map((r) => ({ r, p: r.pr ? pairRow(r.pr, r.mtimeMs) : null }))
  .filter((x) => x.p);
const post = withRow.filter((x) => x.p.row.diffExposure && x.p.row.diffExposure.sectioningAvailable !== undefined);
const pre = withRow.filter((x) => !(x.p.row.diffExposure && x.p.row.diffExposure.sectioningAvailable !== undefined));
console.log(`\n=== round-2 diff sectioning: before / after ===\n`);
console.log(`  paired to a metrics row: ${withRow.length}/${runs.length}   pre-sectioning: ${pre.length}   post: ${post.length}`);
const lensTurns = (x) => {
  const l = x.r.byKind.lens;
  return l && l.agents ? l.turns / l.agents : null;
};
const fmtArm = (arm) => {
  const v = arm.map(lensTurns).filter((x) => x !== null);
  return v.length ? `${v.map((x) => x.toFixed(1)).join(", ")}  (mean ${(v.reduce((s, x) => s + x, 0) / v.length).toFixed(1)})` : "no lens agents";
};
console.log(`  turns per lens, pre : ${fmtArm(pre)}`);
console.log(`  turns per lens, post: ${fmtArm(post)}`);
console.log(
  `\n  DIRECTIONAL ONLY. With ${post.length} post-sectioning run(s) on disk this cannot separate the\n  change from run-to-run variance, and per-lens spread within one PR is 6-26% CV.\n  It becomes a measurement once several post runs exist — which is what agentCost\n  on the row is for, since these transcripts get deleted.`
);

if (anyMissing)
  console.log(
    `\nNOTE: ${anyMissing} agentId(s) named in a journal have no transcript left on disk. They are\ncounted as missing, never as zero-cost agents — the totals above understate those runs.`
  );
console.log();
