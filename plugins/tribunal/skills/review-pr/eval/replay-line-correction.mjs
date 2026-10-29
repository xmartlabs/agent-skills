#!/usr/bin/env node
// replay-line-correction.mjs — a lens that cites the patch's line number
// instead of the file's gets its line corrected, and nothing else does.
//
// Measured 2026-10-22 over 298 lens findings checked against the file at the
// reviewed head: 68 cited the line number of the patch they had read, and the
// final report carried some of them, blocking ones included, pointing past the
// end of the file. The resolver already knew the new-file line of every diff
// line and ran in shadow mode. This replay covers the correction
// (patchLineCorrection in scripts/resolve-evidence-line.mjs) and the region
// that applies it in the workflow (LINE-CORRECTION).
//
// Usage: node eval/replay-line-correction.mjs

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { SKILL_DIR, WORKFLOW } from "./paths.mjs";
import { patchLineCorrection, resolveEvidenceBatch } from "../scripts/resolve-evidence-line.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// Patch line numbers are written next to each line. The second file's hunk
// starts at new-file line 40, so its patch lines and file lines differ.
const DIFF = [
  "diff --git a/src/a.ts b/src/a.ts", // 1
  "index 1111111..2222222 100644", // 2
  "--- a/src/a.ts", // 3
  "+++ b/src/a.ts", // 4
  "@@ -1,3 +1,4 @@", // 5
  " export function first() {", // 6  -> a.ts:1
  "+  const guard = requireAccount(accountId);", // 7  -> a.ts:2
  "   return loadEverything();", // 8  -> a.ts:3
  " }", // 9  -> a.ts:4
  "diff --git a/src/b.ts b/src/b.ts", // 10
  "index 3333333..4444444 100644", // 11
  "--- a/src/b.ts", // 12
  "+++ b/src/b.ts", // 13
  "@@ -40,4 +40,5 @@ export async function second() {", // 14
  "   const rows = await prisma.invoice.findMany({", // 15 -> b.ts:40
  "-    where: { accountId },", // 16 -> removed
  "+    where: { accountId, deletedAt: null },", // 17 -> b.ts:41
  "+    orderBy: { createdAt: 'desc' },", // 18 -> b.ts:42
  "   });", // 19 -> b.ts:43
  "   return rows;", // 20 -> b.ts:44
].join("\n");

console.log("\n--- patchLineCorrection ---");
{
  const fix = patchLineCorrection(DIFF, { file: "src/b.ts", line: 17, evidence: "where: { accountId, deletedAt: null }," });
  check("a patch line on an added line of the cited file moves to its file line", fix && fix.correctedLine === 41 && fix.patchLine === 17, JSON.stringify(fix));

  const multi = patchLineCorrection(DIFF, {
    file: "src/b.ts",
    line: 18,
    evidence: "where: { accountId, deletedAt: null },\n    orderBy: { createdAt: 'desc' },",
  });
  check("any line of a multi-line quote counts, and the patch line picks which", multi && multi.correctedLine === 42, JSON.stringify(multi));

  const prefixed = patchLineCorrection(DIFF, { file: "src/b.ts", line: 17, evidence: "+    where: { accountId, deletedAt: null }," });
  check("a quote copied with the patch's + prefix still matches", prefixed && prefixed.correctedLine === 41, JSON.stringify(prefixed));

  const context = patchLineCorrection(DIFF, { file: "src/b.ts", line: 15, evidence: "const rows = await prisma.invoice.findMany({" });
  check("a context line corrects too", context && context.correctedLine === 40, JSON.stringify(context));

  check("a correct file line is left alone", patchLineCorrection(DIFF, { file: "src/b.ts", line: 41, evidence: "where: { accountId, deletedAt: null }," }) === null);
  check("a patch line that lands in another file's diff is left alone", patchLineCorrection(DIFF, { file: "src/a.ts", line: 17, evidence: "where: { accountId, deletedAt: null }," }) === null);
  check("a patch line on a removed line is left alone", patchLineCorrection(DIFF, { file: "src/b.ts", line: 16, evidence: "where: { accountId }," }) === null);
  check("a claim with no quote line of its own file's diff near it is left alone", patchLineCorrection(DIFF, { file: "src/a.ts", line: 9, evidence: "where: { accountId, deletedAt: null }," }) === null);

  const near = patchLineCorrection(DIFF, { file: "src/b.ts", line: 19, evidence: "where: { accountId, deletedAt: null }," });
  check("a patch line 2 lines from its quote moves, keeping the lens's own line", near && near.correctedLine === 43 && near.patchLine === 19, JSON.stringify(near));
  check(
    "but not when the claim read as a file line is near the quote too",
    patchLineCorrection(DIFF, { file: "src/a.ts", line: 9, evidence: "const guard = requireAccount(accountId);" }) === null,
  );
  const LONG = [
    "diff --git a/src/d.ts b/src/d.ts",
    "--- a/src/d.ts",
    "+++ b/src/d.ts",
    "@@ -100,30 +200,31 @@",
    "+const marker = computeSettlementTotals(invoices);", // 5 -> d.ts:200
    ...Array.from({ length: 30 }, (_, k) => ` const filler${k} = ${k};`), // 6..35 -> d.ts:201..230
  ].join("\n");
  check(
    "a claim more than PATCH_LINE_NEAR patch lines from the quote is left alone",
    patchLineCorrection(LONG, { file: "src/d.ts", line: 5 + 13, evidence: "const marker = computeSettlementTotals(invoices);" }) === null,
  );
  check(
    "and one at PATCH_LINE_NEAR still moves",
    patchLineCorrection(LONG, { file: "src/d.ts", line: 5 + 12, evidence: "const marker = computeSettlementTotals(invoices);" })?.correctedLine === 212,
  );
  check("a generic quote line is never enough", patchLineCorrection(DIFF, { file: "src/a.ts", line: 9, evidence: "}" }) === null);
  check("a header line of the patch has no file line", patchLineCorrection(DIFF, { file: "src/b.ts", line: 14, evidence: "@@ -40,4 +40,5 @@ export async function second() {" }) === null);
  check("no line or no quote, no correction", patchLineCorrection(DIFF, { file: "src/b.ts", evidence: "where: { accountId, deletedAt: null }," }) === null && patchLineCorrection(DIFF, { file: "src/b.ts", line: 17 }) === null);

  // Both readings fit: patch line 26 holds the quote (file line 23), and file
  // line 26 holds it too. A claim of 26 may be right as it stands, so it stays.
  const AMBIG = DIFF + "\n" + [
    "diff --git a/src/c.ts b/src/c.ts", // 21
    "--- a/src/c.ts", // 22
    "+++ b/src/c.ts", // 23
    "@@ -20,2 +22,3 @@", // 24
    "   const total = sumInvoices(rows);", // 25 -> c.ts:22
    "+  const total2 = sumInvoices(rows);", // 26 -> c.ts:23
    "   return total;", // 27 -> c.ts:24
    "@@ -24,1 +26,1 @@", // 28
    "-const legacyTotal = 0;", // 29
    "+const total2 = sumInvoices(rows);", // 30 -> c.ts:26
  ].join("\n");
  check(
    "when the claimed number is already a quoted diff line of the file, nothing moves",
    patchLineCorrection(AMBIG, { file: "src/c.ts", line: 26, evidence: "const total2 = sumInvoices(rows);" }) === null,
  );
  check(
    "the same diff still corrects a claim only the patch reading explains",
    patchLineCorrection(AMBIG, { file: "src/c.ts", line: 30, evidence: "const total2 = sumInvoices(rows);" })?.correctedLine === 26,
  );
}

