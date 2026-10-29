#!/usr/bin/env node
// replay-reconciliation.mjs — regression test for lib/reconcile-votes.mjs.
//
// Replays all 34 external-head slots of the PR #1911 run (wf_302b44be-3dd) —
// the raw CLI output each slot left behind, paired with the vote that run
// actually recorded — and asserts the reconciler still names the three real
// failures in that data:
//
//   10x abstained-despite-verdict — slots recorded UNAVAILABLE because the old
//       `grep -q "^VERDICT:"` classifier could not read a markdown verdict. The
//       vote existed; the record threw it away.
//    1x verdict-mismatch — a vote recorded NOT_REFUTED where codex's own final
//       output says REFUTED.
//    1x over-invoked — the same slot: its dispatch ran the CLI four times.
//
// That last pair is the whole point. It was originally found by an LLM auditor
// reading transcripts; this script rediscovers it from the artifacts alone,
// deterministically, with no model in the loop.
//
// Note what the corpus also shows: three other slots invoked the CLI twice and
// do NOT flag. Two invocations is what the bash's own rate-limit retry produces,
// so the threshold cannot go lower without crying wolf — and an agent that
// re-runs until the CLI agrees with it leaves a final artifact that reconciles
// cleanly. Verdict comparison alone would miss that; only the count catches it,
// and only above the legitimate retry. This is a detector, not a guarantee.
//
// Usage: node eval/replay-reconciliation.mjs [--verbose]

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { FIXTURES_DIR, lib } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE = join(FIXTURES_DIR, "vote-reconciliation");
const ROUNDS_FIXTURE = join(FIXTURES_DIR, "vote-reconciliation-rounds");
const MODEL_FIXTURE = join(FIXTURES_DIR, "vote-reconciliation-model");
const RECONCILER = lib("reconcile-votes.mjs");
// The test seam for the `classifier-failed` case: a path that resolves to
// nothing, so the reconciler's spawn fails the way an unrunnable classifier does.
// Note what this does NOT reproduce — the original Windows defect was EFTYPE
// (exec of a real, present `.sh`), not ENOENT. Reproducing EFTYPE from a test is
// platform-dependent by nature, so the interpreter FORM is pinned at the source
// level by eval/check-mirror.mjs instead, and this case covers what happens
// AFTER any such failure, on every platform.
const BROKEN_CLASSIFIER = {
  TRIBUNAL_CLASSIFIER: join(FIXTURES_DIR, "no-such-classifier.sh"),
};
const verbose = process.argv.includes("--verbose");

function reconcile(fixtureDir, env = {}, extraArgs = []) {
  const recorded = JSON.parse(readFileSync(join(fixtureDir, "recorded-votes.json"), "utf8"));
  // Only the fields the workflow actually hands the reconciler. `round` is
  // included because the shipping `externalVotes` emits it; a fixture whose
  // votes carry MORE than the wire shape would test something that never runs.
  const votes = recorded.map((r) => ({
    slotId: r.slotId,
    source: r.source,
    verdict: r.verdict,
    confidence: r.confidence,
    slotStatus: r.slotStatus,
    ...(r.round === undefined ? {} : { round: r.round }),
  }));
  const raw = execFileSync("node", [RECONCILER, join(fixtureDir, "artifacts"), ...extraArgs], {
    input: JSON.stringify(votes),
    encoding: "utf8",
    env: { ...process.env, ...env },
    // Capture the child's stderr instead of letting it through to ours. The
    // unrunnable-classifier case below provokes one bash diagnostic per slot,
    // and 34 of them buried in this replay's output would make a green run look
    // broken. A real failure still arrives on the thrown error's `stderr`.
    stdio: ["pipe", "pipe", "pipe"],
  });
  const result = JSON.parse(raw.slice(raw.indexOf("{")));
  const byKind = result.mismatches.reduce((acc, m) => {
    acc[m.kind] = (acc[m.kind] || 0) + 1;
    return acc;
  }, {});
  return { recorded, result, byKind };
}

