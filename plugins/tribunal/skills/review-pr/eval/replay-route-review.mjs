#!/usr/bin/env node
// replay-route-review.mjs — Tier 0 guard for scripts/route-review.mjs.
//
// Executes the REAL routeReview (imported, not re-implemented) against real PR
// shapes — PR #2025 is the exact misroute the ROUTE-REVIEW region exists to
// fix (see scripts/route-review.mjs header): a high-risk regex matched a test
// file's name, and the raw file count was used where only product files
// should have counted. If the rules move, move these expectations in the
// same commit or explain why not.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
  routeReview as routeReviewWith,
  productFiles,
  massOpHits,
  diffUsable,
  WORKFLOW_MIN_FILES,
  HIGH_RISK_SCOPE_MAX,
} from "../scripts/route-review.mjs";

import { inContextRiskPathRules } from "../scripts/load-profile.mjs";
import { clientProfileByReviewer } from "./client-profiles.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = resolve(HERE, "..", "scripts", "route-review.mjs");

// Every golden below was written against the rules that used to be constants
// here. They now come from the repo's profile, so the frozen BE profile feeds
// them, and the goldens are unchanged.
const { profile: BE, file: BE_FILE } = clientProfileByReviewer("backend");
const RULES = inContextRiskPathRules(BE);
const routeReview = (input) => routeReviewWith({ riskRules: RULES, ...input });

let failures = 0;
const ok = (name) => console.log(`  ok    ${name}`);
const fail = (name, detail) => {
  failures++;
  console.log(`  FAIL  ${name}${detail ? `\n        ${detail}` : ""}`);
};

const eqArr = (a, b) => Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => x === b[i]);

function checkCase(name, input, expect) {
  const got = routeReview(input);
  const bad = [];
  for (const [k, v] of Object.entries(expect)) {
    const g = got[k];
    const equal = Array.isArray(v) ? eqArr(v, g) : g === v;
    if (!equal) bad.push(`${k}: expected ${JSON.stringify(v)}, got ${JSON.stringify(g)}`);
  }
  if (bad.length) fail(name, bad.join("\n        "));
  else ok(name);
}

console.log("\n--- constants ---");
if (WORKFLOW_MIN_FILES === 10) ok("WORKFLOW_MIN_FILES === 10");
else fail("WORKFLOW_MIN_FILES === 10", `got ${WORKFLOW_MIN_FILES}`);
if (HIGH_RISK_SCOPE_MAX === 6) ok("HIGH_RISK_SCOPE_MAX === 6");
else fail("HIGH_RISK_SCOPE_MAX === 6", `got ${HIGH_RISK_SCOPE_MAX}`);

console.log("\n--- real PR goldens ---");

// PR #2025 — the misroute the ROUTE-REVIEW region exists to fix. 7 raw files,
// 2 excluded (a test file and package.json) -> 5 product files, high-risk but
// scope <= 6 -> in-context. highRiskFiles order follows the INPUT order, not
// alphabetical or grouped order.
const pr2025Files = [
  { additions: 2, changeType: "MODIFIED", deletions: 0, path: "package.json" },
  { additions: 138, changeType: "ADDED", deletions: 0, path: "src/scripts/loadtest/stagingTarget.ts" },
  { additions: 234, changeType: "ADDED", deletions: 0, path: "src/scripts/loadtest/importMemoryCeiling.ts" },
  { additions: 696, changeType: "ADDED", deletions: 0, path: "src/scripts/loadtest/invoiceBulkLoad.ts" },
  { additions: 5, changeType: "MODIFIED", deletions: 2, path: "src/services/v2/batchInvoice.ts" },
  { additions: 186, changeType: "ADDED", deletions: 0, path: "src/services/v2/invoice.export-batching.test.ts" },
  { additions: 91, changeType: "MODIFIED", deletions: 7, path: "src/services/v2/invoice.ts" },
];
checkCase("PR #2025: 7 files, 5 product, high-risk, scope <= 6 -> in-context", { files: pr2025Files }, {
  route: "in-context",
  fileCount: 7,
  scopeCount: 5,
  highRisk: true,
  // Order per input: invoiceBulkLoad.ts precedes batchInvoice.ts/invoice.ts in
  // pr2025Files, so that is the order routeReview must return them in.
  highRiskFiles: [
    "src/scripts/loadtest/invoiceBulkLoad.ts",
    "src/services/v2/batchInvoice.ts",
    "src/services/v2/invoice.ts",
  ],
  churn: 1361,
});
{
  const got = routeReview({ files: pr2025Files });
  const excludedPaths = got.excluded.map((e) => e.path).sort();
  if (eqArr(excludedPaths, ["package.json", "src/services/v2/invoice.export-batching.test.ts"].sort())) {
    ok("PR #2025: excluded exactly the test file + package.json");
  } else {
    fail("PR #2025: excluded exactly the test file + package.json", JSON.stringify(got.excluded));
  }
}

