#!/usr/bin/env node
// replay-carry-forward.mjs — what happens to the PREVIOUS review's findings when
// this run only re-reads part of the PR.
//
// THE CASE THIS EXISTS FOR, and it is not hypothetical. On the first successful
// incremental run (PR #1968, 2026-07-27) the `39962991` baseline carried three
// findings. Two were on unchanged files and were carried forward correctly. The
// third — the CRITICAL `'failed'`-listener bug in `src/jobs/profiles_worker.ts` —
// was on a file that DID change, so it was re-read, did not resurface, and
// disappeared from the report entirely: not as fixed, not as open. A reader had
// to notice an ABSENCE to learn that the question the re-review was launched to
// answer went unanswered.
//
// The golden below is that exact data — the real baseline findings and the real
// findings the run reported — so the classifier is measured against what
// actually happened rather than against an invented scenario.
//
// The discriminating pair is `profiles_worker.ts`: the prior CRITICAL is about a
// `'failed'` listener stranding rows in `error`; this run's surviving finding on
// the SAME FILE is about `contactName ?? undefined` causing a redundant lookup.
// Same file, unrelated defects. A matcher too loose calls the critical "still
// open" and hides that nobody checked it; too strict and every re-raise looks
// new. Both failures are silent, which is why this is pinned.
//
// Evaluates the REAL code sliced out of workflows/peer-review.js.
//
// Usage: node eval/replay-carry-forward.mjs

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";


const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { api, violations } = loadRegion(WORKFLOW, "CARRY-FORWARD", [
  "classifyPriorFindings",
  "cfSameFinding",
  "cfTokens",
  "incrementalFallbackFor",
  "verdictFor",
  "gradePriorVerification",
  "unresolvedPriorBlockersFrom",
  "countsAsBlocker",
  "priorGrade",
]);
const {
  classifyPriorFindings,
  cfSameFinding,
  cfTokens,
  incrementalFallbackFor,
  verdictFor,
  gradePriorVerification,
  unresolvedPriorBlockersFrom,
  countsAsBlocker,
  priorGrade,
} = api;

