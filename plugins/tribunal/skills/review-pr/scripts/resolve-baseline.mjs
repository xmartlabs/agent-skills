#!/usr/bin/env node
// resolve-baseline.mjs — deterministic incremental-re-review baseline predicate.
//
// Why this exists (2026-09-10, PAY v60 followups): the baseline predicate used
// to be Python embedded inside an agent prompt string in workflows/tribunal.js
// (the PYPRIOR heredoc) — a rule applied by hand every run, unreachable by the
// offline test tier except by slicing it out with eval/replay-embedded-bash.mjs.
// That is the exact shape that caused a misrouting incident elsewhere in this
// harness (route-review.mjs's own header tells that story). This script moves
// the predicate into real, imported, Tier-0-tested code, following the same
// pattern route-review.mjs already set.
//
// It also adds a SECOND baseline source. A PR can carry a human APPROVED or
// CHANGES_REQUESTED review with no tribunal row behind it at all — nobody ran
// this harness on it, but a person did look at a specific commit. Scoping an
// incremental re-review against THAT commit is still strictly better than
// reviewing the whole PR again, even though there are no findings to carry
// forward: it narrows the file list, nothing more.
//
// stdin (JSON): { pr, headSha, since, reviews }
//   - pr: number, used only to select this script's own metrics rows (the
//     caller does not pre-filter; this script does, from the full file).
//   - headSha: string, the PR's current head commit.
//   - since: string, optional sha prefix override (the workflow's --since).
//   - reviews: the shape `gh pr view <n> --json reviews --jq
//     '[.reviews[] | {author: .author.login, state, at: .submittedAt,
//     commit: .commit.oid}]'` returns — an array of
//     { author, state, at, commit }.
// stdout: one JSON line:
//   { found, sha, findings, source, reason, ts, reviewer, reviewState }
// Human-readable summary goes to stderr.
//
// ===== BEGIN RESOLVE-BASELINE =====

// Bots never count as a human-review baseline. Named and exported so the rule
// is greppable and testable rather than an inline regex nobody can point at.
export const BOT_LOGIN_SUFFIX_RE = /\[bot\]$/i;
export const BOT_LOGINS = new Set(["coderabbitai"]);

export function isBotAuthor(login) {
  const l = String(login || "");
  if (!l) return false;
  return BOT_LOGIN_SUFFIX_RE.test(l) || BOT_LOGINS.has(l);
}

// Only these two states are evidence someone actually reviewed the PR.
// COMMENTED is explicitly excluded — it is not evidence that anyone reviewed
// anything, per the shape GitHub uses it for (a bot's automated pass, a
// drive-by remark).
const QUALIFYING_REVIEW_STATES = new Set(["APPROVED", "CHANGES_REQUESTED"]);

function projectFinding(f) {
  return {
    file: f.file || "",
    title: f.title || "",
    severity: f.severity,
    blocking: !!f.blocking,
    survived: !!f.survived,
    posted: !!f.posted,
    // The human's correction of the panel — see workflows/tribunal.js's own
    // comment on this field. Copied verbatim, not summarised.
    operatorOverride: f.operatorOverride,
  };
}

// Source 1: the most recent POSTED review of this PR at a DIFFERENT commit.
// Behaviour-identical to the PYPRIOR heredoc it replaces. `rows` is assumed
// pre-filtered to this PR by the caller (see `invokedDirectly` below) — this
// function does no `pr` filtering of its own, so it stays a pure function of
// the rows it is handed rather than reaching for a `pr` argument it doesn't
// need.
function resolveFromTribunalRows(rows, headSha, since) {
  let best = null;
  for (const row of rows || []) {
    if ((row.schemaVersion || 0) < 3) continue;
    const sha = row.headSha || "";
    if (!sha || sha === headSha) continue;
    if (since) {
      if (!sha.startsWith(since)) continue;
    } else if (!row.posted) continue;
    if (!(row.findings || []).length) continue;
    // Last matching row in file order wins — same as the Python `best = row`
    // inside a straight-through loop with no break.
    best = row;
  }
  if (!best) return null;
  return {
    found: true,
    sha: best.headSha,
    findings: (best.findings || []).map(projectFinding),
    source: "tribunal-row",
    reason: "posted review with findings at a different commit",
    ts: best.ts || null,
    reviewer: null,
    reviewState: null,
  };
}

