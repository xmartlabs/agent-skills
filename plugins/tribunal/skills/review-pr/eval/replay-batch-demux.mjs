#!/usr/bin/env node
// replay-batch-demux.mjs — regression test for demuxClaudeBatch, the function
// that turns one batched-Claude-refuter dispatch's raw result into one vote
// per finding it was given.
//
// Batching Claude panel refuters (round-0, escalation, refine re-vote) is new
// (see CLAUDE_BATCH_SIZE / claudeRefuterBatch / demuxClaudeBatch in
// peer-review.js). Unlike the external-driver batch (a relay with no
// reasoning, whose truth is derived from a disk artifact after a barrier), a
// batched Claude call returns its verdicts directly and in-band — so the
// ONE mechanical guarantee that has to hold is: every finding in the batch
// gets exactly one vote back, and a missing/invalid/hallucinated entry
// degrades to an HONEST Claude abstention, never a silently assumed verdict.
// That is what this fixture set exercises, at zero model cost.
//
// The function is re-implemented here in the same shape as peer-review.js.
// That is deliberate duplication with a fixture that would catch drift: the
// workflow script cannot be imported (it is a Workflow-runtime module with
// top-level await and injected globals), so the alternative is no test at
// all — same rationale as replay-dedup.mjs.
//
// Usage: node eval/replay-batch-demux.mjs [--verbose]

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { FIXTURES_DIR } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(
  readFileSync(
    join(FIXTURES_DIR, "batch-demux", "cases.json"),
    "utf8",
  ),
);
const verbose = process.argv.includes("--verbose");

// --- mirrored from peer-review.js ---
function demuxClaudeBatch(findings, result) {
  const bySlot = new Map();
  if (result && Array.isArray(result.verdicts)) {
    const expected = new Set(findings.map((f) => f.slotId));
    const seen = new Set();
    for (const v of result.verdicts) {
      if (
        !v ||
        typeof v.slotId !== "number" ||
        !expected.has(v.slotId) ||
        seen.has(v.slotId)
      )
        continue; // hallucinated/out-of-range/duplicate slotId — dropped, same posture as the dedup pass's index validation
      seen.add(v.slotId);
      bySlot.set(v.slotId, v);
    }
  }
  return findings.map(
    (f) =>
      bySlot.get(f.slotId) || {
        source: "claude",
        verdict: "NOT_REFUTED",
        confidence: 0,
        slotStatus: "UNAVAILABLE",
        failureReason: "unparseable",
        reason:
          "batched refuter dispatch did not return a verdict for this slot",
      },
  );
}
// --- end mirror ---

console.log("batch-demux replay — synthetic fixtures\n");

const failures = [];

for (const { name, findings, result, expected } of fixture.cases) {
  const votes = demuxClaudeBatch(findings, result);

  if (votes.length !== findings.length) {
    failures.push(
      `[${name}] demux returned ${votes.length} votes for ${findings.length} findings — one per finding is the whole contract`,
    );
    continue;
  }

  // Positional match: demux must return one vote per finding, IN THE SAME
  // ORDER the findings were given — callers zip it against `findings`
  // positionally, exactly like a panel's own votes[] array.
  findings.forEach((f, i) => {
    const exp = expected[i];
    const got = votes[i];
    if (exp.slotId !== f.slotId)
      failures.push(`[${name}] fixture bug: expected[${i}].slotId does not match findings[${i}].slotId`);
    if (got.slotStatus !== exp.slotStatus)
      failures.push(
        `[${name}] slot ${f.slotId}: slotStatus ${got.slotStatus}, expected ${exp.slotStatus}`,
      );
    if (got.verdict !== exp.verdict)
      failures.push(
        `[${name}] slot ${f.slotId}: verdict ${got.verdict}, expected ${exp.verdict}`,
      );
    if (exp.failureReason && got.failureReason !== exp.failureReason)
      failures.push(
        `[${name}] slot ${f.slotId}: failureReason ${got.failureReason}, expected ${exp.failureReason}`,
      );
    if (exp.reason && got.reason !== exp.reason)
      failures.push(
        `[${name}] slot ${f.slotId}: reason "${got.reason}", expected "${exp.reason}" — a duplicate/hallucinated entry must not overwrite a slot's first valid vote`,
      );
    // An abstention must NEVER be reported as a live vote worth trusting —
    // this is the exact failure mode PR #1911's postmortem measured (a dead
    // slot arguing FOR the finding). Assert it structurally, not just via the
    // fixture's own expected values.
    if (got.slotStatus === "UNAVAILABLE" && got.confidence !== 0)
      failures.push(
        `[${name}] slot ${f.slotId}: an UNAVAILABLE vote must carry confidence 0, got ${got.confidence}`,
      );
  });

  if (verbose) {
    console.log(`  ${name}:`);
    for (const v of votes)
      console.log(`    slot ${v.slotId ?? "?"} -> ${v.slotStatus} ${v.verdict}`);
  }
}

console.log(`  cases checked: ${fixture.cases.length}`);

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nall batch-demux expectations hold");
