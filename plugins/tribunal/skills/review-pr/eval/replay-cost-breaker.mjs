#!/usr/bin/env node
// replay-cost-breaker.mjs — the spend breaker and the lost-run detector.
//
// Two guards that only matter when something has already gone wrong, which is
// exactly why they need cover: a breaker that never fires and a detector that
// reports zero look identical to a healthy system. The detector DID report zero
// on a day a run had died, and that is what put both in this file.
//
// The breaker rule is sliced out of the shipping COST-BREAKER region, so this
// tests what runs. The detector is imported from eval/lost-runs.mjs.
//
// Usage: node eval/replay-cost-breaker.mjs [--verbose]

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { findLostRuns } from "./lost-runs.mjs";
import { buildBands, checkAgainstBand } from "./cost-band.mjs";
import { WORKFLOW } from "./paths.mjs";

const VERBOSE = process.argv.includes("--verbose");
const failures = [];
let checks = 0;

function eq(label, got, want) {
  checks++;
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g === w) {
    if (VERBOSE) console.log(`  ok   ${label}`);
    return;
  }
  failures.push(`${label}\n       got  ${g}\n       want ${w}`);
}
const ok = (label, cond) => eq(label, !!cond, true);

// The region needs BUDGET_RESERVE, which lives beside it outside the markers, so
// it is injected the same way the gates region gets T_KILL: by evaluating the
// constant into scope. Kept explicit rather than moved inside the markers,
// because the constant belongs with the other spend ceilings.
const { api } = loadRegion(WORKFLOW, "COST-BREAKER", ["costBreakerState"]);

// --- 1. no budget declared: only the ceiling can fire ------------------------

const noBudget = (spent, ceiling = 1_200_000) =>
  api.costBreakerState({ spentOutput: spent, budgetTotal: null, budgetRemaining: Infinity, ceiling });

eq("under the ceiling: no degradation", noBudget(900_000).degrade, false);
eq("at the ceiling: degrade", noBudget(1_200_000).degrade, true);
eq("over the ceiling: degrade", noBudget(1_500_000).degrade, true);
ok("the reason names the ceiling and the proxy", /output ceiling.*proxy for input/.test(noBudget(1_500_000).reason));
// A real large run lands at 150k-950k output. The ceiling exists to catch a
// runaway, so a normal expensive PR must pass — a breaker that fires on healthy
// runs would be worse than none, since it degrades adjudication silently.
eq("a real large run (950k) does not trip it", noBudget(950_000).degrade, false);

// --- 2. a declared budget takes precedence ----------------------------------

const withBudget = (total, remaining, spent = 0) =>
  api.costBreakerState({ spentOutput: spent, budgetTotal: total, budgetRemaining: remaining, ceiling: 1_200_000 });

eq("plenty left: no degradation", withBudget(500_000, 300_000).degrade, false);
eq("inside the reserve: degrade", withBudget(500_000, 40_000).degrade, true);
eq("exactly at the reserve: degrade", withBudget(500_000, 60_000).degrade, true);
eq("just above the reserve: no degradation", withBudget(500_000, 60_001).degrade, false);
ok("the reason names the declared budget", /declared budget/.test(withBudget(500_000, 0).reason));
// The reserve is what pays for synthesis and persist. A run that spends to zero
// produces no report and no metrics row — the expensive failure, not the cheap one.
ok("the reserve is why: it protects synthesis + persist", withBudget(500_000, 1).degrade);

// A declared budget must not MASK the ceiling: a caller who declares a huge
// target still gets the runaway guard.
eq("a large declared budget still respects the ceiling", withBudget(9_000_000, 8_000_000, 1_300_000).degrade, true);

// --- 3. degenerate inputs ---------------------------------------------------

eq("no signal at all: no degradation", api.costBreakerState({}).degrade, false);
eq(
  "budgetTotal 0 is 'not declared', not 'exhausted'",
  api.costBreakerState({ spentOutput: 10, budgetTotal: 0, budgetRemaining: 0, ceiling: 1_200_000 }).degrade,
  false
);
eq(
  "a ceiling of 0 is disabled, not instant",
  api.costBreakerState({ spentOutput: 10, budgetTotal: null, budgetRemaining: Infinity, ceiling: 0 }).degrade,
  false
);
eq("spentOutput is echoed for the row", noBudget(1_300_000).spentOutput, 1_300_000);