function check(name, fixtureDir, expected, extra = () => [], env = {}) {
  const { recorded, result, byKind } = reconcile(fixtureDir, env);
  console.log(`\n${name}`);
  console.log(`  checked                    ${result.checked}`);
  console.log(`  clean slots                ${result.ok}`);
  for (const kind of Object.keys(byKind).sort())
    console.log(`  ${kind.padEnd(26)} ${byKind[kind]}`);

  if (verbose) {
    const byId = Object.fromEntries(recorded.map((r) => [r.slotId, r]));
    for (const m of result.mismatches) {
      const row = byId[m.slotId] || {};
      console.log(
        `    ${String(m.slotId).padStart(2)} ${String(m.source).padEnd(7)} ${m.kind.padEnd(26)} ${(row.agent || "").slice(0, 9)} cli x${row.cliInvocations ?? "?"}`,
      );
    }
  }

  const failures = [];
  if (result.checked !== expected.checked)
    failures.push(`checked ${result.checked}, expected ${expected.checked}`);
  if (result.ok !== expected.ok) failures.push(`ok ${result.ok}, expected ${expected.ok}`);
  for (const [kind, n] of Object.entries(expected.kinds)) {
    if ((byKind[kind] || 0) !== n) failures.push(`${kind}: got ${byKind[kind] || 0}, expected ${n}`);
  }
  // A new mismatch kind against a corpus whose ground truth has not moved is a
  // change in the reconciler, not a discovery — fail rather than absorb it.
  for (const kind of Object.keys(byKind)) {
    if (!(kind in expected.kinds)) failures.push(`unexpected mismatch kind: ${kind} (${byKind[kind]})`);
  }
  failures.push(...extra(result, byKind));
  return failures.map((f) => `[${name}] ${f}`);
}

console.log(`vote reconciliation replay — reconciler: ${RECONCILER}`);

