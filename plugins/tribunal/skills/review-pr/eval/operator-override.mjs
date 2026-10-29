// operator-override.mjs — reading `findings[].operatorOverride`.
//
// The one kind of ground truth in this harness that does not come from a model.
// Every other signal about whether a panel was right is another panel: a
// refuter judging a refuter, with the same blind spots. An operator override is
// a human who read the code and says the panel got it wrong, in a named
// direction. That is what T_KILL and DELTA can actually be tuned against.
//
// Written after the fact, never by the run (peer-review.js emits null) — so
// this module's real job is to refuse to trust the annotation blindly. An
// override that contradicts the row it annotates is a bookkeeping error, and
// silently scoring it would put fabricated ground truth into the corpus, which
// is worse than having none.
//
// Imported by eval/score.mjs; covered by eval/replay-operator-override.mjs.

export const OVERRIDE_DIRECTIONS = ["reinstated", "rejected", "withdrawn"];

// What each direction says about the panel, given what the panel decided:
//   reinstated — panel killed it, operator verified it is REAL   → panel false negative
//   rejected   — panel passed it, operator verified it is NOT real → panel false positive
//   withdrawn  — panel killed it, an EARLIER `reinstated` was retracted after
//                re-verification → the panel was RIGHT. Not an error in either
//                direction, so it scores as neither and is counted separately.
//
// `withdrawn` added 2026-07-29. Before it, the corpus's only such event (PR #1968:
// reinstated 2026-07-24, re-verified as non-reproducing and retracted 2026-07-27)
// was recorded as `dropped` — a value this list never accepted, so score.mjs
// reported it INVALID and never counted it. Relabelling could not fix that: the
// finding has `survived: false`, so `rejected` fails its own consistency rule,
// and `reinstated` has the right shape with the opposite meaning. The vocabulary
// was simply missing the case where an override is taken back.
export const OVERRIDE_BUCKET = {
  reinstated: "panel-false-negative",
  rejected: "panel-false-positive",
  withdrawn: "panel-confirmed-correct"
};

const isNonEmpty = (v) => typeof v === "string" && v.trim().length > 0;

// Returns {present, ok, bucket, errors, warnings}.
//
// `ok: false` means DO NOT SCORE this override — it is a data error, and it is
// reported as one rather than dropped, because an override the operator
// believes they recorded and the scorer silently ignores is the worst of the
// three outcomes.
export function readOverride(finding) {
  const o = finding && finding.operatorOverride;
  if (o === null || o === undefined)
    return { present: false, ok: false, bucket: null, errors: [], warnings: [] };
  const errors = [];
  const warnings = [];

  if (typeof o !== "object" || Array.isArray(o)) {
    return {
      present: true,
      ok: false,
      bucket: null,
      errors: ["operatorOverride is not an object"],
      warnings
    };
  }
  if (!OVERRIDE_DIRECTIONS.includes(o.direction))
    errors.push(
      `direction must be one of ${OVERRIDE_DIRECTIONS.join(" | ")}, got ${JSON.stringify(o.direction)}`
    );

  // The consistency rule, and the whole reason this is code rather than a
  // convention: an override only means something RELATIVE to what the panel
  // decided. "Reinstated" on a finding that survived reinstates nothing;
  // "rejected" on a finding the panel already killed rejects nothing. Either
  // way the pair describes no event, and counting it would invent an error the
  // panel never made.
  if (o.direction === "reinstated" && finding.survived !== false)
    errors.push(
      `direction "reinstated" requires survived === false (panel killed it); row has survived === ${JSON.stringify(finding.survived)}`
    );
  if (o.direction === "rejected" && finding.survived !== true)
    errors.push(
      `direction "rejected" requires survived === true (panel passed it); row has survived === ${JSON.stringify(finding.survived)}`
    );
  // Same shape as "reinstated" — a withdrawal only exists where the panel killed
  // something and a human first put it back. On a finding the panel PASSED there
  // was no reinstatement to retract.
  if (o.direction === "withdrawn" && finding.survived !== false)
    errors.push(
      `direction "withdrawn" requires survived === false (panel killed it, an earlier reinstatement was retracted); row has survived === ${JSON.stringify(finding.survived)}`
    );

  // An override with no stated verification is the same unrationalized verdict
  // it exists to correct — one opinion replacing another. Refuse it.
  if (!isNonEmpty(o.reason))
    errors.push("reason is required: state how the finding was verified by hand");

  if (!isNonEmpty(o.by)) warnings.push("no `by` recorded — who overrode is unattributable");
  if (!isNonEmpty(o.at)) warnings.push("no `at` recorded — the override is undated");
  // A claim about code with no tree behind it cannot be re-checked later, which
  // is most of what makes ground truth durable. Not fatal: the annotation is
  // still usable for a rate, just not for a re-read.
  if (!isNonEmpty(o.verifiedAt))
    warnings.push("no `verifiedAt` sha — the claim cannot be re-checked against a tree");

  return {
    present: true,
    ok: errors.length === 0,
    bucket: errors.length === 0 ? OVERRIDE_BUCKET[o.direction] : null,
    errors,
    warnings
  };
}

// Every override across a set of metrics rows, flattened for reporting.
// Deliberately runs over ALL canonical rows, not only the ones with a corpus
// entry: an override IS ground truth on its own, and requiring a corpus file
// first would mean the labelled cases sit unused until someone remembers to
// annotate the PR — which is exactly what happened to PR #1968.
export function collectOverrides(rows) {
  const out = [];
  for (const r of rows || []) {
    for (const f of (r && r.findings) || []) {
      const v = readOverride(f);
      if (!v.present) continue;
      out.push({
        pr: r.pr,
        headSha: r.headSha,
        ts: r.ts,
        title: f.title,
        file: f.file,
        survived: f.survived,
        refuteFraction: f.refuteFraction,
        threshold: f.threshold,
        direction: f.operatorOverride && f.operatorOverride.direction,
        ...v
      });
    }
  }
  return out;
}