// --- 4. the lost-run detector ------------------------------------------------

const started = (pr, sha, ts) => JSON.stringify({ event: "started", pr, headSha: sha, ts });
const completed = (pr, sha, ts) => JSON.stringify({ event: "completed", pr, headSha: sha, ts });

eq(
  "a matched pair is not lost",
  findLostRuns([started(1, "a", "t1"), completed(1, "a", "t2")]).lost.length,
  0
);
eq("an unmatched start is lost", findLostRuns([started(1, "a", "t1")]).lost.length, 1);

// THE REGRESSION. Three runs at one commit, two finished. The set-membership
// version reported zero here, because any completion marked the key done and the
// dict kept only the last start.
const sameSha = findLostRuns([
  started(1935, "abc", "t1"),
  completed(1935, "abc", "t2"),
  started(1935, "abc", "t3"),
  completed(1935, "abc", "t4"),
  started(1935, "abc", "t5")
]);
eq("three starts, two completions at ONE sha: exactly one lost", sameSha.lost.length, 1);
// Guarded: when this regresses `lost` is empty, and an unguarded index read
// would crash the file before the summary — hiding the other 24 results behind a
// stack trace.
eq("and it is the LAST start, not the first", sameSha.lost.length ? sameSha.lost[0].ts : "NONE FOUND", "t5");

// Matching is LIFO over the chronological event stream: reviews run one at a
// time, so a completion belongs to the most recent start still open. With three
// starts and one completion the survivor is the LAST start, and the two earlier
// ones are the losses.
//
// This assert previously expected ["t2","t3"] — the positional answer, pairing
// the first n starts with the n completions. That version named the WRONG RUNS
// on real data: it declared a run that had finished ten minutes earlier lost,
// while treating a five-hour-old start that really had died as completed.
const twoLost = findLostRuns([
  started(1, "a", "t1"),
  started(1, "a", "t2"),
  started(1, "a", "t3"),
  completed(1, "a", "t4")
]);
eq("three starts, one completion: the two OLDEST are lost", twoLost.lost.map((e) => e.ts), ["t1", "t2"]);

// Interleaved: a completion lands between two starts, so it belongs to the one
// that opened just before it — not to the oldest still-open run.
const interleaved = findLostRuns([
  started(1, "a", "2026-07-24T15:00:00Z"),
  started(1, "a", "2026-07-24T16:00:00Z"),
  completed(1, "a", "2026-07-24T16:30:00Z"),
  started(1, "a", "2026-07-24T17:00:00Z"),
  completed(1, "a", "2026-07-24T17:30:00Z")
]);
eq(
  "interleaved: only the run nobody ever finished is lost",
  interleaved.lost.map((e) => e.ts),
  ["2026-07-24T15:00:00Z"]
);

// An unmatched start is indistinguishable from a run STILL RUNNING. Age is what
// separates them, and it is reported rather than guessed: a check performed
// during a review must not report that review as a loss.
const now = Date.parse("2026-07-25T12:00:00Z");
const fresh = findLostRuns([started(1, "a", "2026-07-25T11:55:00Z")], now);
eq("a start 5 minutes old is flagged as possibly still running", fresh.lost[0].possiblyRunning, true);
const stale = findLostRuns([started(1, "a", "2026-07-24T11:55:00Z")], now);
eq("a start a day old is not", stale.lost[0].possiblyRunning, false);
eq("without a clock, nothing is claimed either way", findLostRuns([started(1, "a", "t1")]).lost[0].possiblyRunning, false);

// A run that died before resolving headRefOid must not be paired with one that
// resolved it — a null SHA is its own bucket, never a wildcard.
const nullSha = findLostRuns([started(1, null, "t1"), completed(1, "abc", "t2")]);
eq("a null headSha is not matched by a resolved one", nullSha.lost.length, 1);

// More completions than starts is a data problem, not a negative count.
eq(
  "more completions than starts yields no lost run and no negative",
  findLostRuns([started(1, "a", "t1"), completed(1, "a", "t2"), completed(1, "a", "t3")]).lost.length,
  0
);