// Source 2: the most recent qualifying HUMAN review, tried only when source 1
// finds nothing. Carries no findings — it only narrows scope.
function resolveFromHumanReviews(reviews, headSha) {
  let best = null;
  for (const r of reviews || []) {
    if (!QUALIFYING_REVIEW_STATES.has(r.state)) continue;
    if (isBotAuthor(r.author)) continue;
    const commit = r.commit || "";
    if (!commit || commit === headSha) continue;
    // Most recent by submittedAt. ISO-8601 timestamps sort correctly as
    // strings; an entry with no timestamp never outranks one that has one.
    const at = r.at || "";
    if (!best || at > (best.at || "")) best = { ...r, commit, at };
  }
  if (!best) return null;
  return {
    found: true,
    sha: best.commit,
    findings: [],
    source: "human-review",
    reason: `${best.state} review by ${best.author}`,
    ts: null,
    reviewer: best.author,
    reviewState: best.state,
  };
}

export function resolveBaseline({ rows, reviews, headSha, since }) {
  const fromRows = resolveFromTribunalRows(rows, headSha, since);
  if (fromRows) return fromRows;
  const fromReviews = resolveFromHumanReviews(reviews, headSha);
  if (fromReviews) return fromReviews;
  return {
    found: false,
    sha: null,
    findings: [],
    source: null,
    reason:
      "no posted prior review with findings at a different commit, and no qualifying human review",
    ts: null,
    reviewer: null,
    reviewState: null,
  };
}
// ===== END RESOLVE-BASELINE =====

import { fileURLToPath } from "node:url";
import { realpathSync } from "node:fs";
// The documented chokepoint for reading metrics.jsonl (see that file's own
// header) — not a hand-rolled reader here, so this script picks up
// TRIBUNAL_DATA_DIR overrides and the "corrupt line is data loss, not a row"
// handling for free, and stays in the one place replay-rows.mjs polices.
import { allRows } from "../eval/rows.mjs";

const invokedDirectly = (() => {
  try {
    return (
      process.argv[1] &&
      realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)
    );
  } catch {
    return false;
  }
})();

// Keyed on repo AND PR number: two repos share one number space, so FE PR #N
// must never inherit BE PR #N's posted findings as its baseline. A row with no
// `repo` predates the field and still matches, as it always did.
export function readMetricsRows(pr, repo = null, rows = allRows()) {
  return rows.filter(
    (row) => row.pr === pr && (!repo || !row.repo || row.repo === repo),
  );
}

if (invokedDirectly) {
  let raw = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) raw += chunk;
  let input;
  try {
    input = JSON.parse(raw);
  } catch (e) {
    console.error(`resolve-baseline: stdin is not valid JSON: ${e.message}`);
    process.exit(1);
  }
  if (typeof input.pr !== "number" || !Number.isFinite(input.pr)) {
    console.error('resolve-baseline: input must carry a numeric "pr"');
    process.exit(1);
  }
  const repo = typeof input.repo === "string" && input.repo ? input.repo : null;
  if (!repo)
    console.error("resolve-baseline: no \"repo\" given, matching on the PR number alone");
  const rows = readMetricsRows(input.pr, repo);
  const decision = resolveBaseline({
    rows,
    reviews: Array.isArray(input.reviews) ? input.reviews : [],
    headSha: input.headSha || "",
    since: input.since || "",
  });
  process.stdout.write(JSON.stringify(decision) + "\n");
  console.error(
    decision.found
      ? `baseline: ${decision.source} at ${String(decision.sha).slice(0, 8)} — ${decision.reason} (${decision.findings.length} finding(s))`
      : `baseline: none — ${decision.reason}`,
  );
}