console.log(`\nCARRY-FORWARD region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

// --- the real data ----------------------------------------------------------
// metrics.jsonl row 2026-07-24T20:39:14Z, headSha 39962991, posted: true.
const PRIOR = [
  {
    file: "src/services/v2/contactResults.ts",
    title: "Redundant prisma.account lookup re-fetches data already in scope on every update/merge job outcome",
    severity: "nit",
    blocking: false,
  },
  {
    file: "src/controllers/v2/account.controller.ts",
    title:
      "Enqueue-failure + partner redelivery creates a duplicate ContactResults row that is stranded in 'error' forever",
    severity: "important",
    blocking: true,
  },
  {
    file: "src/jobs/profiles_worker.ts",
    title: "Worker's 'failed' listener permanently flips contactResults row to error on any retryable failure",
    severity: "critical",
    blocking: true,
  },
];
// The scope the 2026-07-27 run actually computed.
const SCOPE = {
  changed: ["src/controllers/v2/account.controller.test.ts", "src/jobs/profiles_worker.ts"],
  added: ["src/jobs/retryState.test.ts", "src/jobs/retryState.ts"],
  unchanged: [
    "src/controllers/v2/account.controller.ts",
    "src/controllers/v2/contactResults.ts",
    "src/services/v2/contactResults.ts",
    "src/jobs/queue.ts",
    "src/routes/v2/contactResults.routes.ts",
  ],
  gone: [],
};
// The one finding that run reported and survived.
const CURRENT = [
  {
    file: "src/jobs/profiles_worker.ts",
    title: "outcome.contactName ?? undefined collapses an intentional null into 'not provided', triggering an unintended DB lookup",
    severity: "nit",
  },
];

console.log(`\n--- the golden: PR #1968, 39962991 -> 83b6ecff ---`);
const out = classifyPriorFindings(SCOPE, PRIOR, CURRENT);
check("2 findings carried forward (unchanged files)", out.carried.length === 2, `${out.carried.length}`);
check(
  "the carried ones are the contactResults nit and the account.controller blocker",
  out.carried.map((f) => f.file).sort().join(",") ===
    "src/controllers/v2/account.controller.ts,src/services/v2/contactResults.ts",
  JSON.stringify(out.carried.map((f) => f.file)),
);
check("nothing moot", out.moot.length === 0, `${out.moot.length}`);
// THE assertion. Before this region existed the critical was in no list at all.
check(
  "the profiles_worker CRITICAL is DID-NOT-RESURFACE, not silently dropped",
  out.notResurfaced.length === 1 && out.notResurfaced[0].severity === "critical",
  JSON.stringify(out.notResurfaced.map((f) => f.title)),
);
check(
  "it is NOT reported as still-open — this run's finding on that file is a different defect",
  out.stillOpen.length === 0,
  JSON.stringify(out.stillOpen.map((f) => f.title)),
);
check(
  "every prior finding lands in exactly one bucket",
  out.carried.length + out.moot.length + out.stillOpen.length + out.notResurfaced.length === PRIOR.length,
);

console.log(`\n--- the matcher itself ---`);
const priorCritical = PRIOR[2];
const currentNit = CURRENT[0];
check(
  "same file + unrelated titles do NOT match",
  cfSameFinding(priorCritical, currentNit) === false,
);
check(
  "a genuine re-raise DOES match (same defect, reworded)",
  cfSameFinding(priorCritical, {
    file: "src/jobs/profiles_worker.ts",
    title: "The 'failed' listener still flips the contactResults row to error on a retryable failure",
  }) === true,
);
check(
  "different files never match, however similar the title",
  cfSameFinding(priorCritical, { ...priorCritical, file: "src/jobs/other_worker.ts" }) === false,
);
check(
  "close line numbers match even with unrelated titles (same defect, renamed)",
  cfSameFinding(
    { file: "a.ts", line: 100, title: "completely unrelated words here" },
    { file: "a.ts", line: 120, title: "nothing in common at all" },
  ) === true,
);

console.log(`\n--- degenerate inputs must not lose a finding ---`);
const noScope = classifyPriorFindings(null, PRIOR, CURRENT);
check("no scope -> nothing classified (a full review carries nothing)", noScope.carried.length === 0);
const noFile = classifyPriorFindings(SCOPE, [{ title: "no file recorded", severity: "important" }], []);
check("a finding with no file is CARRIED, never dropped", noFile.carried.length === 1);
const unknownFile = classifyPriorFindings(SCOPE, [{ file: "src/never/seen.ts", title: "x" }], []);
check("a file in no bucket is CARRIED, never dropped", unknownFile.carried.length === 1);
const noCurrent = classifyPriorFindings(SCOPE, PRIOR, []);
check(
  "a run that reports nothing still accounts for all three priors",
  noCurrent.carried.length + noCurrent.notResurfaced.length === 3,
);

console.log(`\n--- "did the author fix it?" — the claim needs code behind it ---`);
// A fix claim retires a real defect on a model's say-so. It is the most
// expensive output this harness can produce, so it is the one claim that must
// carry a quote. Everything unsupported degrades to CANNOT_TELL, which counts
// as unresolved everywhere.
const realFix = gradePriorVerification({
  verdict: "FIXED",
  // The actual guard the author added, which a person verified by hand.
  quote: "if (job && isLastRetry(job)) {",
  reason: "the flip is now gated on the final attempt",
});
check("FIXED with a real quote survives", realFix.verdict === "FIXED", JSON.stringify(realFix));
const bareFix = gradePriorVerification({ verdict: "FIXED", reason: "looks handled" });
check(
  "FIXED with NO quote is downgraded to CANNOT_TELL",
  bareFix.verdict === "CANNOT_TELL",
  JSON.stringify(bareFix),
);
check(
  "...and the downgrade says why, instead of vanishing",
  /no quote/.test(bareFix.reason),
  bareFix.reason,
);
check(
  "a token-length 'quote' does not buy a fix claim",
  gradePriorVerification({ verdict: "FIXED", quote: "ok", reason: "x" }).verdict === "CANNOT_TELL",
);
check(
  "STILL_OPEN needs a quote too — an unsupported accusation is not a finding",
  gradePriorVerification({ verdict: "STILL_OPEN", reason: "feels wrong" }).verdict === "CANNOT_TELL",
);
check(
  "a dead dispatch is CANNOT_TELL, never a fix",
  gradePriorVerification(null).verdict === "CANNOT_TELL",
);
check(
  "an invented verdict is CANNOT_TELL, never a fix",
  gradePriorVerification({ verdict: "PROBABLY_FINE", quote: "some real looking code here" }).verdict ===
    "CANNOT_TELL",
);

console.log(`\n--- a verified fix clears the blocker; anything else does not ---`);
const blockingCritical = { file: "src/jobs/profiles_worker.ts", severity: "critical", blocking: true };
check(
  "verified FIXED stops counting against the PR",
  unresolvedPriorBlockersFrom([], [{ ...blockingCritical, verification: realFix }]).length === 0,
);
check(
  "CANNOT_TELL still counts — unknown is not resolved",
  unresolvedPriorBlockersFrom([], [{ ...blockingCritical, verification: bareFix }]).length === 1,
);
check(
  "a missing verification counts, it does not default to fixed",
  unresolvedPriorBlockersFrom([], [{ ...blockingCritical }]).length === 1,
);
check(
  "carried-forward blockers always count — nobody looked at them at all",
  unresolvedPriorBlockersFrom([{ severity: "important", blocking: true }], []).length === 1,
);
// The end-to-end shape of the real PR #1968 case, with the verification the
// harness should now produce.
check(
  "PR #1968 after a verified fix: REQUEST CHANGES becomes COMMENT, not APPROVE",
  verdictFor(
    [],
    unresolvedPriorBlockersFrom(
      [{ file: "src/controllers/v2/account.controller.ts", severity: "important", blocking: true }],
      [{ ...blockingCritical, verification: realFix }],
    ).map((f) => f.severity),
  ) === "COMMENT",
);

console.log(`\n--- the human's correction outranks the panel's flag ---`);
// The real record from the 2026-07-24 review of PR #1968: the panel KILLED this
// finding, the operator traced it in source and reinstated it, and the row kept
// the panel's flags with the correction stored alongside. Carry-forward read
// only the flags, so a reinstated blocker came back as non-blocking and the row
// recorded APPROVE on a PR with it open.
const REINSTATED = {
  file: "src/controllers/v2/account.controller.ts",
  severity: "important",
  blocking: false,
  survived: false,
  operatorOverride: {
    direction: "reinstated",
    by: "alice",
    verifiedAt: "d878683309f5702689e754ce007590418986db7a",
    reason: "Traced in source at this sha",
  },
};
check(
  "an operator-reinstated finding blocks, despite blocking:false from the panel",
  countsAsBlocker(REINSTATED) === true,
);
check(
  "...and grades as hard as a critical, not as its panel-left severity",
  priorGrade(REINSTATED) === "critical",
  priorGrade(REINSTATED),
);
check(
  "an operator-DROPPED finding stops blocking, even at critical severity",
  countsAsBlocker({
    severity: "critical",
    blocking: true,
    operatorOverride: { direction: "dropped", by: "alice" },
  }) === false,
);
// The 2026-07-29 vocabulary fix. `dropped` above was never a valid direction —
// eval/operator-override.mjs refused it and score.mjs reported PR #1968's row as
// INVALID, so the corpus ran on two ground-truth labels while looking like it had
// three. `withdrawn` is the real name for that event (panel killed it, an earlier
// reinstatement was retracted, the panel was right). Both must stop the finding
// blocking: the legacy spelling because an unmigrated row must not silently start
// blocking again, the new one because it is what gets written from now on.
check(
  "an operator-WITHDRAWN finding stops blocking, even at critical severity",
  countsAsBlocker({
    severity: "critical",
    blocking: true,
    operatorOverride: { direction: "withdrawn", by: "alice" },
  }) === false,
);
check(
  "an operator-REJECTED finding stops blocking too",
  countsAsBlocker({
    severity: "critical",
    blocking: true,
    operatorOverride: { direction: "rejected", by: "alice" },
  }) === false,
);
check(
  "with no override, the panel's flags still decide",
  countsAsBlocker({ severity: "important", blocking: true }) === true &&
    countsAsBlocker({ severity: "nit", blocking: false }) === false,
);
check(
  "a critical still blocks without any override",
  countsAsBlocker({ severity: "critical", blocking: false }) === true,
);
// End to end, the exact #1968 shape: nothing new, the reinstated blocker
// carried forward, the critical verified FIXED.
{
  const unresolved = unresolvedPriorBlockersFrom(
    [REINSTATED, { file: "src/services/v2/contactResults.ts", severity: "nit", blocking: false }],
    [{ file: "src/jobs/profiles_worker.ts", severity: "critical", blocking: true, verification: realFix }],
  );
  check(
    "only the reinstated finding is left counting",
    unresolved.length === 1 && unresolved[0].file === REINSTATED.file,
    JSON.stringify(unresolved.map((f) => f.file)),
  );
  check(
    "the real #1968 row grades REQUEST CHANGES, not APPROVE",
    verdictFor([], unresolved.map(priorGrade)) === "REQUEST CHANGES",
    verdictFor([], unresolved.map(priorGrade)),
  );
}

console.log(`\n--- the verdict cannot improve just because the run looked less ---`);
// The real regression: on 2026-07-27 the row recorded APPROVE for PR #1968 while
// two blocking items stood, because the verdict was derived from THIS run's
// findings and this run raised none. The report said REQUEST CHANGES; the row is
// what --metrics and the corpus scorer read.
check(
  "0 new findings + an unresolved prior CRITICAL is REQUEST CHANGES, not APPROVE",
  verdictFor([], ["critical"]) === "REQUEST CHANGES",
  verdictFor([], ["critical"]),
);
check(
  "0 new findings + an unresolved prior important is COMMENT, not APPROVE",
  verdictFor([], ["important"]) === "COMMENT",
  verdictFor([], ["important"]),
);
check("nothing anywhere is APPROVE", verdictFor([], []) === "APPROVE");
{
  const demoted = (severity) => ({ severity, isBehaviorChanging: true, touchesHighRiskSurface: true, verifiedByGateNote: "Downgraded from Blocking: verifiedBy is missing" });
  check("a gate-demoted important does not block", verdictFor([demoted("important")], []) === "COMMENT", verdictFor([demoted("important")], []));
  check("a gate-demoted critical does not block either, and does not approve", verdictFor([demoted("critical")], []) === "COMMENT", verdictFor([demoted("critical")], []));
  check(
    "an anchored blocker beside a demoted one still blocks",
    verdictFor([demoted("important"), { severity: "important", isBehaviorChanging: true }], []) === "REQUEST CHANGES",
  );
}
check(
  "a new critical still dominates",
  verdictFor([{ severity: "critical" }], []) === "REQUEST CHANGES",
);
check("undefined inputs do not throw or silently approve", verdictFor(undefined, ["critical"]) === "REQUEST CHANGES");

console.log(`\n--- code-health rule, not a severity tally (gap 1) ---`);
// A surviving `important` finding that is behaviour-changing OR on a high-risk
// surface must block, whatever its bare severity says — the false negative
// the old severity tally had (PR-shape: an important, behaviour-changing
// finding on a payments path survived a full panel and still graded COMMENT).
check(
  "important + isBehaviorChanging escalates to REQUEST CHANGES",
  verdictFor([{ severity: "important", isBehaviorChanging: true }], []) === "REQUEST CHANGES",
);
check(
  "important + touchesHighRiskSurface escalates to REQUEST CHANGES",
  verdictFor([{ severity: "important", touchesHighRiskSurface: true }], []) === "REQUEST CHANGES",
);
check(
  "important alone (neither flag set) is still just COMMENT",
  verdictFor([{ severity: "important" }], []) === "COMMENT",
);
check(
  "nits alone must never withhold approval",
  verdictFor([{ severity: "nit" }], []) === "APPROVE",
);
check(
  "one nit + one escalated important still blocks",
  verdictFor(
    [{ severity: "nit" }, { severity: "important", isBehaviorChanging: true }],
    [],
  ) === "REQUEST CHANGES",
);
// The #1968 shape end to end: nothing new, one carried important-blocker, one
// not-resurfaced critical.
{
  const out2 = classifyPriorFindings(SCOPE, PRIOR, []);
  const unresolved = [...out2.carried, ...out2.notResurfaced].filter(
    (f) => f.blocking || f.severity === "critical",
  );
  check(
    "the real #1968 shape yields REQUEST CHANGES",
    verdictFor([], unresolved.map((f) => f.severity)) === "REQUEST CHANGES",
    JSON.stringify(unresolved.map((f) => f.severity)),
  );
}

console.log(`\n--- fail open, but fail loud ---`);
// "Full because nothing to compare against" and "full because something broke"
// must not be the same row. Both of 2026-07-27's plumbing bugs produced the
// second while looking like the first, and both were caught by a human noticing
// the lens count rather than by anything in the data.
check(
  "no baseline -> null (an ordinary first review is not a degradation)",
  incrementalFallbackFor(null, false, 0, "no baseline") === null,
);
check(
  "baseline + usable scope -> null",
  incrementalFallbackFor("39962991", true, 0, null) === null,
);
const fb = incrementalFallbackFor("91c5e0458392", false, 0, "a diff was empty");
check(
  "baseline + failed scope -> expected:true with the baseline and the reason",
  fb?.expected === true && fb.baseline === "91c5e0458392" && fb.reason === "a diff was empty",
  JSON.stringify(fb),
);
check(
  "unknown in-scope paths are named as the reason",
  /3 in-scope path\(s\)/.test(incrementalFallbackFor("abc", false, 3, "ignored")?.reason || ""),
);
check(
  "a failure with no reason still says so rather than reading as fine",
  incrementalFallbackFor("abc", false, 0, null)?.reason === "scope unusable, no reason given",
);

// --- anti-drift -------------------------------------------------------------
// The region carries its own copy of the dedup token rule because a region is
// sliced in isolation. Pinned here on the real titles so the two cannot diverge
// about what "the same finding" means.
console.log(`\n--- anti-drift: the region's tokens match the shipping dedup rule ---`);
const src = readFileSync(WORKFLOW, "utf8");
const stopMatch = /const TITLE_STOP = new Set\(\n\s*"([^"]+)"/.exec(src);
const overlapMatch = /const TITLE_OVERLAP_MIN = ([\d.]+);/.exec(src);
const windowMatch = /const MERGE_LINE_WINDOW = (\d+);/.exec(src);
check("found the shipping constants", !!stopMatch && !!overlapMatch && !!windowMatch);
if (stopMatch && overlapMatch && windowMatch) {
  const cfOverlap = /const CF_TITLE_OVERLAP_MIN = ([\d.]+);/.exec(src);
  const cfWindow = /const CF_LINE_WINDOW = (\d+);/.exec(src);
  const cfStop = /const CF_STOP = new Set\(\n\s*"([^"]+)"/.exec(src);
  check("overlap threshold agrees", cfOverlap?.[1] === overlapMatch[1], `${cfOverlap?.[1]} vs ${overlapMatch[1]}`);
  check("line window agrees", cfWindow?.[1] === windowMatch[1], `${cfWindow?.[1]} vs ${windowMatch[1]}`);
  check("stop-word list agrees", cfStop?.[1] === stopMatch[1]);
  // Behavioural, not textual: same tokens out for the titles that matter.
  for (const t of [...PRIOR.map((p) => p.title), CURRENT[0].title]) {
    const mine = [...cfTokens(t)].sort().join(",");
    const theirs = t
      .toLowerCase()
      .split(/[^a-z0-9_]+/)
      .filter((x) => x.length > 2 && !new Set(stopMatch[1].split(" ")).has(x))
      .sort()
      .join(",");
    check(`same tokens for "${t.slice(0, 42)}…"`, mine === theirs);
  }
}

