#!/usr/bin/env node
// route-review.mjs — deterministic Workflow-vs-in-context routing decision.
//
// Why this exists (2026-09-09, PR #2025): the Dispatch decision in SKILL.md
// (lines 88-114) was PROSE ONLY, applied by hand each run. On PR #2025 (7
// changed files, 2 of them `*.test.ts`/`package.json`) the "high-risk" regex
// matched a path named `invoice.export-batching.test.ts` exactly as it would a
// real money-path file, and the raw file count was used for the risk rule's
// scope too — so the PR routed to the Workflow. Counting only PRODUCT files
// (excluding tests, fixtures, lockfiles, docs, etc. — see
// PRODUCT_FILE_EXCLUSIONS) drops scope to 5, which is <= HIGH_RISK_SCOPE_MAX,
// and the PR should route in-context: measured 3x cheaper and 2x faster on the
// identical pinned commit (`specs/2026-07-29-small-pr-routing.md`). Same
// remedy already applied elsewhere in this harness (in-context-plan.mjs,
// count-in-context-agents.mjs, persist-review-record.mjs): the decision moves
// into code, and deviation stays possible but must be declared.
//
// Content risk (2026-09-10, v61): a mass delete or update is high-risk and
// lives in the diff, never in a path, so until now SKILL.md merely INSTRUCTED
// the orchestrator to override by hand — the same instructed-not-enforced gap
// this script was written to close for the other two rules. `--diff <path>`
// now feeds the patch in and a mass op in an ADDED product line counts as
// high-risk surface, under the same `scope > HIGH_RISK_SCOPE_MAX` gate.
//
// That gate was measured, not chosen: `specs/2026-09-10-content-risk-gate.md`
// ran three candidates over 35 merged BE PRs. Escalating on any content hit
// regardless of scope routes PR #2025 back to the Workflow — the exact PR this
// script exists to keep in-context — because it legitimately adds
// `invoiceQueueDumps.deleteMany`. A churn-gated variant only separates #2025
// from #1992 at a threshold inside a 166-line gap between them, which is
// fitting a rule to two points. So the hit joins the existing risk gate rather
// than getting one of its own, and every hit is reported either way.
//
// stdin (JSON): either the verbatim shape of `gh pr view <n> --json files`
//   ({ "files": [{path, additions, deletions, changeType}, ...] }), or
//   { "files": ["path", ...] } for a re-review's in-scope list (strings only).
// argv: `--diff <path>` to a unified diff of the same PR. Optional.
//   `--profile <file>`, the profile load-profile.mjs --save wrote. Required:
//   which paths are high-risk is the repo's own list.
// stdout: one JSON line:
//   { route, fileCount, scopeCount, highRisk, pathRisk, highRiskFiles,
//     contentRisk, contentRiskHits, excluded, churn, reason }
//   `contentRisk` is null when no diff was supplied or the diff was unusable,
//   never false: "did not look" and "looked and found nothing" are different
//   answers and only one of them is evidence.
// Human-readable summary goes to stderr.
//
// ===== BEGIN ROUTE-REVIEW =====

export const WORKFLOW_MIN_FILES = 10;
export const HIGH_RISK_SCOPE_MAX = 6;

