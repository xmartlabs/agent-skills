#!/usr/bin/env node
// Scores peer-review runs against the annotated corpus.
//
//   node eval/score.mjs            — score every corpus PR that has a canonical
//                                    metrics record; list the rest as not-run
//
// Matching is deliberately conservative: a finding only scores TP/FP when the
// file matches (when the annotation names one) AND enough description tokens
// overlap. Everything else lands in `unknown` — a queue for human judgment,
// never silently classified. The corpus grows by resolving unknowns, not by
// loosening the matcher.
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { isCanonical, latestPerCommit, METRICS_PATH } from "./rows.mjs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { collectOverrides } from "./operator-override.mjs";

const here = dirname(fileURLToPath(import.meta.url));
// Overridable so a replay can run THIS scorer against fixtures instead of
// re-implementing its classification — the same reason the pure regions exist.
// Unset in every normal invocation, so the default paths are the shipping ones.
const corpusDir = process.env.SCORE_CORPUS_DIR || CORPUS_DIR;
// METRICS_PATH, not a locally rebuilt path: the log moved out of the skill
// directory when the harness became a plugin, and this line kept pointing at the
// old location — so every corpus score silently reported "(score unavailable)",
// including the one written into a RELEASES.md freeze row on 2026-07-30.
// `replay-rows.mjs` is supposed to catch exactly this and did not: its guard
// matched `readFileSync(<path>)` textually, and this file reads `logPath`, built
// on a separate line. The guard has been widened.
const logPath = process.env.SCORE_METRICS || METRICS_PATH;

// An absent or empty corpus is the NORMAL early state, not an error: this file
// shipped before corpus/ existed and `readdirSync` threw, so the scorer could
// not even report that it had nothing to score.
if (!existsSync(corpusDir)) {
  console.log(`No corpus at ${corpusDir} — nothing to score yet. See eval/README.md for what an entry is.`);
  process.exit(0);
}
const corpus = readdirSync(corpusDir)
  .filter((f) => f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(join(corpusDir, f), "utf8")));

const rows = readFileSync(logPath, "utf8")
  .split("\n")
  .filter(Boolean)
  .map((l) => JSON.parse(l));

// Canonical rows only — pre-canonical shapes are documented as unaggregatable
// (references/metrics-schema.md). `mode` is the canonical-shape sentinel.
const canonical = rows.filter(isCanonical);
const skipped = rows.length - canonical.length;
// Re-runs of the SAME commit are not independent samples. PR 1957 has four rows
// for one headSha; counting them four times weights that commit 4x in every
// rate. `rerunOf` is set by the workflow (schemaVersion 2); older rows are
// de-duplicated by pr+headSha as a fallback.
// ...but "one per commit" must keep the LATEST, not the first. Dropping every
// `rerunOf` row let the FIRST run of a commit win permanently: PR #1796 went on
// being scored against a 2026-07-21 row carrying no lens, no evidence and no
// refutationType while a v20 re-run of the identical tree sat in the log unused,
// and `worksheet.mjs` — which takes the latest — was preparing findings the
// scorer would never look at. Same commit, two answers.
// `logPath`, not the default: SCORE_METRICS must keep working, or the replay
// silently scores the real corpus instead of its fixtures. It caught exactly that
// the moment this line was written without the argument.
const scored = latestPerCommit(logPath);
const reruns = canonical.length - scored.length;
const bySchema = canonical.reduce((acc, r) => {
  const v = r.schemaVersion || "legacy";
  acc[v] = (acc[v] || 0) + 1;
  return acc;
}, {});

import { matches } from "./annotation-match.mjs";
import { CORPUS_DIR } from "./paths.mjs";

let totals = { TP: 0, FP: 0, FN: 0, accurate: 0, undecidable: 0, unknown: 0 };
// Same annotation id in two buckets is a contradiction in the ground truth, and
// picking one silently would let every rate below be computed from a fact the
// corpus disagrees with itself about. Collected, reported, and exit non-zero —
// release.mjs runs this non-blocking, so a loud failure here costs a freeze
// nothing and costs a wrong number everything.
const bucketErrors = [];
const BUCKETS = ["knownReal", "knownBogus", "knownAccurateNonDefect", "undecidable"];
for (const entry of corpus) {
  const seen = new Map();
  for (const b of BUCKETS)
    for (const a of entry[b] || []) {
      if (seen.has(a.id)) bucketErrors.push(`PR #${entry.pr}: id "${a.id}" is in both ${seen.get(a.id)} and ${b}`);
      else seen.set(a.id, b);
    }
  for (const a of entry.undecidable || [])
    if (!a.reason || !String(a.reason).trim())
      bucketErrors.push(`PR #${entry.pr}: undecidable "${a.id}" has no reason — it must say what WOULD settle it`);
}