console.log(`\n--- import-adjacency heuristic (2026-08-18) ---`);
// classifyPriorFindings' 4th param. A finding on an unchanged file normally
// carries forward blind; when the caller (context-bookkeeping's bash) finds
// that file imports or is imported by a file this PR DID change, it should
// escalate into notResurfaced instead, tagged escalatedByImportHeuristic, so
// it gets a real finding-verifier dispatch rather than an unchecked assertion.
{
  const scope = {
    changed: ["src/services/v2/accounts.ts"],
    added: [],
    unchanged: ["src/controllers/v2/auth.controller.ts"],
    gone: [],
  };
  const prior = [
    {
      file: "src/controllers/v2/auth.controller.ts",
      title: "Some finding on the controller",
      severity: "important",
      blocking: true,
    },
  ];
  const adjacency = {
    "src/controllers/v2/auth.controller.ts": [
      { file: "src/services/v2/accounts.ts", matchConfidence: "path" },
    ],
  };
  const escalated = classifyPriorFindings(scope, prior, [], adjacency);
  check(
    "a related import relationship escalates the finding out of carried-forward",
    escalated.carried.length === 0 && escalated.notResurfaced.length === 1,
    JSON.stringify({ carried: escalated.carried.length, notResurfaced: escalated.notResurfaced.length }),
  );
  check(
    "the escalated finding is tagged, with the related file recorded",
    escalated.notResurfaced[0]?.escalatedByImportHeuristic === true &&
      (escalated.notResurfaced[0]?.relatedFiles || []).includes("src/services/v2/accounts.ts"),
    JSON.stringify(escalated.notResurfaced[0]),
  );

  const notEscalated = classifyPriorFindings(scope, prior, [], {});
  check(
    "no adjacency entry -> unchanged carry-forward behavior (backward compatible)",
    notEscalated.carried.length === 1 && notEscalated.notResurfaced.length === 0,
    JSON.stringify({ carried: notEscalated.carried.length, notResurfaced: notEscalated.notResurfaced.length }),
  );

  const noAdjacencyArg = classifyPriorFindings(scope, prior, []);
  check(
    "omitting the 4th arg entirely does not throw and preserves old behavior",
    noAdjacencyArg.carried.length === 1,
    JSON.stringify(noAdjacencyArg.carried.length),
  );
}

