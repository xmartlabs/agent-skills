#!/usr/bin/env node
// external-slot-autopsy.mjs — re-derive the "27% of external slots fail" claim
// (cost-architecture-analysis §6.quater d) from metrics.jsonl, split by HEAD and
// by whether the run predates the 2026-07-23 slot fixes. Costs 0 tokens.
//
// Usage: node eval/external-slot-autopsy.mjs [--dirs]
//        --dirs also inspects surviving artifact dirs to correlate
//        `orphan-artifact` counts with refined (round-1) slots.
//
// Why it exists. The priority table ranked this lever as "a bug, nobody has
// looked at why" with a ~29M waste estimate, and neither number was derivable:
// the 268 denominator does not reproduce, and the waste was never attributed to
// a measured per-slot cost. Both had to be recomputed before the lever could be
// ranked — the standing rule that an estimate must not order the work.
//
// The split that matters is not the aggregate rate. It is (a) which head failed
// and (b) whether the run predates the sticky-routing / failure-class fixes,
// because those two cuts move the number from "27% systemic" to "one retired
// head plus a class that is now zero".

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { canonicalRows, summary } from "./rows.mjs";
import { join, resolve } from "node:path";
import { AGENT_COSTS_PATH } from "./paths.mjs";

const withDirs = process.argv.includes("--dirs");

// The 2026-07-23 fixes: sticky routing (markUnhealthy), "unparseable" split out
// of "refused", and "harness" no longer latching a family unhealthy. Runs at or
// after this timestamp carry all three. Chosen from the run sequence, not from a
// commit date — ~/.claude is not a git repo, so the code has no dated history to
// bisect and the boundary is an OBSERVED one: the last run with a codex failure
// is 2026-07-23T13:06Z, the first clean one 2026-07-23T17:20Z.
const FIX_CUT = "2026-07-23T17:00:00Z";

// canonicalRows, not a raw read: 5 of the 48 rows predate the canonical shape
// and are documented as not comparable. The first version of this file read the
// file raw — written the same day its author cited that warning.
const rows = canonicalRows();
console.log(`  ${summary().line}`);

const tally = () => ({ votes: 0, fails: 0, reasons: {} });
const byHead = { codex: tally(), gemini: tally() };
const byEra = { pre: { codex: tally(), gemini: tally() }, post: { codex: tally(), gemini: tally() } };
let undetailed = 0;
let totalFailedSlots = 0;

for (const row of rows) {
  const era = (row.ts || "") < FIX_CUT ? "pre" : "post";
  for (const f of row.findings || []) {
    for (const v of f.votes || []) {
      const s = v.source;
      if (byHead[s]) {
        byHead[s].votes++;
        byEra[era][s].votes++;
      }
    }
    const detail = f.failedSlotsDetail || [];
    const slots = f.failedSlots || [];
    totalFailedSlots += slots.length;
    // A failedSlots entry with no detail row cannot be attributed to a head:
    // failedSlotsDetail postdates the earliest runs. Counted separately rather
    // than folded into either head — an unattributable failure is not evidence
    // about codex or about gemini.
    if (slots.length && !detail.length) undetailed += slots.length;
    for (const d of detail) {
      const s = d.source;
      if (!byHead[s]) continue;
      byHead[s].fails++;
      byEra[era][s].fails++;
      const k = d.reason || "unspecified";
      byHead[s].reasons[k] = (byHead[s].reasons[k] || 0) + 1;
    }
  }
}

const pct = (f, v) => (f + v ? ((f / (f + v)) * 100).toFixed(1) + "%" : "n/a");
const line = (name, t) =>
  `  ${name.padEnd(8)} ${String(t.votes).padStart(4)} votes  ${String(t.fails).padStart(3)} fails  ${pct(t.fails, t.votes).padStart(6)}`;

const extVotes = byHead.codex.votes + byHead.gemini.votes;
console.log(`rows: ${rows.length}`);
console.log(
  `\nEXTERNAL ATTEMPTS = external votes + failed slots = ${extVotes} + ${totalFailedSlots} = ${extVotes + totalFailedSlots}`,
);
console.log(
  `aggregate failure rate: ${pct(totalFailedSlots, extVotes)}  (§6.quater d claimed 72/268 = 27%)`,
);

console.log("\nBY HEAD (all runs)");
console.log(line("codex", byHead.codex));
console.log(line("gemini", byHead.gemini));
console.log(`  undetailed (head unknown): ${undetailed}`);

console.log("\nBY ERA (cut at the 2026-07-23 slot fixes)");
for (const era of ["pre", "post"]) {
  console.log(` ${era}-fix:`);
  console.log(line("codex", byEra[era].codex));
  console.log(line("gemini", byEra[era].gemini));
}