const dirty = findLostRuns(["{ not json", "", JSON.stringify({ event: "weird", pr: 1 }), started(2, "b", "t1")]);
eq("malformed lines are counted, not silently dropped", dirty.malformed, 2);
eq("and the good line still parses", dirty.lost.length, 1);

// --- 5. the cost band -------------------------------------------------------

const row = (pr, sha, turns, ts) => ({ pr, headSha: sha, ts, agentCost: { turns, inputTokens: turns * 50000 } });

eq(
  "band: one sample is not a band",
  buildBands([row(1, "a", 100, "t1")])[0].band,
  null
);
ok(
  "band: and it says why",
  /needs 3 runs/.test(buildBands([row(1, "a", 100, "t1")])[0].reason)
);
const band3 = buildBands([row(1, "a", 100, "t1"), row(1, "a", 200, "t2"), row(1, "a", 300, "t3")])[0];
eq("band: three samples give a median", band3.band.medianTurns, 200);
eq("band: the band is 2x either side", [band3.band.low, band3.band.high], [100, 400]);
// A row without agentCost contributes nothing — it is not a zero-cost run, it is
// an unmeasured one, and the whole file exists because those two got conflated.
eq(
  "band: rows without agentCost are excluded, not treated as 0",
  buildBands([row(1, "a", 100, "t1"), { pr: 1, headSha: "a", agentCost: null }])[0].samples,
  1
);
eq(
  "band: a different sha is a different band",
  buildBands([row(1, "a", 100, "t1"), row(1, "b", 100, "t2")]).length,
  2
);
// An uncheckable run is reported as UNCHECKED, never as clean.
eq(
  "band: no band means not checked, not 'fine'",
  checkAgainstBand(row(1, "a", 900, "t9"), buildBands([row(1, "a", 100, "t1")])[0]),
  { checked: false, outside: null, reason: "needs 3 runs carrying agentCost, has 1" }
);
const chk = checkAgainstBand(row(1, "a", 900, "t9"), band3);
eq("band: a 900-turn run against a 200 median is outside", [chk.checked, chk.outside], [true, true]);
ok("band: and the reason names the number", /900 turns is above 400/.test(chk.reason));
eq(
  "band: an in-band run is not flagged",
  checkAgainstBand(row(1, "a", 210, "t9"), band3).outside,
  false
);

// Size is compared in inputUnits, the exact figure: agentCost's output is a lower
// bound (2026-10-17). A new row carries inputUnits; an old one gets it derived
// from inputTokens/cacheRead/cacheCreation (fresh = inputTokens - both caches).
const unitsRow = (ts, agentCost) => ({ pr: 2, headSha: "u", ts, agentCost: { turns: 100, ...agentCost } });
const unitsBand = buildBands([
  unitsRow("t1", { inputUnits: 500, inputTokens: 1, cacheRead: 0, cacheCreation: 0, outputTokens: 9e9 }),
  // old row: 100 fresh + 1000*1.25 + 10000*0.1 = 2350, output must not count
  unitsRow("t2", { inputTokens: 11100, cacheRead: 10000, cacheCreation: 1000, outputTokens: 9e9 }),
  unitsRow("t3", { inputUnits: 9000, inputTokens: 1, cacheRead: 0, cacheCreation: 0, outputTokens: 0 })
])[0];
eq("band: medianInputUnits is the median of exact inputUnits, old row derived", unitsBand.band.medianInputUnits, 2350);
eq(
  "band: an old row with no cache split has no inputUnits, never a guess",
  buildBands([unitsRow("t1", { inputTokens: 11100 }), unitsRow("t2", { inputTokens: 1 }), unitsRow("t3", { inputTokens: 2 })])[0].band.medianInputUnits,
  null
);

// --- the ceiling measures THIS RUN, not the turn ----------------------------
// budget.spent() is shared across the main loop and every workflow in a turn, so
// before 2026-07-27 a cheap review launched late in a session tripped the ceiling
// on tokens other runs had spent. PR #1923 fired at 1229783 >= 1200000 and lost a
// refine while its own output was 38.6k. These arms pin the fix in both
// directions: the turn total alone must NOT degrade, and the run's own spend must.

