#!/usr/bin/env node
// replay-vote-derivation.mjs — regression test for deriving external votes from
// their CLI artifacts instead of from what the wrapper agent said.
//
// On the 2026-07-23 PR #1911 re-run the panel recorded votes that its own
// artifacts contradict. Two of them, verbatim:
//
//   slot 0 · gemini   artifact: "gemini-exec.sh: line 22: 3: missing prompt"
//                     classifier: FAILCLASS unparseable
//                     recorded:   NOT_REFUTED, confidence 1.0, slotStatus OK
//
//   slot 1 · codex    artifact: the codex greeting, no task ever ran
//                     classifier: FAILCLASS unparseable
//                     recorded:   REFUTED, confidence 0.33, slotStatus OK
//
// A confidence of 1.0 is the maximum weight in the weighted tally, so the first
// one is a fabricated vote carrying more influence than any real one on that
// panel. Nothing in the harness could contradict it: the verdict was whatever
// the agent said it was.
//
// The fix derives each external vote from the artifact. This replays the real
// 35 recorded votes against the real 43 artifacts and asserts the derivation
// abstains where the CLI produced nothing and preserves the verdicts where it
// produced something. It runs the real applyArtifactTruth, cut out of the
// workflow by the ARTIFACT-TRUTH markers, so there is no second copy to drift.
//
// Those 35 votes were relayed: each external slot then had its own agent. Since
// the batched drivers, an external slot has no task and no relayed vote, and
// the second half of this replay covers that shape. Until v105 every successful
// driver-slot vote was counted as a correction of a CRASHED relay.
//
// Usage: node eval/replay-vote-derivation.mjs [--verbose]

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { FIXTURES_DIR, WORKFLOW } from "./paths.mjs";

const FIX = join(FIXTURES_DIR, "vote-derivation");
const verbose = process.argv.includes("--verbose");
const classified = JSON.parse(readFileSync(join(FIX, "classified.json"), "utf8"));
const recorded = JSON.parse(readFileSync(join(FIX, "recorded-votes.json"), "utf8"));

const failures = [];

function loadTruth() {
  const { api, violations } = loadRegion(
    WORKFLOW,
    "ARTIFACT-TRUTH",
    ["applyArtifactTruth", "voteCorrections"],
    ["ARTIFACT-KEY"],
  );
  if (violations.length)
    failures.push(`ARTIFACT-TRUTH region references run state: ${violations.join(" | ")}`);
  return api;
}

// The key is what links a slot to its artifact file, and classify-artifacts.sh
// and reconcile-votes.mjs build the same names. If it drifts, every lookup
// misses and every external vote silently becomes an abstention.
const { api: keyApi } = loadRegion(WORKFLOW, "ARTIFACT-KEY", ["artifactKey"]);
for (const [args, want] of [
  [[0, "codex", 0], "0-codex"],
  [[3, "gemini", 1], "3r1-gemini"],
  [[undefined, "codex", 0], "x-codex"],
])
  if (keyApi.artifactKey(...args) !== want)
    failures.push(`artifactKey(${args.join(", ")}) is "${keyApi.artifactKey(...args)}", expected "${want}"`);

// Each call gets a fresh region, so voteCorrections starts empty.
const relayedRun = loadTruth();
const RELAYED_SLOT = (source) => ({ source, task: () => undefined });
const derive = (api, v, slot, truth, slotId, round) =>
  api.applyArtifactTruth([v], [slot], truth, slotId, round)[0];

const truth = classified.entries;
const derived = recorded.map((v) => ({
  slotId: v.slotId,
  source: v.source,
  before: v,
  after: derive(relayedRun, v, RELAYED_SLOT(v.source), truth, v.slotId, 0),
}));
const corrections = relayedRun.voteCorrections;

const byKind = {};
for (const c of corrections) byKind[c.kind] = (byKind[c.kind] || 0) + 1;
const abstainedAfter = derived.filter((d) => d.after.slotStatus === "UNAVAILABLE").length;
const abstainedBefore = recorded.filter((v) => v.slotStatus === "UNAVAILABLE").length;

console.log("external vote derivation replay — PR #1911 artifacts\n");
console.log(`  artifacts classified    ${classified.total}`);
console.log(`  votes recorded          ${recorded.length}`);
console.log(`  corrections             ${corrections.length}  ${JSON.stringify(byKind)}`);
console.log(`  abstentions before      ${abstainedBefore}`);
console.log(`  abstentions after       ${abstainedAfter}`);

if (verbose)
  for (const c of corrections.slice(0, 12))
    console.log(
      `    slot ${c.slotId} ${c.source}: ${c.kind} — reported ${c.reported.verdict}@${c.reported.confidence}`,
    );

const find = (slotId, source) => derived.find((d) => d.slotId === slotId && d.source === source);