for (const entry of corpus) {
  // Same PR number in another repo is another PR: a corpus entry names its repo.
  const runs = scored.filter(
    (r) => r.pr === entry.pr && (!entry.repo || !r.repo || r.repo === entry.repo),
  );
  // Annotations describe the code at one commit. A later run on a fix commit
  // would score every annotated defect as missed, so the labelled commit wins
  // when it has a row and the latest run is only the fallback.
  const labelled = entry.reviewedSha || entry.headSha;
  const atLabelled = labelled ? runs.filter((r) => r.headSha === labelled) : [];
  const run = (atLabelled.length ? atLabelled : runs).at(-1);
  console.log(`\n=== PR #${entry.pr} [${entry.status} / ${entry.groundTruthTier}] ===`);
  if (run && labelled && run.headSha !== labelled)
    console.log(`  scoring ${String(run.headSha).slice(0, 8)}, not the labelled ${labelled.slice(0, 8)}: no row at the labelled commit`);

  if (!run) {
    console.log(`  not yet run with canonical metrics (${runs.length} runs found)`);
    if (entry.status === "annotated" && entry.knownReal.length)
      console.log(`  -> ${entry.knownReal.length} knownReal annotation(s) waiting for a corpus run`);
    continue;
  }
  if (entry.status !== "annotated") {
    console.log(`  has a run but no settled annotations — resolve the entry first`);
    continue;
  }

  const surviving = run.findings.filter((f) => f.survived);
  const hitReal = new Set();

  for (const f of surviving) {
    const real = entry.knownReal.find((a) => matches(f, a));
    const bogus = (entry.knownBogus || []).find((a) => matches(f, a));
    // A finding can be TRUE and still not be a defect — a perf nit, an unused
    // include, a linear scan. Scoring those as FP would teach every later
    // calibration that a CORRECT finding is a false positive, and push the
    // harness toward suppressing valid observations. Counted, separately.
    const accurate = (entry.knownAccurateNonDefect || []).find((a) => matches(f, a));
    // Not decidable from the code — needs production data or an incident. Kept
    // out of both rates and NAMED, so the queue can reach zero honestly instead
    // of leaving "not judged yet" and "cannot be judged" as the same value.
    const undecided = (entry.undecidable || []).find((a) => matches(f, a));
    if (real) {
      hitReal.add(real.id);
      totals.TP++;
      console.log(`  TP  ${f.title}  -> ${real.id}`);
    } else if (bogus) {
      totals.FP++;
      console.log(`  FP  ${f.title}  -> ${bogus.id}`);
    } else if (accurate) {
      totals.accurate++;
      console.log(`  ~~  ${f.title}  -> ${accurate.id} (accurate, not a defect)`);
    } else if (undecided) {
      totals.undecidable++;
      console.log(`  --  ${f.title}  -> ${undecided.id} (undecidable: ${undecided.reason})`);
    } else {
      totals.unknown++;
      console.log(`  ??  ${f.title}  -> no annotation: judge and add to the entry`);
    }
  }
  for (const a of entry.knownReal.filter((a) => !hitReal.has(a.id))) {
    totals.FN++;
    console.log(`  FN  MISSED: ${a.id} (${String(a.evidence || "").slice(0, 60)}…)`);
    // Was it found but killed? That distinction decides whether the miss is a
    // lens problem (never raised) or a panel problem (raised, then killed).
    const killed = run.findings.find((f) => !f.survived && matches(f, a));
    if (killed)
      console.log(`      NOTE: a lens DID raise it and the panel killed it (refuteFraction ${killed.refuteFraction}) — this is a T_KILL/panel problem, not a lens problem`);
  }
}