const runScoped = (turn, run, ceiling = 1_200_000) =>
  api.costBreakerState({
    spentOutput: turn,
    runOutput: run,
    budgetTotal: null,
    budgetRemaining: Infinity,
    ceiling
  });

eq(
  "PR #1923 verbatim: turn over the ceiling, run at 3% of it -> no degradation",
  runScoped(1_229_783, 38_596).degrade,
  false
);
eq(
  "the run's OWN spend still fires the ceiling",
  runScoped(1_229_783, 1_300_000).degrade,
  true
);
eq(
  "a first run in a turn is unaffected (turn == run)",
  runScoped(1_500_000, 1_500_000).degrade,
  true
);
eq(
  "the reason names both numbers, so the row can say whose tokens fired it",
  /this run spent 1300000 >= 1200000; turn total 1400000/.test(
    runScoped(1_400_000, 1_300_000).reason || ""
  ),
  true
);
eq("runOutput is echoed back for the row", runScoped(900, 100).runOutput, 100);

// Omitting runOutput must keep the OLD meaning exactly — every caller and golden
// written before this change still means what it meant.
eq(
  "runOutput absent falls back to the turn total",
  api.costBreakerState({
    spentOutput: 1_300_000,
    budgetTotal: null,
    budgetRemaining: Infinity,
    ceiling: 1_200_000
  }).degrade,
  true
);

// The DECLARED budget stays turn-scoped on purpose: a budget the operator set for
// the turn really is shared, and running it out is a real stop condition whoever
// spent it. This arm exists so a future "make everything per-run" edit has to
// argue with it.
eq(
  "a declared budget still degrades on the TURN, even with a tiny run",
  api.costBreakerState({
    spentOutput: 999_000,
    runOutput: 10,
    budgetTotal: 1_000_000,
    budgetRemaining: 1_000,
    ceiling: 1_200_000
  }).degrade,
  true
);

// --- the run counter survives a turn ending under a background run -----------
// budget.spent() starts over when a new turn begins, and a background workflow
// keeps running across it. BE #2200 (2026-10-21) recorded review: -1610496 and
// its breaker measured a negative run. SPENT-COUNTER folds readings into a total
// that only grows; the shipping region is what runs here.

const counter = loadRegion(WORKFLOW, "SPENT-COUNTER", ["spentCounter"]);
ok("the SPENT-COUNTER region is pure", counter.violations.length === 0);
const feed = (readings) => {
  const q = [...readings];
  const c = counter.api.spentCounter(() => q.shift());
  const seen = [];
  while (q.length) seen.push(c.now());
  return { seen, resets: c.resets() };
};
eq("a steady counter reads as the run's own spend", feed([1000, 1500, 4000]), { seen: [500, 3000], resets: 0 });
eq(
  "the #2200 shape: a drop is a reset, never a negative phase",
  feed([1_700_000, 1_710_000, 90_000, 120_000]),
  { seen: [10_000, 100_000, 130_000], resets: 1 }
);
eq("a reset to zero adds nothing and still counts", feed([500, 0, 0]), { seen: [0, 0], resets: 1 });
eq("two resets are both counted", feed([10, 50, 5, 30, 2]), { seen: [40, 45, 70, 72], resets: 2 });

const src = readFileSync(WORKFLOW, "utf8");
ok("the breaker measures the run through the counter", src.includes("runOutput: runSpent.now(),"));
ok(
  "every phase boundary reads the counter, never the turn",
  ["tokensAtStart", "tokensAtContextEnd", "tokensAtReviewEnd", "tokensAtVerifyEnd", "tokensAtEnd"].every((n) =>
    src.includes(`const ${n} = runSpent.now();`)
  )
);
ok("the row records how many resets the counter saw", src.includes("counterResets: runSpent.resets(),"));

console.log(
  failures.length
    ? `\nreplay-cost-breaker: ${checks - failures.length}/${checks} checks pass\n\n` +
        failures.map((f) => `  FAIL ${f}`).join("\n") +
        "\n"
    : `\nreplay-cost-breaker: ${checks}/${checks} checks pass\n`
);
process.exit(failures.length ? 1 : 0);