console.log("\nFAILURE REASONS");
for (const h of ["codex", "gemini"]) {
  const rs = Object.entries(byHead[h].reasons).sort((a, b) => b[1] - a[1]);
  console.log(`  ${h}: ${rs.map(([k, n]) => `${k}=${n}`).join(" ") || "none"}`);
}

// --- Cost per external slot, measured, not extrapolated from refuter averages.
// The external head is a bash subprocess inside a Claude WRAPPER agent, and one
// wrapper drives many slots — so the Claude-side cost of a failed slot is a
// fraction of an agent, not a whole one. Reported per RUN because the slot count
// per wrapper varies with the finding count.
const costPath = AGENT_COSTS_PATH;
if (existsSync(costPath)) {
  console.log("\nEXTERNAL-HEAD WRAPPER COST (Claude side, from transcripts)");
  const slotsByRun = new Map();
  for (const row of rows) {
    if (row.externalSlots != null) slotsByRun.set(`${row.pr}`, row.externalSlots);
  }
  for (const l of readFileSync(costPath, "utf8").split("\n").filter(Boolean)) {
    const d = JSON.parse(l);
    const e = (d.byKind || {})["external-heads"];
    if (!e || !e.agents) continue;
    const slots = slotsByRun.get(`${d.pr}`);
    const perSlot = slots ? `~${Math.round(e.inputTokens / slots / 1000)}k/slot` : "slots unknown";
    console.log(
      `  pr${d.pr} ${d.runId}: ${e.agents} wrapper(s), ${e.turns} turns, ` +
        `${(e.inputTokens / 1e6).toFixed(2)}M input  (${perSlot})`,
    );
  }
}

// --- The defect the aggregate hides: orphan artifacts track REFINED slots.
// `reconcile-votes.mjs` builds `<slotId>-<source>.out`, while a refine writes
// `<slotId>r1-<source>.out` (peer-review.js artifactKey). So every refined slot
// yields 2 spurious orphan entries (.out + .meta) and its vote is compared
// against the ROUND-0 artifact. Asserted here rather than described: the
// prediction is orphans === 2 x (round-1 .out files).
// Attribution is done against the run's OWN `recorded-votes.json` — the exact
// input the reconciler was given — rather than against the metrics row, whose
// per-finding votes carry `slotId: null` and so cannot be matched to a file at
// all. Runs that left no recorded-votes.json are reported as uncheckable rather
// than as explained: "did not fire" is not "does not exist".
if (withDirs) {
  console.log("\nORPHAN-ARTIFACT ATTRIBUTION (surviving dirs only)");
  for (const row of rows) {
    const dir = row.artifactDir;
    const recon = row.voteReconciliation;
    if (!dir || !recon || !existsSync(dir)) continue;
    const orphans = (recon.counts || {})["orphan-artifact"] || 0;
    if (!orphans) continue;
    const votesFile = join(dir, "recorded-votes.json");
    if (!existsSync(votesFile)) {
      console.log(
        `  ${row.ts} pr${row.pr}: orphans=${orphans} -> UNCHECKABLE (no recorded-votes.json)`,
      );
      continue;
    }
    const recorded = JSON.parse(readFileSync(votesFile, "utf8"));
    if (!recorded.length) {
      console.log(
        `  ${row.ts} pr${row.pr}: orphans=${orphans} -> UNCHECKABLE (empty recorded-votes.json)`,
      );
      continue;
    }
    const seen = new Set(recorded.map((v) => `${v.slotId}-${v.source}`));
    const buckets = {};
    for (const f of readdirSync(dir)) {
      const m = /^(.+?)-(codex|gemini)\.(out|meta)$/.exec(f);
      if (!m || seen.has(`${m[1]}-${m[2]}`)) continue;
      const k = `${/r\d+$/.test(m[1]) ? "round1" : "round0"}-${m[2]}`;
      buckets[k] = (buckets[k] || 0) + 1;
    }
    const unmatched = Object.values(buckets).reduce((a, b) => a + b, 0);
    const round1 = Object.entries(buckets)
      .filter(([k]) => k.startsWith("round1"))
      .reduce((a, [, n]) => a + n, 0);
    const verdict =
      unmatched === orphans && round1 === orphans
        ? "ALL from round-1 artifact naming"
        : `${round1}/${orphans} from round-1 naming; residue ${orphans - round1} unattributed`;
    console.log(
      `  ${row.ts} pr${row.pr}: orphans=${orphans} unmatchedFiles=${JSON.stringify(buckets)} -> ${verdict}`,
    );
  }
}