console.log("\n--- resolveEvidenceBatch carries the correction ---");
{
  const out = resolveEvidenceBatch(DIFF, [
    { id: 0, file: "src/b.ts", line: 17, evidence: "where: { accountId, deletedAt: null }," },
    { id: 1, file: "src/b.ts", line: 41, evidence: "where: { accountId, deletedAt: null }," },
  ]);
  const [moved, kept] = out.results || [];
  check("the batch ran on a usable diff", out.ran === true, JSON.stringify(out));
  check("a corrected result names both lines", moved && moved.correctedLine === 41 && moved.patchLine === 17);
  check("and the quote is resolved at the corrected line", moved && moved.matchKind === "exact" && moved.claimedLine === 41);
  check("an uncorrected result carries neither field", kept && !("correctedLine" in kept) && !("patchLine" in kept) && kept.matchKind === "exact");
}

console.log("\n--- LINE-CORRECTION region ---");
let api;
{
  const loaded = loadRegion(WORKFLOW, "LINE-CORRECTION", ["applyLineCorrection"]);
  api = loaded.api;
  check("region references no run state", loaded.violations.length === 0, loaded.violations.join(" | "));
  const c = { file: "src/b.ts", line: 17 };
  check("a matching correction applies", api.applyLineCorrection(c, { correctedLine: 41, patchLine: 17 }) === true && c.line === 41 && c.lineCorrectedFrom === 17);
  const s = { file: "src/b.ts", line: "17" };
  check("a line copied as a string still matches", api.applyLineCorrection(s, { correctedLine: 41, patchLine: 17 }) === true && s.line === 41);
  const other = { file: "src/b.ts", line: 20 };
  check("a correction for a different line is refused", api.applyLineCorrection(other, { correctedLine: 41, patchLine: 17 }) === false && other.line === 20 && !("lineCorrectedFrom" in other));
  for (const [why, r] of [
    ["no correction", { matchKind: "exact" }],
    ["a non-integer line", { correctedLine: "41", patchLine: 17 }],
    ["a zero line", { correctedLine: 0, patchLine: 17 }],
    ["the same line", { correctedLine: 17, patchLine: 17 }],
  ]) {
    const x = { file: "src/b.ts", line: 17 };
    check(`${why} changes nothing`, api.applyLineCorrection(x, r) === false && x.line === 17);
  }
  check("no resolution changes nothing", api.applyLineCorrection({ line: 17 }, null) === false);
}

console.log("\n--- wiring ---");
{
  const src = readFileSync(WORKFLOW, "utf8");
  const loop = src.slice(src.indexOf("allCandidates[i].evidenceResolution = resolution;"), src.indexOf("allCandidates[i].evidenceResolution = resolution;") + 300);
  check("the resolver loop applies the correction and counts it", /if \(applyLineCorrection\(allCandidates\[i\], resolution\)\) evidenceLineCorrectedCount\+\+;/.test(loop));
  check("the correction runs before the dedup fold", src.indexOf("applyLineCorrection(allCandidates[i]") < src.indexOf("// --- Pre-panel dedup"));
  check("the row reports the count, not a constant 0", /\n\s+evidenceLineCorrectedCount,\n/.test(src) && !/evidenceLineCorrectedCount: 0/.test(src));
  check("each finding row says whether its line moved", /lineCorrected: v\.finding\.evidenceResolution \? Number\.isInteger\(v\.finding\.lineCorrectedFrom\) : null/.test(src));
  const resolverSrc = readFileSync(join(SKILL_DIR, "scripts", "resolve-evidence-line.mjs"), "utf8");
  check("the batch corrects before it resolves", /const fix = patchLineCorrection\(diffText, c\);/.test(resolverSrc));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall line-correction expectations hold");
