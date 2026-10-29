#!/usr/bin/env node
// annotation-match.mjs — the ONE definition of "does this finding match this
// annotation". Lived inline in score.mjs until eval/worksheet.mjs needed the same
// predicate to decide which findings are still unjudged.
//
// Extracted rather than copied, for the reason replay-rows.mjs already enforces
// one level up: two definitions of the same decision is the problem, not the fix.
// A worksheet that disagrees with the scorer about what counts as unlabelled
// would quietly ask a human to judge something already judged, or skip something
// nobody has.
//
// The threshold is deliberately conservative. A near-miss lands in the queue for a
// person rather than being auto-classified — the corpus grows by resolving
// unknowns, never by loosening this.

export const OVERLAP_THRESHOLD = 0.3;

export const STOP = new Set(
  "the a an of in on to for is are and or not with does no same this that".split(" "),
);
export const tokens = (s) =>
  new Set(
    String(s || "")
      .toLowerCase()
      .split(/[^a-z0-9_]+/)
      .filter((t) => t.length > 2 && !STOP.has(t)),
  );
export const overlap = (a, b) => {
  const ta = tokens(a), tb = tokens(b);
  let n = 0;
  for (const t of ta) if (tb.has(t)) n++;
  return n / Math.max(1, Math.min(ta.size, tb.size));
};

const sameFile = (a, b) => a.endsWith(b) || b.endsWith(a);

// `files` lists alternate anchors for a defect that spans files (a cause in one,
// a consumer in another): lenses file the same claim at either end.
export const matches = (finding, annotation) => {
  const anchors = [annotation.file, ...(annotation.files || [])].filter(Boolean);
  if (anchors.length && finding.file && !anchors.some((a) => sameFile(finding.file, a)))
    return false;
  const text = `${finding.title} ${finding.evidence || ""}`;
  const ann = `${annotation.id} ${annotation.description || ""}`;
  return overlap(text, ann) >= OVERLAP_THRESHOLD;
};

