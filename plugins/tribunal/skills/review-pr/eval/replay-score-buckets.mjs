#!/usr/bin/env node
// replay-score-buckets.mjs — the four-bucket classification, run as the SHIPPING
// scorer against fixtures.
//
// WHY IT EXISTS. `eval/score.mjs` is the only instrument that can answer "better
// or worse", and every number it prints is computed from hand-written ground
// truth. A classification bug here does not fail loudly — it produces a confident
// rate about nothing, which is the exact failure the corpus exists to prevent.
//
// The three properties that are easy to get wrong, and are pinned here:
//   1. `knownAccurateNonDefect` must NOT raise the FP rate. A finding can be true
//      and still not be a defect; scoring those as FP would teach every later
//      calibration that a CORRECT finding is a false positive.
//   2. `undecidable` must leave both rates untouched AND still be named. Silently
//      dropping it turns "cannot be judged" into "does not exist".
//   3. The same id in two buckets is the ground truth contradicting itself, and
//      must fail loudly rather than let the first match win.
//
// It runs the real script via SCORE_CORPUS_DIR / SCORE_METRICS rather than
// re-implementing `matches()`, so the check cannot drift from what ships.
//
// Usage: node eval/replay-score-buckets.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCORE = resolve(HERE, "score.mjs");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nfour-bucket classification — eval/score.mjs`);

const dir = mkdtempSync(join(tmpdir(), "pr-score-"));
let caseN = 0;

// One canonical row with four surviving findings whose titles are deliberately
// distinct, so a label can only match the one it names.
const FINDINGS = [
  { title: "escapeSqlString omits backslashes before embedding", file: "src/a.ts" },
  { title: "Per-row prisma.account lookup instead of one batched query", file: "src/b.ts" },
  { title: "Backfill fabricates taxYear for affected rows", file: "src/c.ts" },
  { title: "Unguarded parseFloat on a money amount field", file: "src/d.ts" },
];

function run(entry, laterRows = []) {
  caseN += 1;
  const cdir = join(dir, `c${caseN}`);
  mkdirSync(cdir, { recursive: true });
  writeFileSync(
    join(cdir, "pr-9001.json"),
    JSON.stringify({
      repo: "x/y",
      pr: 9001,
      headSha: "deadbeef",
      status: "annotated",
      groundTruthTier: "human-judged",
      knownReal: [],
      knownBogus: [],
      ...entry,
    }),
  );
  const metrics = join(dir, `m${caseN}.jsonl`);
  writeFileSync(
    metrics,
    JSON.stringify({
      schemaVersion: 3,
      mode: "workflow",
      pr: 9001,
      headSha: "deadbeef",
      ts: "2026-07-28T00:00:00Z",
      findings: FINDINGS.map((f) => ({ ...f, survived: true, evidence: "" })),
    }) +
      "\n" +
      laterRows
        .map((r) => JSON.stringify({ schemaVersion: 3, mode: "workflow", pr: 9001, findings: [], ...r }) + "\n")
        .join(""),
  );
  let out, code = 0;
  try {
    out = execFileSync("node", [SCORE], {
      encoding: "utf8",
      env: { ...process.env, SCORE_CORPUS_DIR: cdir, SCORE_METRICS: metrics },
    });
  } catch (err) {
    out = `${err.stdout || ""}${err.stderr || ""}`;
    code = err.status ?? 1;
  }
  const t = out.match(/TP=(\d+)\s+FP=(\d+)\s+FN=(\d+)\s+accurate=(\d+)\s+undecidable=(\d+)\s+unknown=(\d+)/);
  return {
    out,
    code,
    TP: +(t?.[1] ?? -1), FP: +(t?.[2] ?? -1), FN: +(t?.[3] ?? -1),
    accurate: +(t?.[4] ?? -1), undecidable: +(t?.[5] ?? -1), unknown: +(t?.[6] ?? -1),
    fpRate: (out.match(/FP rate = ([\d.]+)/) || [])[1],
    noise: (out.match(/noise\s+= ([\d.]+)/) || [])[1],
  };
}

const ann = (id, file, description, extra = {}) => ({ id, file, description, ...extra });

console.log(`\n--- an unlabelled finding stays in the queue ---`);
{
  const r = run({});
  check("all four are unknown", r.unknown === 4, JSON.stringify(r));
  check("no rate is computed over a partly-judged corpus", !r.fpRate && /NOT COMPUTED/.test(r.out), r.fpRate);
}