// --- operator overrides: the only ground truth here that is not a model ------
//
// Run over EVERY canonical row, not just the corpus PRs. An override is already
// a labelled panel error on its own; gating it behind a corpus entry is how the
// PR #1968 reinstatement sat unused. Corroboration against an annotation is
// reported when it exists, but is not required for the count.
const overrides = collectOverrides(canonical);
console.log(`\n=== operator overrides (${overrides.length}) ===`);
if (!overrides.length) {
  console.log(`  none recorded. A panel decision reversed by hand and NOT recorded here is`);
  console.log(`  ground truth thrown away — patch the row (see references/metrics-schema.md).`);
} else {
  const bad = overrides.filter((o) => !o.ok);
  const good = overrides.filter((o) => o.ok);
  const byBucket = good.reduce((a, o) => ({ ...a, [o.bucket]: (a[o.bucket] || 0) + 1 }), {});
  for (const o of good) {
    const rf = o.refuteFraction === null || o.refuteFraction === undefined ? "n/a" : o.refuteFraction;
    console.log(`  ${o.bucket}  PR #${o.pr}  ${o.title}`);
    console.log(`      panel said refuteFraction ${rf} vs kill ${o.threshold ?? "?"}; operator: ${o.direction}`);
    for (const w of o.warnings) console.log(`      warn: ${w}`);
  }
  // Not folded into TP/FP/FN: those are lens-and-panel outcomes scored against
  // the corpus, while these are labelled PANEL errors. Adding them to the same
  // totals would mix a measurement with its own correction.
  console.log(
    `  buckets: ${Object.entries(byBucket).map(([k, v]) => `${k}=${v}`).join(", ") || "none"}`,
  );
  if (good.length)
    console.log(
      `  these are labelled panel errors — the input for tuning T_KILL / DELTA, kept OUT of the TP/FP/FN totals above on purpose`,
    );
  for (const o of bad) {
    console.log(`  INVALID  PR #${o.pr}  ${o.title}`);
    for (const e of o.errors) console.log(`      error: ${e}`);
  }
  if (bad.length)
    console.log(
      `  ${bad.length} override(s) are NOT scored — an annotation that contradicts its own row would be fabricated ground truth. Fix the row.`,
    );
}

console.log(`\n=== totals ===`);
console.log(
  `  TP=${totals.TP}  FP=${totals.FP}  FN=${totals.FN}  accurate=${totals.accurate}  undecidable=${totals.undecidable}  unknown=${totals.unknown}`,
);
// FP rate keeps `accurate` in the DENOMINATOR and out of the numerator: a
// correct-but-unhelpful finding costs the reader attention without being a lie.
// `noise` is that cost, measured on its own so neither number hides the other.
// Both are withheld until the queue for a PR is empty — a rate computed over a
// partly-judged corpus is a confident number about nothing, which is the failure
// this whole exercise exists to avoid.
{
  const judged = totals.TP + totals.FP + totals.accurate;
  if (totals.unknown > 0) {
    console.log(
      `  FP rate / noise: NOT COMPUTED — ${totals.unknown} finding(s) still unjudged. A rate over a partly-labelled corpus is a confident number about nothing.`,
    );
  } else if (judged === 0) {
    console.log(`  FP rate / noise: nothing judged yet`);
  } else {
    console.log(`  FP rate = ${(totals.FP / judged).toFixed(3)}  (bogus / real+bogus+accurate)`);
    console.log(`  noise   = ${(totals.accurate / judged).toFixed(3)}  (accurate-but-not-a-defect / same denominator)`);
    if (totals.undecidable)
      console.log(`  ${totals.undecidable} undecidable finding(s) excluded from both — listed above with their reasons`);
  }
}
if (skipped) console.log(`  (${skipped} pre-canonical metrics rows skipped — documented as unaggregatable)`);
if (reruns) console.log(`  (${reruns} re-run row(s) excluded — same pr+headSha counted once)`);
console.log(`  rows by schemaVersion: ${Object.entries(bySchema).map(([k, v]) => `${k}=${v}`).join(", ")}`);
console.log(`  unknowns are a human-judgment queue: resolve them into the corpus entries.`);

if (bucketErrors.length) {
  console.log(`\n=== CORPUS ERRORS (${bucketErrors.length}) — the ground truth contradicts itself ===`);
  for (const e of bucketErrors) console.log(`  ${e}`);
  console.log(`  Every rate above is computed from this data. Fix the entries before trusting any of it.`);
  process.exit(1);
}