console.log(`\n--- PR #2012: auth.controller.ts / accounts.ts (the case that motivated this) ---`);
// Reconstructed from the reported shape, not read from the real PR diff: a
// finding cited auth.controller.ts, which this PR left unchanged; the actual
// fix landed in accounts.ts, a file the controller calls into, which this PR
// DID change. Before the import-adjacency heuristic, classifyPriorFindings
// had no way to cross the two — the finding stayed CARRIED FORWARD as an open
// blocker regardless of the fix. Pinned here as a permanent regression golden,
// the same way PR #1968 is pinned above for the sibling not-resurfaced bug.
{
  const scope = {
    changed: ["src/services/v2/accounts.ts"],
    added: [],
    unchanged: [
      "src/controllers/v2/auth.controller.ts",
      "src/controllers/v2/account.controller.ts",
    ],
    gone: [],
  };
  const prior = [
    {
      file: "src/controllers/v2/auth.controller.ts",
      title: "Account-level skip pushes success:false rows for orders never attempted",
      severity: "important",
      blocking: true,
    },
  ];
  const adjacency = {
    "src/controllers/v2/auth.controller.ts": [
      { file: "src/services/v2/accounts.ts", matchConfidence: "path" },
    ],
  };
  const out2012 = classifyPriorFindings(scope, prior, [], adjacency);
  check(
    "PR #2012 shape: the controller finding escalates instead of carrying forward blind",
    out2012.carried.length === 0 &&
      out2012.notResurfaced.length === 1 &&
      out2012.notResurfaced[0].file === "src/controllers/v2/auth.controller.ts",
    JSON.stringify(out2012),
  );
  check(
    "without the heuristic (no adjacency), the same shape reproduces the original bug",
    classifyPriorFindings(scope, prior, []).carried.length === 1,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall carry-forward expectations hold");