// Same 7 paths, but the test file and package.json are renamed to look like
// product files. This proves the exclusion filter is LOAD-BEARING: without it
// scope would be 5 either way, but with the rename scope becomes 7 (> 6) and
// the same high-risk PR now correctly routes to the Workflow.
const pr2025Renamed = pr2025Files.map((f) => {
  if (f.path === "src/services/v2/invoice.export-batching.test.ts") {
    return { ...f, path: "src/services/v2/invoice.exportBatching.ts" };
  }
  if (f.path === "package.json") {
    return { ...f, path: "src/services/v2/config.ts" };
  }
  return f;
});
checkCase(
  "PR #2025 renamed (test/manifest disguised as product files): scope 7 -> workflow — proves exclusion filter is load-bearing",
  { files: pr2025Renamed },
  { route: "workflow", fileCount: 7, scopeCount: 7, highRisk: true },
);

// PR #2049 — 11 files, raw count alone routes to the Workflow.
const pr2049Files = [
  "prisma/migrations/20260902000000_add_payment_transactions_payments_transaction_id_index/migration.sql",
  "prisma/migrations/20260101000000_add_payment_reference_index/migration.sql",
  "prisma/schema.prisma",
  "src/controllers/v2/payment-types.ts",
  "src/services/v2/payments.test.ts",
  "src/services/v2/payments.ts",
  "src/services/v2/ledger/ledgerSdk.ts",
  "src/services/v2/ledger/types.ts",
  "src/tests/jobs/ledger/refundSplitAllocation.paymentWebhook.multiInvoice.test.ts",
  "src/tests/jobs/ledger/refundSplitAllocation.paymentWebhook.test.ts",
  "src/tests/ledgerSdk.createPayment.test.ts",
];
checkCase("PR #2049: 11 files -> workflow, reason cites 11 >= 10", { files: pr2049Files }, {
  route: "workflow",
  fileCount: 11,
  scopeCount: 7,
  highRisk: true,
  reason: "11 files >= 10",
});

// PR #1992 — mass-delete script change, 4 files, in-context. KNOWN LIMITATION:
// the mass delete is only visible in the diff CONTENT, not the path, so this
// script cannot see it — content-aware detection is an accepted follow-up
// (see scripts/route-review.mjs and RELEASES.md).
const pr1992Files = [
  "scripts/partner/sync-partner-records.test.ts",
  "scripts/partner/sync-partner-records.ts",
  "scripts/partner/purgeStaleRowsSql.test.ts",
  "scripts/partner/purgeStaleRowsSql.ts",
];
checkCase(
  "PR #1992: 4 files, mass-delete script change -> in-context (KNOWN LIMITATION: content-aware mass-delete detection not implemented)",
  { files: pr1992Files },
  { route: "in-context", fileCount: 4, scopeCount: 2, highRisk: false },
);

console.log("\n--- synthetic cases ---");

// 9 vs 10 generic files: the WORKFLOW_MIN_FILES boundary.
const genericFiles = (n) => Array.from({ length: n }, (_, i) => `src/a${i + 1}.ts`);
checkCase("9 generic files -> in-context", { files: genericFiles(9) }, { route: "in-context" });
checkCase("10 generic files -> workflow (boundary)", { files: genericFiles(10) }, { route: "workflow" });

// HIGH_RISK_SCOPE_MAX boundary: 7 vs 6 product files, one of which is
// prisma/schema.prisma (high-risk).
const scope7 = [...genericFiles(6), "prisma/schema.prisma"];
checkCase("7 product files incl. prisma/schema.prisma -> workflow (scope 7 > 6)", { files: scope7 }, {
  route: "workflow",
  scopeCount: 7,
  highRisk: true,
});
const scope6 = [...genericFiles(5), "prisma/schema.prisma"];
checkCase("6 product files incl. prisma/schema.prisma -> in-context (scope 6 <= 6, HIGH_RISK_SCOPE_MAX boundary)", { files: scope6 }, {
  route: "in-context",
  scopeCount: 6,
  highRisk: true,
});

// Scripts stay in scope for the high-risk rule — deliberately not excluded.
checkCase("only risk match is src/scripts/loadtest/invoiceBulkLoad.ts -> highRisk true (scripts stay in scope)", {
  files: ["src/scripts/loadtest/invoiceBulkLoad.ts", ...genericFiles(3)],
}, { highRisk: true });

