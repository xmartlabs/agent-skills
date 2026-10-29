#!/usr/bin/env node
// replay-rows.mjs — one definition of "which rows are comparable", and nobody
// reading around it.
//
// 5 of the 48 recorded rows predate the canonical shape and are documented as
// not aggregatable (references/metrics-schema.md, and the lever ledger). That
// warning lived in prose, and the result on 2026-07-27 was measurable: of the
// scripts that read metrics.jsonl, one had the correct predicate written out
// inline, several had weaker ones (`r.pr && r.headSha`), and the one with no
// filter at all had been written that same day — by someone who had cited the
// warning twelve hours earlier.
//
// Two failure modes, and this file guards both:
//   1. An analysis reads the file raw and silently mixes shapes.
//   2. An analysis writes its OWN canonical predicate, which then drifts from
//      the shared one — two definitions of comparable is the problem, not the
//      fix.
//
// Usage: node eval/replay-rows.mjs

import { readFileSync, readdirSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { allRows, canonicalRows, legacyRows, independentRows, isCanonical, summary } from "./rows.mjs";
import { RECORDED_METRICS_PATH } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nrow selection — eval/rows.mjs`);

console.log(`\n--- the partition is total and disjoint ---`);
const all = allRows(RECORDED_METRICS_PATH);
const canon = canonicalRows(RECORDED_METRICS_PATH);
const legacy = legacyRows(RECORDED_METRICS_PATH);
check("canonical + legacy accounts for every row", canon.length + legacy.length === all.length, `${canon.length}+${legacy.length} vs ${all.length}`);
check("the two sets do not overlap", canon.every(isCanonical) && legacy.every((r) => !isCanonical(r)));
check("there ARE legacy rows to exclude (the guard is not vacuous)", legacy.length > 0, `${legacy.length}`);
check("independent ⊆ canonical", independentRows(RECORDED_METRICS_PATH).length <= canon.length);

console.log(`\n--- the predicate is the shape sentinel, not a proxy ---`);
// schemaVersion is NOT the test: a row can carry one and still be shaped wrong,
// and the legacy rows that matter are the ones missing `mode`/`findings`.
check("a row with mode + findings is canonical", isCanonical({ mode: "workflow", findings: [] }));
check("a row missing findings is not", !isCanonical({ mode: "workflow" }));
check("a row missing mode is not", !isCanonical({ findings: [] }));
check("null/undefined never crash the predicate", !isCanonical(null) && !isCanonical(undefined));

console.log(`\n--- a corrupt line is data loss, not a row ---`);
const dir = mkdtempSync(join(tmpdir(), "pr-rows-"));
const f = join(dir, "metrics.jsonl");
writeFileSync(f, ['{"mode":"workflow","findings":[],"pr":1,"headSha":"a"}', "{ this is not json", ""].join("\n"));
check("unparseable lines are counted, not silently dropped", summary(f).unparseable === 1, JSON.stringify(summary(f)));
check("...and never reach the analyses", canonicalRows(f).length === 1);

console.log(`\n--- nobody reads the file around the helper ---`);
// Analyses only. A replay may read raw: it tests decision logic against whatever
// historical shapes exist, which is the opposite of comparing rows to each other.
const offenders = [];
for (const name of readdirSync(join(HERE))) {
  if (!name.endsWith(".mjs") || name === "rows.mjs" || name === "paths.mjs" || name.startsWith("replay-")) continue;
  const src = readFileSync(join(HERE, name), "utf8");
  // Two distinct offences, because matching only the first let a real one
  // through. Until 2026-07-30 this checked reads alone — `readFileSync(<path>)`
  // — and `score.mjs` builds its path on one line and reads it on another, so
  // the guard stayed green while the scorer pointed at a file that had moved out
  // of this directory. Every corpus score reported "(score unavailable)" and one
  // of those non-answers was written into a RELEASES.md freeze row.
  //
  // So: CONSTRUCTING the path counts as reading it. A comment mentioning
  // metrics.jsonl still passes — only join()/resolve() around it fails.
  const reads = /readFileSync\([^)]*metrics\.jsonl|readFileSync\(METRICS/.test(src);
  const builds = /(?:join|resolve)\([^)]*["']metrics\.jsonl["']/.test(src);
  if (reads || builds) offenders.push(`${name}${builds && !reads ? " (builds its own path)" : ""}`);
}
check("no analysis reads or rebuilds the metrics.jsonl path", offenders.length === 0, offenders.join(", "));

// And nobody re-implements the predicate: one definition, or they drift.
const dupes = [];
for (const name of readdirSync(join(HERE))) {
  if (!name.endsWith(".mjs") || name === "rows.mjs") continue;
  const src = readFileSync(join(HERE, name), "utf8");
  // The FULL predicate, not a bare Array.isArray — replay-gates.mjs uses that
  // defensively inside a loop, which is correct code. A checker that fires on
  // correct code gets ignored, and an ignored checker is worse than none.
  if (/r\.mode && Array\.isArray\(r\.findings\)/.test(src)) dupes.push(name);
}
check("no second copy of the canonical predicate", dupes.length === 0, dupes.join(", "));

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall row-selection expectations hold");
