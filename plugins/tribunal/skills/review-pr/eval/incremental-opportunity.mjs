// incremental-opportunity.mjs — how much is there in NOT re-reviewing a PR from
// scratch, and can it even be sized?
//
// The cost analysis ranks "no memoisation between runs" as the largest lever on
// the board: 1,278M of 1,531M corpus input (83%) is re-execution. This script
// takes that apart, because the headline number is not a saving.
//
// Three questions, in the order that matters:
//
//   1. WHAT KIND of re-run is it? A re-run at the SAME sha with a CHANGED FLAG is
//      an ablation — it was re-run precisely to get a different answer, and a
//      cache would have defeated its purpose. Only same-sha same-config repeats
//      are pure recompute.
//   2. For the production case — a re-review after the author pushes — how much
//      of the PR actually changed between the two reviews?
//   3. Does lens cost even TRACK file count? If it does not, "89% of files
//      skippable" is not "89% cheaper", and the whole lever shrinks.
//
// Question 3 is the one that stops this being implementable today, and it is
// exactly the question the four levers already killed on 2026-07-25 were not
// asked early enough.
//
// Usage: node eval/incremental-opportunity.mjs [--repo <owner/name>] [--json]

import { readFileSync, existsSync } from "node:fs";
import { canonicalRows } from "./rows.mjs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { METRICS_PATH } from "./paths.mjs";
import { operatorConfigPath, resolveRepo } from "../../../lib/operator.mjs";

const METRICS = METRICS_PATH;
const asJson = process.argv.includes("--json");
const repoArgAt = process.argv.indexOf("--repo");
// --repo, else defaultRepo from the operator config. No cwd fallback: these
// numbers are about the repo the ledger rows describe, not wherever this ran.
const resolveLedgerRepo = (flagValue) => {
  const r = resolveRepo({ flag: flagValue || "", cwdRepoOf: () => null });
  if (r.repo) return r.repo;
  const why = r.error.startsWith("no repo")
    ? `no repo: pass --repo <owner/name> or set "defaultRepo" in ${operatorConfigPath()}`
    : r.error;
  console.error(`incremental-opportunity: ${why}`);
  process.exit(2);
};
const REPO = resolveLedgerRepo(repoArgAt === -1 ? "" : process.argv[repoArgAt + 1]);

const gh = (args) => {
  try {
    const out = execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    return out.trim() ? JSON.parse(out) : null;
  } catch {
    return null;
  }
};

if (!existsSync(METRICS)) {
  console.error("no metrics.jsonl");
  process.exit(1);
}
// canonicalRows(): 5 of 48 rows predate the canonical shape and are documented
// as not comparable. Reading the file raw is how that warning got ignored by
// six of eleven consumers — see eval/rows.mjs.
const rows = canonicalRows().filter((r) => r.pr && r.headSha);

// --- 1. what kind of re-run ---------------------------------------------------
// The flags that make a re-run an ABLATION rather than a repeat. Deliberately
// includes the ceilings: raising maxPanels re-runs the same PR to get MORE
// adjudication, which is a different question, not the same one recomputed.
const config = (r) =>
  JSON.stringify([
    r.codexEnabled,
    r.geminiEnabled,
    r.batchPanels,
    r.botComments,
    r.lensCount,
    r.panelCeiling,
    r.agentCeiling
  ]);

const bySha = new Map();
const byPr = new Map();
for (const r of rows) {
  const k = `${r.pr}@@${r.headSha}`;
  if (!bySha.has(k)) bySha.set(k, []);
  bySha.get(k).push(r);
  if (!byPr.has(r.pr)) byPr.set(r.pr, new Set());
  byPr.get(r.pr).add(r.headSha);
}

let pureRepeats = 0;
let ablations = 0;
for (const runs of bySha.values()) {
  if (runs.length < 2) continue;
  const base = config(runs[0]);
  for (const r of runs.slice(1)) (config(r) === base ? (pureRepeats += 1) : (ablations += 1));
}

// --- 2. the production case ---------------------------------------------------
const reReviewed = [...byPr.entries()].filter(([, shas]) => shas.size > 1);
const deltas = [];
for (const [pr, shaSet] of reReviewed) {
  const ordered = [...shaSet].sort((a, b) => {
    const t = (s) => Math.min(...bySha.get(`${pr}@@${s}`).map((r) => Date.parse(r.ts || 0) || 0));
    return t(a) - t(b);
  });
  const first = ordered[0];
  const last = ordered[ordered.length - 1];
  const filesAtLast = gh(["api", `repos/${REPO}/compare/development...${last}`, "--jq", "[.files[].filename]"]);
  const changedBetween = gh(["api", `repos/${REPO}/compare/${first}...${last}`, "--jq", "[.files[].filename]"]);
  if (!filesAtLast || !changedBetween) {
    deltas.push({ pr, error: "gh could not resolve one of the compares" });
    continue;
  }
  const touched = filesAtLast.filter((f) => changedBetween.includes(f));
  deltas.push({
    pr,
    from: first.slice(0, 8),
    to: last.slice(0, 8),
    prFiles: filesAtLast.length,
    changed: touched.length,
    skippable: filesAtLast.length - touched.length
  });
}