// A test file matching the risk regex must NOT make the PR high-risk.
checkCase("only risk match is src/services/v2/invoice.test.ts -> highRisk false (test files excluded from risk scan)", {
  files: ["src/services/v2/invoice.test.ts", ...genericFiles(3)],
}, { highRisk: false });

// The routing gate reads the same rules as the in-context lens plan, so it
// inherits whatever they match. `role` is unanchored in the profile on purpose
// (the reasoning sits in in-context-plan.mjs). This is the routing-side guard: without it,
// anchoring `role` moves only replay-in-context-plan.mjs and someone could
// read that as one file's goldens being wrong.
checkCase("only risk match is a camelCase role file -> workflow at scope 7 (goes red under \\brole)", {
  files: ["src/services/v2/userRole.ts", ...genericFiles(6)],
}, { route: "workflow", scopeCount: 7, pathRisk: true });
checkCase("the same camelCase role file at scope 6 -> in-context (the gate still applies to it)", {
  files: ["src/services/v2/userRole.ts", ...genericFiles(5)],
}, { route: "in-context", scopeCount: 6, pathRisk: true });

// Plain strings (the re-review in-scope list shape) must produce the same
// route as the equivalent objects.
{
  const objForm = routeReview({ files: pr1992Files.map((path) => ({ path, additions: 0, deletions: 0 })) });
  const strForm = routeReview({ files: pr1992Files });
  if (objForm.route === strForm.route && objForm.scopeCount === strForm.scopeCount) {
    ok("plain-string files produce the same route as equivalent objects");
  } else {
    fail(
      "plain-string files produce the same route as equivalent objects",
      `obj=${JSON.stringify(objForm)} str=${JSON.stringify(strForm)}`,
    );
  }
}

// A string file list carries no per-file churn, so churn has to come from the
// top-level additions/deletions. Regression guard: asking the NORMALIZED
// entries whether they carry churn answers yes (normalizing a string fills in
// 0), which silently reports churn 0 on every re-review.
{
  const withTopLevel = routeReview({ files: pr1992Files, additions: 100, deletions: 50 });
  const noChurnInfo = routeReview({ files: pr1992Files });
  const perFile = routeReview({ files: [{ path: "src/a.ts", additions: 100, deletions: 50 }] });
  if (withTopLevel.churn === 150 && noChurnInfo.churn === 0 && perFile.churn === 150) {
    ok("churn falls back to top-level additions/deletions for a string file list");
  } else {
    fail(
      "churn falls back to top-level additions/deletions for a string file list",
      `topLevel=${withTopLevel.churn} none=${noChurnInfo.churn} perFile=${perFile.churn}`,
    );
  }
}

console.log("\n--- content risk (mass delete/update in the diff) ---");

// One fixture exercising every attribution rule at once. The line numbers in
// the expectations below are counted from the hunk headers, so a parser that
// miscounts (a '-' line consuming a new-file line, say) fails here rather than
// silently reporting a hit against the wrong line.
const FIXTURE_DIFF = [
  "diff --git a/src/services/v2/accounts.ts b/src/services/v2/accounts.ts",
  "index 1111111..2222222 100644",
  "--- a/src/services/v2/accounts.ts",
  "+++ b/src/services/v2/accounts.ts",
  "@@ -10,6 +10,9 @@ export const purge = async () => {",
  "   const ids = await collect();",
  '   logger.info("purging");',
  "-  await prisma.account.deleteMany({ where: { id: ids[0] } });",
  "+  // a comment naming deleteMany must not count as a hit",
  "+  await prisma.account.deleteMany({ where: { id: { in: ids } } });",
  "   return ids.length;",
  " };",
  "diff --git a/src/services/v2/other.ts b/src/services/v2/other.ts",
  "index 3333333..4444444 100644",
  "--- a/src/services/v2/other.ts",
  "+++ b/src/services/v2/other.ts",
  "@@ -5,3 +5,4 @@ export const touch = async () => {",
  "   await prisma.thing.updateMany({ where: {} });",
  "+  const x = 1;",
  "   return x;",
  " };",
  "diff --git a/src/services/v2/accounts.test.ts b/src/services/v2/accounts.test.ts",
  "index 5555555..6666666 100644",
  "--- a/src/services/v2/accounts.test.ts",
  "+++ b/src/services/v2/accounts.test.ts",
  '@@ -1,3 +1,4 @@ describe("purge", () => {',
  '   describe("purge", () => {',
  "+  await prisma.account.deleteMany({});",
  '   it("works", () => {});',
  " });",
].join("\n");