// Files that stop counting toward "scope" for the high-risk rule, because they
// are not product surface — a reviewer skipping them loses nothing risky.
// Excluded files are NOT skipped by the review itself, only by this count.
//
// `src/scripts/**` and `scripts/**` are DELIBERATELY NOT excluded: in this
// org they are operational scripts that run against production data (PR
// #1992 was an entirely-under-`scripts/<partner>/` change with mass deletes
// that correctly got a full review). Omitting them from this list looks like
// an oversight; it is not.
export const PRODUCT_FILE_EXCLUSIONS = [
  { reason: "test", pattern: /\.(test|spec)\.[^/]+$/i },
  { reason: "test", pattern: /(^|\/)__tests__\// },
  { reason: "test", pattern: /^src\/tests\// },
  { reason: "fixture", pattern: /(^|\/)__fixtures__\// },
  { reason: "snapshot", pattern: /(^|\/)__snapshots__\/|\.snap$/ },
  { reason: "manifest", pattern: /(^|\/)package\.json$/ },
  { reason: "lockfile", pattern: /(^|\/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml)$/ },
  { reason: "docs", pattern: /\.md$/i },
  { reason: "docs", pattern: /^docs\// },
  { reason: "ci", pattern: /^\.github\// },
];

// A high-risk path is one that forces an in-context lens: the same rules
// (`input.riskRules`, from the profile through load-profile.mjs), so the
// routing gate and the lens plan cannot disagree about what is risky.

export function productFiles(paths) {
  const product = [];
  const excluded = [];
  for (const path of paths) {
    const hit = PRODUCT_FILE_EXCLUSIONS.find((e) => e.pattern.test(path));
    if (hit) excluded.push({ path, reason: hit.reason });
    else product.push(path);
  }
  return { product, excluded };
}

// The high-risk surface that has no path: a statement that can touch many rows
// at once.
export const MASS_OP_RE =
  /deleteMany|updateMany|\bDELETE\s+FROM\b|\bTRUNCATE\b|\bDROP\s+TABLE\b|\$executeRaw/i;

// A line whose content is only a comment. Line-based on purpose, because the
// scan is line-based too and anything cleverer would claim a precision it does
// not have. It catches `//`, `/*`, a `*` continuation, SQL `--` and `#`, and
// misses a mass op sitting after code on the same line — the safe way to miss.
// Measured worth on the corpus: it drops 8 of 33 hits on PR #2081 and takes PR
// #2049 from 2 hits to 0, both of which were prose ABOUT a deleteMany.
export const COMMENT_ONLY_RE = /^\s*(\/\/|\/\*|\*|--|#)/;

// A `gh pr diff` that came back truncated still parses and still looks like a
// diff. Ported from `diffIsUsable` in workflows/tribunal.js, which exists
// because PR #1911 reviewed 2KB of a truncation banner as if it were the PR.
// Here the cost of missing it is quieter and worse: an unusable diff yields no
// hits, which is indistinguishable from a clean PR unless it is called out.
export function diffUsable(diffText, churn) {
  if (typeof diffText !== "string" || diffText.length === 0) return false;
  if (diffText.length < 200) return false;
  if (/^\s*(Full diff available at:|Output too large)/m.test(diffText)) return false;
  if (diffText.includes("Preview (first ")) return false;
  if (!/^diff --git /m.test(diffText)) return false;
  const c = Number(churn) || 0;
  if (c >= 50 && diffText.length < c * 8) return false;
  return true;
}

// Attribute every ADDED line to the file it lands in, and scan only the lines
// in product files. Added lines only: a `deleteMany` sitting in diff CONTEXT
// was already there before this PR and is not what this PR is doing.
export function massOpHits(diffText, productPaths) {
  if (typeof diffText !== "string" || !diffText) return [];
  const inScope = new Set(productPaths || []);
  const hits = [];
  let file = null;
  let newLine = 0;
  for (const raw of diffText.split("\n")) {
    if (raw.startsWith("diff --git ")) {
      const m = raw.match(/^diff --git a\/(.+) b\/(.+)$/);
      file = m ? m[2] : null;
      newLine = 0;
      continue;
    }
    // `+++ b/path` overrides the header above, whose a/b split breaks on any
    // path containing a space.
    if (raw.startsWith("+++ ")) {
      const p = raw.slice(4).trim();
      file = p === "/dev/null" ? null : p.replace(/^b\//, "");
      continue;
    }
    if (raw.startsWith("--- ")) continue;
    if (raw.startsWith("@@")) {
      const m = raw.match(/^@@ -\d+(?:,\d+)? \+(\d+)/);
      newLine = m ? Number(m[1]) : 0;
      continue;
    }
    if (!file) continue;
    if (raw.startsWith("+")) {
      const body = raw.slice(1);
      if (inScope.has(file) && MASS_OP_RE.test(body)) {
        hits.push({
          file,
          line: newLine,
          comment: COMMENT_ONLY_RE.test(body),
          snippet: body.trim().slice(0, 120),
        });
      }
      newLine++;
    } else if (raw.startsWith(" ") || raw === "") {
      newLine++;
    }
    // A '-' line consumes no new-file line number.
  }
  return hits;
}

export function routeReview(input) {
  const rawFiles = Array.isArray(input?.files) ? input.files.filter(Boolean) : [];
  const normalized = rawFiles.map((f) =>
    typeof f === "string" ? { path: f, additions: 0, deletions: 0 } : f,
  );
  const paths = normalized.map((f) => f.path).filter(Boolean);

  const fileCount = paths.length;
  const { product, excluded } = productFiles(paths);
  const scopeCount = product.length;
  if (!Array.isArray(input?.riskRules))
    throw new Error("route-review: input.riskRules is required (the profile's risk paths, from load-profile.mjs)");
  const highRiskFiles = product.filter((p) => input.riskRules.some((r) => r.re.test(p)));
  const pathRisk = highRiskFiles.length > 0;

  // Asked of the RAW entries, not the normalized ones: normalizing a string
  // path fills in additions/deletions as 0, which would satisfy this test and
  // leave the top-level fallback below unreachable for exactly the input the
  // fallback exists for — a re-review's in-scope list, which is strings.
  const hasPerFileChurn = rawFiles.some(
    (f) =>
      typeof f === "object" &&
      (typeof f.additions === "number" || typeof f.deletions === "number"),
  );
  const churn = hasPerFileChurn
    ? normalized.reduce(
        (sum, f) => sum + (Number(f.additions) || 0) + (Number(f.deletions) || 0),
        0,
      )
    : (Number(input?.additions) || 0) + (Number(input?.deletions) || 0);

  // null, not false, whenever the diff was absent or unusable. A caller that
  // treats "no hits" and "never looked" as the same answer will report a
  // truncated fetch as a clean PR.
  const diffGiven = typeof input?.diff === "string" && input.diff.length > 0;
  const looked = diffGiven && diffUsable(input.diff, churn);
  const contentRiskHits = looked
    ? massOpHits(input.diff, product).filter((h) => !h.comment)
    : [];
  const contentRisk = looked ? contentRiskHits.length > 0 : null;

  // A mass op is high-risk surface that happens to have no path, so it joins
  // the path match rather than getting a gate of its own. See the header for
  // the corpus run that rejected the alternatives.
  const highRisk = pathRisk || contentRisk === true;
  const riskWhy = [
    ...highRiskFiles,
    ...contentRiskHits.map((h) => `${h.file}:${h.line}`),
  ].join(", ");

  // The size rule deliberately uses the RAW count, tests included — its
  // rationale is orchestrator context pressure, which does not care whether a
  // file is a test. Only the high-risk rule counts product files.
  let route;
  let reason;
  if (fileCount === 0) {
    route = "in-context";
    reason = "no files";
  } else if (fileCount >= WORKFLOW_MIN_FILES) {
    route = "workflow";
    reason = `${fileCount} files >= ${WORKFLOW_MIN_FILES}`;
  } else if (highRisk && scopeCount > HIGH_RISK_SCOPE_MAX) {
    route = "workflow";
    reason = `${fileCount} files < ${WORKFLOW_MIN_FILES}; high-risk (${riskWhy}) and scope ${scopeCount} > ${HIGH_RISK_SCOPE_MAX} product files`;
  } else if (highRisk) {
    route = "in-context";
    reason = `${fileCount} files < ${WORKFLOW_MIN_FILES}; high-risk (${riskWhy}) but scope ${scopeCount} <= ${HIGH_RISK_SCOPE_MAX} product files`;
  } else {
    route = "in-context";
    reason = `${fileCount} files < ${WORKFLOW_MIN_FILES}; no high-risk surface (scope ${scopeCount} product files)`;
  }
  // Say "content risk unknown" only where not knowing it could have changed
  // the answer: an in-context route that a content hit would have flipped,
  // which is scope above the threshold with no risky path. Everywhere else the
  // route is already settled by size or by scope, so the caveat would be noise
  // implying a gap that is not there. `contentRisk: null` is in the JSON
  // regardless, for a caller that wants to know whether anyone looked.
  const contentCouldHaveFlipped =
    route === "in-context" &&
    fileCount > 0 &&
    fileCount < WORKFLOW_MIN_FILES &&
    scopeCount > HIGH_RISK_SCOPE_MAX;
  if (contentRisk === null && contentCouldHaveFlipped) {
    reason += diffGiven
      ? "; diff unusable, so content risk is UNKNOWN and could have made this a workflow"
      : "; no diff supplied, so content risk is UNKNOWN and could have made this a workflow";
  }

  return {
    route,
    fileCount,
    scopeCount,
    highRisk,
    pathRisk,
    highRiskFiles,
    contentRisk,
    contentRiskHits,
    excluded,
    churn,
    reason,
  };
}
// ===== END ROUTE-REVIEW =====

import { fileURLToPath } from "node:url";
import { realpathSync, readFileSync } from "node:fs";
import { inContextRiskPathRules, readProfileFile } from "./load-profile.mjs";

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

if (invokedDirectly) {
  let raw = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) raw += chunk;
  let input;
  try {
    input = JSON.parse(raw);
  } catch (e) {
    console.error(`route-review: stdin is not valid JSON: ${e.message}`);
    process.exit(1);
  }
  if (!Array.isArray(input.files)) {
    console.error('route-review: input must carry a "files" array');
    process.exit(1);
  }

  // The only I/O the diff needs, kept out of the pure region above so Tier 0
  // can still reach the decision. Reading the patch here rather than piping it
  // through stdin also keeps the JSON on stdin small.
  const diffFlag = process.argv.indexOf("--diff");
  if (diffFlag !== -1) {
    const diffPath = process.argv[diffFlag + 1];
    if (!diffPath) {
      console.error("route-review: --diff needs a path");
      process.exit(1);
    }
    try {
      input.diff = readFileSync(diffPath, "utf8");
    } catch (e) {
      // Loud, not silent. The caller asked for a content scan; degrading to
      // "no content risk" here would report an unread diff as a clean one, and
      // nothing has been spent yet, so failing costs only a corrected command.
      console.error(`route-review: cannot read --diff ${diffPath}: ${e.message}`);
      process.exit(1);
    }
  }

  const profileFlag = process.argv.indexOf("--profile");
  const profilePath = profileFlag !== -1 ? process.argv[profileFlag + 1] : "";
  if (!profilePath) {
    console.error("route-review: --profile <file> is required, the file load-profile.mjs --save wrote");
    process.exit(1);
  }
  const loaded = readProfileFile(profilePath);
  if (loaded.error) {
    console.error(`route-review: ${loaded.error}`);
    process.exit(1);
  }
  input.riskRules = inContextRiskPathRules(loaded.profile);

  const decision = routeReview(input);
  process.stdout.write(JSON.stringify(decision) + "\n");
  console.error(
    `route: ${decision.route} — ${decision.reason} ` +
      `(fileCount ${decision.fileCount}, scopeCount ${decision.scopeCount}, churn ${decision.churn}, ` +
      `excluded ${decision.excluded.length}, contentRisk ${decision.contentRisk})`,
  );
  for (const h of decision.contentRiskHits) {
    console.error(`  mass op: ${h.file}:${h.line}  ${h.snippet}`);
  }
}