const failures = [
  ...check(
    "PR #1911 — the recorded corpus (votes with NO round: back-compat)",
    FIXTURE,
    {
      checked: 34,
      ok: 23,
      kinds: { "abstained-despite-verdict": 10, "verdict-mismatch": 1, "over-invoked": 1 },
    },
    (result) => {
      // The verdict-mismatch and the over-invocation must land on the same slot:
      // the dispatch that re-ran codex four times and then reported the opposite
      // of what it got. If they ever separate, the corpus or the reconciler
      // changed meaning.
      const slotsFor = (kind) =>
        result.mismatches.filter((m) => m.kind === kind).map((m) => m.slotId).sort();
      return JSON.stringify(slotsFor("verdict-mismatch")) === JSON.stringify(slotsFor("over-invoked"))
        ? []
        : [
            `verdict-mismatch slots ${JSON.stringify(slotsFor("verdict-mismatch"))} != over-invoked slots ${JSON.stringify(slotsFor("over-invoked"))}`,
          ];
    },
  ),
  // Real artifacts from the 2026-07-25 #1911 run (wMo9p8), the run whose row
  // reported 12 orphans + 5 mismatches and failed its own self-audit — every one
  // of them spurious. Slots 6/12/15 were REFINED, so their vote is the ROUND-1
  // answer and lives in `<slot>r1-codex.out`; slots 0/3 were not. Nothing here is
  // synthetic: the bug is in filenames, and only real names carry it.
  //
  // Pre-fix output of this scenario, captured 2026-07-27 before touching the
  // reconciler — this is the red it had to be seen in:
  //   checked 5, clean slots 2, confidence-mismatch 2, orphan-artifact 6,
  //   vote-without-verdict 1
  // i.e. exactly the production symptom, reproduced offline in 0 tokens.
  ...check(
    "refined slots — the vote is the round-1 answer",
    ROUNDS_FIXTURE,
    {
      checked: 5,
      ok: 5,
      // One per superseded ARTIFACT KEY (slots 6/12/15 round-0), not per file:
      // `.out` and `.meta` are the same answer and must count once. Counting
      // files is what reported 6 dropped answers as 12.
      kinds: { "superseded-round": 3 },
    },
    (result) => {
      const superseded = result.mismatches
        .filter((m) => m.kind === "superseded-round")
        .map((m) => m.slotId)
        .sort((a, b) => a - b);
      // Named slots, not just a count: a rule that strips `r<N>` too eagerly
      // would still produce three of these, on the wrong slots.
      return JSON.stringify(superseded) === JSON.stringify([6, 12, 15])
        ? []
        : [`superseded-round on slots ${JSON.stringify(superseded)}, expected [6,12,15]`];
    },
  ),
  // Synthetic (see fixtures/vote-reconciliation-model/) — three codex slots,
  // two on one model and one on another, plus a gemini slot that never writes
  // a MODEL: line. Proves codexModelsUsed (a) reads the .meta sidecar's MODEL:
  // line the same way it already reads RUNS:/BLOCKED:, (b) dedupes+sorts, and
  // (c) never picks up a gemini slot's meta — gemini's model is fixed, not
  // config-driven, so it never writes the line at all.
  ...check(
    "codexModelsUsed — mixed models across codex slots, gemini slot ignored",
    MODEL_FIXTURE,
    { checked: 4, ok: 4, kinds: {} },
    (result) => {
      const expected = ["gpt-5.4", "gpt-5.6-sol"];
      return JSON.stringify(result.codexModelsUsed) === JSON.stringify(expected)
        ? []
        : [`codexModelsUsed ${JSON.stringify(result.codexModelsUsed)}, expected ${JSON.stringify(expected)}`];
    },
  ),
  // A classifier that could not RUN is not the same fact as an
  // artifact carrying no verdict, and until 2026-08-12 this file could not tell
  // them apart — which is precisely how the defect stayed uncharacterised for
  // three weeks. On Windows the classifier was spawned as a directly-executable
  // `.sh` (EFTYPE) against a path from the pre-plugin layout (ENOENT), and every
  // artifact came back as `vote-without-verdict`: a well-formed data result
  // naming the wrong culprit. The reds this replay reported — `ok 13, expected
  // 23`, `abstained-despite-verdict: got 0, expected 10` — read as a corpus
  // disagreement when the classifier had never run once.
  //
  // Same corpus as the first case, so the contrast is exact: 34 slots that
  // reconcile to `ok 23` with a working classifier must reconcile to `ok 0` with
  // a dead one, and to ZERO `vote-without-verdict` either way.
  ...check(
    "a classifier that cannot run is reported as such, never as a vote deviation",
    FIXTURE,
    {
      checked: 34,
      ok: 0,
      // `over-invoked` survives because it is read from the .meta sidecar before
      // the artifact is ever classified — the one finding that remains truthful
      // when the classifier is gone.
      kinds: { "classifier-failed": 34, "over-invoked": 1 },
    },
    (result) => {
      const f = [];
      // The headline count has to reach the caller as its own field. `counts`
      // alone is not enough: --compact caps the mismatch list at COMPACT_LIMIT,
      // and this is the number lib/persist-run.sh and workflows/tribunal.js
      // branch on.
      if (result.classifierFailed !== 34)
        f.push(`classifierFailed ${result.classifierFailed}, expected 34`);
      // Stated explicitly rather than left to the unexpected-kind guard above:
      // this exact substitution IS the defect, so it earns a named assertion.
      const disguised = result.mismatches.filter((m) => m.kind === "vote-without-verdict").length;
      if (disguised) f.push(`${disguised} slot(s) still disguised as vote-without-verdict`);
      // The reverse leak too. With `failclass !== "none"` the abstention branch
      // used to fall through in silence, so a dead classifier also switched OFF
      // the check that catches a head whose real verdict was thrown away.
      const swallowed = result.mismatches.filter((m) => m.kind === "abstained-despite-verdict").length;
      if (swallowed)
        f.push(`${swallowed} abstention(s) judged against a classifier that never ran`);
      return f;
    },
    BROKEN_CLASSIFIER,
  ),
];

// The field must survive --compact, which is the ONLY mode lib/persist-run.sh
// ever uses. Asserted separately because `check()` reads the full form: a
// `classifierFailed` that exists only there would be correct in this replay and
// absent on the one path that runs in production — the same shape of gap as the
// `detail`/`actual` eviction that hid the failure in the first place.
{
  // Only the --compact run happens here. The full form's value was already
  // computed AND asserted by the case above (`classifierFailed !== 34`), so
  // re-deriving it would mean 34 more spawn attempts against a path that does not
  // exist, to learn something this file has already proved. 34 is that same
  // pinned expectation, not a second magic number.
  const compact = reconcile(FIXTURE, BROKEN_CLASSIFIER, ["--compact"]).result;
  console.log("\nclassifierFailed survives --compact");
  console.log(`  --compact                  ${compact.classifierFailed}`);
  if (compact.classifierFailed !== 34)
    failures.push(`[--compact] classifierFailed ${compact.classifierFailed}, expected 34`);
}

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nall reconciliation expectations hold");