const FIXTURE_PRODUCT = ["src/services/v2/accounts.ts", "src/services/v2/other.ts"];

{
  const hits = massOpHits(FIXTURE_DIFF, FIXTURE_PRODUCT);
  const code = hits.filter((h) => !h.comment);
  const problems = [];
  if (hits.length !== 2) problems.push(`expected 2 raw hits, got ${hits.length}: ${JSON.stringify(hits)}`);
  if (code.length !== 1) problems.push(`expected 1 non-comment hit, got ${code.length}`);
  if (code[0]?.file !== "src/services/v2/accounts.ts") problems.push(`hit file: ${code[0]?.file}`);
  // 10 and 11 are context, the '-' line consumes nothing, 12 is the comment,
  // so the real call lands on 13.
  if (code[0]?.line !== 13) problems.push(`hit line: expected 13, got ${code[0]?.line}`);
  if (problems.length) fail("massOpHits: attribution, line numbers, comment flag", problems.join("\n        "));
  else ok("massOpHits: attribution, line numbers, comment flag");
}
{
  // Each of these is a separate way to be wrong, so each gets its own name.
  const hits = massOpHits(FIXTURE_DIFF, FIXTURE_PRODUCT);
  const files = hits.map((h) => h.file);
  if (!files.includes("src/services/v2/accounts.test.ts")) ok("a mass op ADDED in a test file is not a hit (product filter)");
  else fail("a mass op ADDED in a test file is not a hit (product filter)", JSON.stringify(hits));

  if (!hits.some((h) => h.file === "src/services/v2/other.ts")) ok("a mass op on a CONTEXT line is not a hit (it predates the PR)");
  else fail("a mass op on a CONTEXT line is not a hit (it predates the PR)", JSON.stringify(hits));

  if (!hits.some((h) => h.line === 12 && !h.comment)) ok("a mass op on a REMOVED line is not a hit");
  else fail("a mass op on a REMOVED line is not a hit", JSON.stringify(hits));
}

// The gate: a content hit is high-risk surface under the SAME scope rule as a
// risky path. Measured, not chosen — see specs/2026-09-10-content-risk-gate.md
// and the script header. These two cases are what make the gate load-bearing.
const contentScope7 = [...FIXTURE_PRODUCT, ...genericFiles(5)];
const contentScope6 = [...FIXTURE_PRODUCT, ...genericFiles(4)];
checkCase(
  "content hit + scope 7 and NO risky path -> workflow (the v60 rule alone would say in-context)",
  { files: contentScope7, diff: FIXTURE_DIFF },
  { route: "workflow", scopeCount: 7, pathRisk: false, contentRisk: true, highRisk: true },
);
checkCase(
  "content hit + scope 6 -> in-context (same HIGH_RISK_SCOPE_MAX boundary a risky path gets)",
  { files: contentScope6, diff: FIXTURE_DIFF },
  { route: "in-context", scopeCount: 6, pathRisk: false, contentRisk: true, highRisk: true },
);
checkCase(
  "same 7 files with no diff -> in-context and contentRisk null, never false",
  { files: contentScope7 },
  { route: "in-context", contentRisk: null, highRisk: false },
);

// A truncated `gh pr diff` still parses and still contains `diff --git`, so
// "no hits" from one is not evidence of anything. PR #1911 reviewed 2KB of a
// truncation banner; here the same failure would look like a clean PR.
{
  const truncated =
    "Full diff available at: https://github.com/o/r/pull/1.diff\n" + FIXTURE_DIFF;
  const got = routeReview({ files: contentScope7, diff: truncated });
  if (got.contentRisk === null && got.route === "in-context") {
    ok("truncation banner -> contentRisk null (not false) and no escalation on unread evidence");
  } else {
    fail(
      "truncation banner -> contentRisk null (not false) and no escalation on unread evidence",
      `contentRisk=${got.contentRisk} route=${got.route}`,
    );
  }
  if (/content risk is UNKNOWN/.test(got.reason)) ok("an unusable diff says so in the reason, where it could have flipped the route");
  else fail("an unusable diff says so in the reason", got.reason);
}
{
  const problems = [];
  if (diffUsable(FIXTURE_DIFF, 0) !== true) problems.push("a real diff must be usable");
  if (diffUsable("", 0) !== false) problems.push("empty must be unusable");
  if (diffUsable("no diff header here, but long enough to pass the length floor. ".repeat(6), 0) !== false) {
    problems.push("text without a `diff --git` header must be unusable");
  }
  // The churn-proportion rule: 400 changed lines cannot fit in 900 bytes.
  if (diffUsable(FIXTURE_DIFF, 400) !== false) problems.push("a diff far too short for its churn must be unusable");
  if (problems.length) fail("diffUsable() rules", problems.join("\n        "));
  else ok("diffUsable() rules");
}