// 1 · the two documented fabrications become abstentions.
for (const [slotId, source, was] of [
  [0, "gemini", "NOT_REFUTED@1"],
  [1, "codex", "REFUTED@0.33"],
]) {
  const d = find(slotId, source);
  if (!d) failures.push(`slot ${slotId} ${source} missing from the fixture`);
  else if (d.after.slotStatus !== "UNAVAILABLE")
    failures.push(
      `slot ${slotId} ${source} (recorded ${was}) still counts as a vote — its artifact has no verdict`,
    );
  else if (d.after.failureReason !== "unparseable")
    failures.push(`slot ${slotId} ${source} abstained but as "${d.after.failureReason}"`);
}

// 2 · a slot whose CLI really did answer keeps its verdict and confidence.
const good = find(0, "codex");
if (!good || good.after.slotStatus !== "OK" || good.after.verdict !== "REFUTED")
  failures.push("slot 0 codex answered REFUTED and must survive derivation unchanged");
else if (Math.abs(good.after.confidence - 0.39) > 0.001)
  failures.push(`slot 0 codex confidence drifted to ${good.after.confidence}, expected 0.39`);

// 3 · derivation can only ever add abstentions, never invent a vote where the
//     panel recorded none — the correction is one-directional by construction.
if (abstainedAfter < abstainedBefore)
  failures.push("derivation turned an abstention into a vote — it must never manufacture one");

// 4 · every fabrication the run contained is caught, not just the two above.
if (corrections.length < 2)
  failures.push(`expected the known fabrications to be caught, got ${corrections.length}`);
if (!byKind["vote-without-verdict"])
  failures.push("no vote-without-verdict correction — the class the run actually exhibited");

// 5 · driver slots: no task, no relayed vote. The shape of the 2026-10-22
//     PR #2186 run, where three Codex slots answered NOT_REFUTED and the row
//     recorded three CRASHED relays "corrected" by their own artifacts.
const driverRun = loadTruth();
const DRIVER_SLOT = (source) => ({ source, task: null });
const driverTruth = {
  "0-codex": { failclass: "none", verdict: "NOT_REFUTED", confidence: 0.99, confidenceDeclared: true },
  "4-codex": { failclass: "none", verdict: "REFUTED", confidence: 0.8, confidenceDeclared: true },
  "5-codex": { failclass: "confined" },
};
const answered = derive(driverRun, undefined, DRIVER_SLOT("codex"), driverTruth, 0, 0);
const refuted = derive(driverRun, undefined, DRIVER_SLOT("codex"), driverTruth, 4, 0);
const confined = derive(driverRun, undefined, DRIVER_SLOT("codex"), driverTruth, 5, 0);
const missing = derive(driverRun, undefined, DRIVER_SLOT("codex"), driverTruth, 7, 0);

if (answered.slotStatus !== "OK" || answered.verdict !== "NOT_REFUTED" || answered.confidence !== 0.99)
  failures.push(`driver slot with a verdict must take it from the artifact, got ${JSON.stringify(answered)}`);
if (refuted.slotStatus !== "OK" || refuted.verdict !== "REFUTED")
  failures.push(`driver slot REFUTED artifact must stay REFUTED, got ${JSON.stringify(refuted)}`);
if (confined.slotStatus !== "UNAVAILABLE" || confined.failureReason !== "confined")
  failures.push(`driver slot with a confined artifact must abstain as confined, got ${JSON.stringify(confined)}`);
if (missing.slotStatus !== "UNAVAILABLE" || missing.failureReason !== "error")
  failures.push(`driver slot with no artifact must abstain as error, got ${JSON.stringify(missing)}`);
if (driverRun.voteCorrections.length !== 0)
  failures.push(
    `driver slots relay no vote, so nothing can be corrected; got ${driverRun.voteCorrections.length} correction(s): ${JSON.stringify(driverRun.voteCorrections.map((c) => `${c.slotId}:${c.kind}:${c.reported && c.reported.slotStatus}`))}`,
  );

// 6 · a relayed slot whose agent returned nothing is still a crash the
//     artifact overrode, so the driver carve-out must not swallow it.
const crashRun = loadTruth();
derive(crashRun, undefined, RELAYED_SLOT("codex"), driverTruth, 0, 0);
if (crashRun.voteCorrections.length !== 1 || crashRun.voteCorrections[0].reported.slotStatus !== "CRASHED")
  failures.push(
    `a relayed slot that returned nothing must still record one CRASHED correction, got ${JSON.stringify(crashRun.voteCorrections)}`,
  );
console.log(`  driver-slot corrections ${driverRun.voteCorrections.length} (expected 0)`);

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nfabricated votes are replaced by abstentions; real verdicts pass through intact");