console.log(`\n--- accurate-but-not-a-defect does NOT count as a false positive ---`);
{
  const r = run({
    knownReal: [ann("escapesqlstring-backslash", "src/a.ts", "escapeSqlString omits backslashes")],
    knownBogus: [ann("backfill-fabricates-taxyear", "src/c.ts", "Backfill fabricates taxYear")],
    knownAccurateNonDefect: [ann("per-row-prisma-account-lookup", "src/b.ts", "Per-row prisma.account lookup batched query")],
    undecidable: [ann("parsefloat-money-amount", "src/d.ts", "Unguarded parseFloat money amount", { reason: "needs the production value distribution" })],
  });
  check("queue is empty", r.unknown === 0, JSON.stringify(r));
  check("TP=1 FP=1 accurate=1 undecidable=1", r.TP === 1 && r.FP === 1 && r.accurate === 1 && r.undecidable === 1, JSON.stringify(r));
  // 1 bogus over (1 real + 1 bogus + 1 accurate) = 0.333. If `accurate` were
  // scored as FP it would read 0.667; if it were dropped entirely, 0.500.
  check("FP rate keeps accurate in the DENOMINATOR only", r.fpRate === "0.333", `got ${r.fpRate}`);
  check("noise is reported separately", r.noise === "0.333", `got ${r.noise}`);
  check("the undecidable is named with its reason", /undecidable: needs the production value distribution/.test(r.out));
  check("...and is excluded from both rates", /excluded from both/.test(r.out));
}

console.log(`\n--- undecidable does not move the rates ---`);
{
  const base = run({
    knownReal: [ann("escapesqlstring-backslash", "src/a.ts", "escapeSqlString omits backslashes")],
    knownBogus: [ann("backfill-fabricates-taxyear", "src/c.ts", "Backfill fabricates taxYear")],
    knownAccurateNonDefect: [
      ann("per-row-prisma-account-lookup", "src/b.ts", "Per-row prisma.account lookup batched query"),
      ann("parsefloat-money-amount", "src/d.ts", "Unguarded parseFloat money amount"),
    ],
  });
  const withUndecidable = run({
    knownReal: [ann("escapesqlstring-backslash", "src/a.ts", "escapeSqlString omits backslashes")],
    knownBogus: [ann("backfill-fabricates-taxyear", "src/c.ts", "Backfill fabricates taxYear")],
    knownAccurateNonDefect: [ann("per-row-prisma-account-lookup", "src/b.ts", "Per-row prisma.account lookup batched query")],
    undecidable: [ann("parsefloat-money-amount", "src/d.ts", "Unguarded parseFloat money amount", { reason: "needs production data" })],
  });
  check(
    "moving a finding to undecidable raises the FP rate rather than hiding it",
    base.fpRate === "0.250" && withUndecidable.fpRate === "0.333",
    `base ${base.fpRate}, with ${withUndecidable.fpRate}`,
  );
  check("...because the denominator shrank, which is honest", withUndecidable.accurate === 1, JSON.stringify(withUndecidable));
}

console.log(`\n--- the ground truth may not contradict itself ---`);
{
  const r = run({
    knownReal: [ann("dup-id", "src/a.ts", "escapeSqlString omits backslashes")],
    knownBogus: [ann("dup-id", "src/c.ts", "Backfill fabricates taxYear")],
  });
  check("a duplicated id fails loudly", /CORPUS ERRORS/.test(r.out), r.out.slice(-200));
  check("...and exits non-zero", r.code !== 0, `exit ${r.code}`);
  check("...naming both buckets", /is in both knownReal and knownBogus/.test(r.out));
}

console.log(`\n--- the labelled commit is scored, not a later fix commit ---`);
{
  const real = [ann("escapesqlstring-backslash", "src/a.ts", "escapeSqlString omits backslashes")];
  const later = [{ headSha: "f1xed000", ts: "2026-07-29T00:00:00Z" }];
  const r = run({ knownReal: real }, later);
  check("a clean run on the fix commit does not turn the labelled hit into a miss", r.TP === 1 && r.FN === 0, JSON.stringify(r));
  const unlabelled = run({ headSha: "0ther000", knownReal: real }, later);
  check("with no row at the labelled commit the latest run is scored", unlabelled.FN === 1, JSON.stringify(unlabelled));
  check("...and the fallback is named", /not the labelled 0ther000/.test(unlabelled.out), unlabelled.out.slice(-300));
}

console.log(`\n--- alternate anchor files ---`);
{
  const hit = run({ knownReal: [ann("escapesqlstring-backslash", undefined, "escapeSqlString omits backslashes", { files: ["src/zz.ts", "src/a.ts"] })] });
  check("a finding at any listed file matches", hit.TP === 1, JSON.stringify(hit));
  const miss = run({ knownReal: [ann("escapesqlstring-backslash", undefined, "escapeSqlString omits backslashes", { files: ["src/zz.ts"] })] });
  check("a finding at an unlisted file does not", miss.TP === 0 && miss.FN === 1, JSON.stringify(miss));
}

console.log(`\n--- an undecidable without a reason is not an answer ---`);
{
  const r = run({ undecidable: [ann("parsefloat-money-amount", "src/d.ts", "Unguarded parseFloat money amount")] });
  check("missing reason fails loudly", /has no reason/.test(r.out), r.out.slice(-200));
  check("...and exits non-zero", r.code !== 0, `exit ${r.code}`);
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall four-bucket expectations hold");