console.log("\n--- productFiles() ---");
{
  const { product, excluded } = productFiles(["a.ts", "a.test.ts", "README.md", "package.json"]);
  if (eqArr(product, ["a.ts"]) && excluded.length === 3) {
    ok("productFiles() partitions correctly");
  } else {
    fail("productFiles() partitions correctly", JSON.stringify({ product, excluded }));
  }
}

console.log("\n--- CLI behaviour ---");
{
  const out = execFileSync(process.execPath, [SCRIPT, "--profile", BE_FILE], {
    input: JSON.stringify({ files: ["src/a.ts", "src/b.ts"] }),
    encoding: "utf8",
  });
  const lines = out.trim().split("\n");
  if (lines.length !== 1) {
    fail("CLI: exactly one JSON line on stdout", `${lines.length} lines: ${out}`);
  } else {
    try {
      JSON.parse(lines[0]);
      ok("CLI: exactly one JSON line on stdout, exit 0");
    } catch (e) {
      fail("CLI: exactly one JSON line on stdout", `not valid JSON: ${e.message}`);
    }
  }
}
{
  let exitCode = 0;
  try {
    execFileSync(process.execPath, [SCRIPT, "--profile", BE_FILE], { input: JSON.stringify({}), encoding: "utf8", stdio: "pipe" });
  } catch (e) {
    exitCode = e.status;
  }
  if (exitCode !== 0) ok(`CLI: input {} -> non-zero exit (${exitCode})`);
  else fail("CLI: input {} -> non-zero exit", "exited 0");
}

{
  let exitCode = 0;
  let stderr = "";
  try {
    execFileSync(process.execPath, [SCRIPT], { input: JSON.stringify({ files: ["src/a.ts"] }), encoding: "utf8", stdio: "pipe" });
  } catch (e) {
    exitCode = e.status;
    stderr = String(e.stderr || "");
  }
  if (exitCode !== 0 && stderr.includes("--profile")) ok("CLI: no --profile -> non-zero exit naming the flag");
  else fail("CLI: no --profile -> non-zero exit naming the flag", `exit ${exitCode}: ${stderr}`);
}

console.log("\n--- the rules come from the profile ---");
{
  let threw = false;
  try {
    routeReviewWith({ files: ["src/a.ts"] });
  } catch {
    threw = true;
  }
  if (threw) ok("routeReview without riskRules throws instead of calling nothing risky");
  else fail("routeReview without riskRules throws instead of calling nothing risky", "it returned");
}
{
  const fe = inContextRiskPathRules(clientProfileByReviewer("frontend").profile);
  const same = fe.length === RULES.length && fe.every((r, i) => r.focus === RULES[i].focus && r.re.source === RULES[i].re.source);
  if (same) ok("the FE profile routes on the same rules as BE, as the shared constants did");
  else fail("the FE profile routes on the same rules as BE", JSON.stringify(fe.map((r) => r.re.source)));
}
{
  // Without tribunal.inContextRiskPaths the gate reads riskPaths, the wider
  // list, so a bare `migration` path becomes risky.
  const { tribunal, ...shared } = BE;
  const wide = routeReviewWith({ riskRules: inContextRiskPathRules(shared), files: ["src/jobs/migration-runner.ts", ...genericFiles(6)] });
  const narrow = routeReview({ files: ["src/jobs/migration-runner.ts", ...genericFiles(6)] });
  if (wide.pathRisk === true && narrow.pathRisk === false) ok("no tribunal section -> the gate falls back to riskPaths");
  else fail("no tribunal section -> the gate falls back to riskPaths", `wide ${wide.pathRisk}, narrow ${narrow.pathRisk}`);
  const { riskPaths, ...bare } = shared;
  const none = routeReviewWith({ riskRules: inContextRiskPathRules(bare), files: ["src/auth/login.ts", ...genericFiles(6)] });
  if (none.pathRisk === false && none.route === "in-context") ok("no risk paths at all -> no path is high-risk");
  else fail("no risk paths at all -> no path is high-risk", JSON.stringify(none));
}

if (failures) {
  console.log(`\n${failures} case(s) FAILED`);
  process.exit(1);
}
console.log("\nall route-review expectations hold");
