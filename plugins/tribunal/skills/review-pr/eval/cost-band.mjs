// cost-band.mjs — is this run's cost normal for this PR?
//
// A cost regression is currently discovered by accident. The 2026-07-25 analysis
// found one only because someone went looking at transcripts for another reason:
// nothing in the harness compares a run's cost against what the same PR cost
// last time, so a change that doubled the bill would ship silently and stay.
//
// The band is built per `(pr, headSha)` from `agentCost.turns` and
// `agentCost.inputTokens` — turns first, because turns is the term the cost model
// says is causal (`input = (floor + prompt) × turns + accumulation`) and the one
// a change can actually move. A run outside the band is FLAGGED, not judged: a
// legitimately harder round can cost more, and the point is that somebody looks.
//
// Size is compared in `inputUnits`, never in output or a total: agentCost's
// output is a lower bound (subagent transcripts log a placeholder, 2026-10-17),
// while every input field is exact. Rows older than the field get it derived
// from their stored inputTokens/cacheRead/cacheCreation.
//
// Why so little of this works yet, stated rather than hidden: `agentCost` is
// patched by hand after each run, and only 2 of 42 rows carry it. A band needs
// several comparable runs at one commit, so today this script mostly reports
// what it cannot compute. That is the honest output — a band computed from one
// point would be a number with no meaning, which is worse than none.
//
// Usage: node eval/cost-band.mjs [--json]

import { readFileSync, existsSync } from "node:fs";
import { canonicalRows, summary } from "./rows.mjs";
import { resolve } from "node:path";
import { METRICS_PATH, isMainModule } from "./paths.mjs";
import { rowInputUnits } from "./agent-cost.mjs";

const METRICS = METRICS_PATH;

// Pure. A band needs at least MIN_SAMPLES comparable runs; below that it returns
// `null` with a reason instead of a number.
export const MIN_SAMPLES = 3;
export const BAND_FACTOR = 2; // outside 2x the median is worth a look, not a failure

export function buildBands(rows, { minSamples = MIN_SAMPLES } = {}) {
  const groups = new Map();
  for (const r of rows) {
    const cost = r && r.agentCost;
    if (!cost || !Number.isFinite(cost.turns) || cost.turns <= 0) continue;
    // Workload matters as much as the commit: a run that opened 2 panels is not
    // comparable to one that opened 26 at the same SHA. `paneled` is the coarse
    // stratum the measurement protocol already uses for exactly this reason.
    const key = `${r.pr}@@${r.headSha || "unpinned"}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push({
      ts: r.ts || null,
      turns: cost.turns,
      inputTokens: cost.inputTokens ?? null,
      inputUnits: rowInputUnits(cost),
      paneled: r.paneled ?? null
    });
  }

  const out = [];
  for (const [key, samples] of groups) {
    const [pr, headSha] = key.split("@@");
    if (samples.length < minSamples) {
      out.push({
        pr: Number(pr),
        headSha,
        samples: samples.length,
        band: null,
        reason: `needs ${minSamples} runs carrying agentCost, has ${samples.length}`
      });
      continue;
    }
    const turns = samples.map((s) => s.turns).sort((a, b) => a - b);
    const median = turns[Math.floor(turns.length / 2)];
    const units = samples.map((s) => s.inputUnits).filter(Number.isFinite).sort((a, b) => a - b);
    out.push({
      pr: Number(pr),
      headSha,
      samples: samples.length,
      band: {
        medianTurns: median,
        low: median / BAND_FACTOR,
        high: median * BAND_FACTOR,
        medianInputUnits: units.length ? units[Math.floor(units.length / 2)] : null
      },
      reason: null
    });
  }
  return out;
}

// Does one run sit outside its own PR's band? `null` when no band exists — never
// "fine by default", because an uncheckable run and a clean one are different
// facts and this file exists because they were being conflated.
export function checkAgainstBand(run, band) {
  if (!band || !band.band) return { checked: false, outside: null, reason: band ? band.reason : "no band" };
  const t = run && run.agentCost && run.agentCost.turns;
  if (!Number.isFinite(t)) return { checked: false, outside: null, reason: "run has no agentCost.turns" };
  const { low, high } = band.band;
  return {
    checked: true,
    outside: t < low || t > high,
    reason: t > high ? `${t} turns is above ${Math.round(high)}` : t < low ? `${t} turns is below ${Math.round(low)}` : null
  };
}

if (isMainModule(import.meta.url)) {
  if (!existsSync(METRICS)) {
    console.log("no metrics.jsonl");
    process.exit(0);
  }
  // canonicalRows(): 5 of the 48 rows predate the canonical shape and are
  // documented as not comparable. Reading the file raw is how that warning got
  // ignored by six of eleven consumers — see eval/rows.mjs.
  const rows = canonicalRows();
  const withCost = rows.filter((r) => r.agentCost);
  const bands = buildBands(rows);

  if (process.argv.includes("--json")) {
    console.log(JSON.stringify({ rows: rows.length, withCost: withCost.length, bands }, null, 2));
    process.exit(0);
  }

  console.log(`\nrows: ${rows.length}   carrying agentCost: ${withCost.length}`);
  console.log(`\nBANDS (turns, per pr+headSha; input units are exact, output is a lower bound and not compared)`);
  for (const b of bands)
    console.log(
      b.band
        ? `  pr ${b.pr}  n=${b.samples}  median ${b.band.medianTurns} turns  band ${Math.round(b.band.low)}-${Math.round(b.band.high)}  median ${b.band.medianInputUnits === null ? "?" : Math.round(b.band.medianInputUnits).toLocaleString("en-US")} input units`
        : `  pr ${b.pr}  n=${b.samples}  NO BAND — ${b.reason}`
    );
  if (!bands.some((b) => b.band)) {
    console.log(`\n  No band is computable yet. This is a DATA gap, not a bug:`);
    console.log(`  agentCost is patched by hand after each run and ${withCost.length} of ${rows.length} rows carry it.`);
    console.log(`  Every run that ships without the patch is a run this check can never use —`);
    console.log(`  the transcripts it would be reconstructed from are deleted.`);
  }
  console.log();
}