// --- 3. does cost track file count? -------------------------------------------
// `context === 0` marks a RESUMED run whose review phase replayed from cache — its
// review cost is not this run's and folding it in understates every fresh run.
const pts = [];
for (const r of rows) {
  const co = r.claudeOutputTokens;
  if (!co || typeof co !== "object") continue;
  if (!r.fileCount || !r.lensCount || !co.review || !co.context) continue;
  pts.push({ files: r.fileCount, perLens: co.review / r.lensCount, pr: r.pr });
}
const mean = (a) => a.reduce((s, x) => s + x, 0) / (a.length || 1);
const small = pts.filter((p) => p.files <= 14).map((p) => p.perLens);
const big = pts.filter((p) => p.files > 14);
const bigVals = big.map((p) => p.perLens);
const bigPrs = [...new Set(big.map((p) => p.pr))];
const ratio = small.length && bigVals.length ? mean(bigVals) / mean(small) : null;
const fileRange = pts.length ? Math.max(...pts.map((p) => p.files)) / Math.min(...pts.map((p) => p.files)) : null;
// cost ~ files^k
const exponent = ratio && fileRange ? Math.log(ratio) / Math.log(fileRange) : null;

if (asJson) {
  console.log(JSON.stringify({ pureRepeats, ablations, deltas, ratio, exponent, bigPrs }, null, 2));
  process.exit(0);
}

console.log(`\n1. WHAT THE RE-RUNS ACTUALLY ARE`);
console.log(`   distinct (pr, sha) pairs : ${bySha.size}`);
console.log(`   re-runs at the same sha  : ${pureRepeats + ablations}`);
console.log(`     with a CHANGED flag (ablation — a cache would defeat it) : ${ablations}`);
console.log(`     with identical config (pure recompute, cacheable)        : ${pureRepeats}`);
console.log(`   The 83% "re-execution" headline is real as an observation and is`);
console.log(`   almost entirely the first line: re-running to get a DIFFERENT answer.`);

console.log(`\n2. THE PRODUCTION CASE — re-review after the author pushes`);
console.log(`   PRs reviewed at more than one sha: ${reReviewed.length} of ${byPr.size}`);
for (const d of deltas)
  console.log(
    d.error
      ? `   pr ${d.pr}: ${d.error}`
      : `   pr ${d.pr}  ${d.from} -> ${d.to}   ${d.changed} of ${d.prFiles} files changed, ${d.skippable} skippable`
  );
const tot = deltas.filter((d) => !d.error);
const totFiles = tot.reduce((s, d) => s + d.prFiles, 0);
const totChanged = tot.reduce((s, d) => s + d.changed, 0);
if (totFiles)
  console.log(
    `   TOTAL: ${totChanged} of ${totFiles} files changed between reviews — ${Math.round((100 * (totFiles - totChanged)) / totFiles)}% of the re-reading was of UNCHANGED files`
  );

console.log(`\n3. BUT DOES COST TRACK FILE COUNT? (this is what shrinks the lever)`);
console.log(`   n=${pts.length} fresh runs (resumed runs excluded: their review phase replayed from cache)`);
if (ratio) {
  console.log(`   <=14 files : ${Math.round(mean(small)).toLocaleString("en-US")} output tokens per lens (n=${small.length})`);
  console.log(`   > 14 files : ${Math.round(mean(bigVals)).toLocaleString("en-US")} output tokens per lens (n=${bigVals.length})`);
  console.log(`   ${ratio.toFixed(2)}x the cost for ${fileRange.toFixed(1)}x the files  ->  cost ~ files^${exponent.toFixed(2)}`);
  console.log(`\n   So skipping 89% of files does NOT save 89%. At that exponent it is nearer`);
  console.log(`   ${Math.round(100 * (1 - Math.pow(0.11, exponent)))}% of the review phase — and the review phase is not the whole run.`);
  console.log(`\n   TWO REASONS NOT TO USE EVEN THAT NUMBER:`);
  console.log(`   · every run in the >14 bucket is PR #${bigPrs.join(", #")} — the exponent may be`);
  console.log(`     measuring that PR's difficulty, not its file count. Same confound that`);
  console.log(`     invalidated two earlier analyses.`);
  console.log(`   · this is OUTPUT tokens, ~13% of the bill. The input side needs agentCost,`);
  console.log(`     which is patched by hand and present on 2 of 42 rows.`);
} else {
  console.log(`   not enough spread to say`);
}

console.log(`\nVERDICT: the lever is real in FILES and unsized in COST. What would size it is`);
console.log(`agentCost on more rows, which accrues for free as runs happen — no experiment needed.`);
console.log(`\nAnd before any implementation: an incremental review CHANGES WHAT THE AGENTS SEE,`);
console.log(`so it needs the ground-truth rig (eval/build-groundtruth-arms.mjs), not a cost`);
console.log(`measurement. The structural risk is specific: a re-review that skips unchanged`);
console.log(`files must carry FORWARD the prior findings for those files, or the second review`);
console.log(`reports fewer problems and reads as an improvement.\n`);
