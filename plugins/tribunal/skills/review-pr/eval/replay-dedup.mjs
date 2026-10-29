#!/usr/bin/env node
// replay-dedup.mjs — regression test for the pre-panel dedup guards.
//
// Replays the exact clusters the dedup dispatch returned on the PR #1911 run
// (wf_302b44be-3dd) — [[4,21,24,25,27],[5,22],[9,26]] over that run's 29
// candidates — through the current guards, and asserts:
//
//   1. the cross-file guard splits [4,21,24,25,27]: only the two genuine
//      dedupe-contacts.ts duplicates survive as a cluster; the exit-0 instances
//      in bulk-void-invoices.ts, update-payment-links.ts and
//      reverse-cancelled-order.ts stay separate candidates, each reaching its
//      own panel. Under the old code all four collapsed into the first and
//      never appeared in the review at all;
//   2. [9,26] — end-of-run flush in bulk-late-fee-application.ts vs the same
//      defect in bulk-rebuild-invoices.ts — likewise stays split;
//   3. [5,22], two framings of one defect in one file, still merges;
//   4. a merged candidate carries mergedLocations for every member, so nothing
//      a merge touches can vanish from the report.
//
// The within-file veto (mergeableWithin and its constants) is the shipping
// code: the DEDUP-VETO region cut out of workflows/tribunal.js by
// extract-pure-region.mjs. Until 2026-10-19 it was a copy kept here by hand,
// pinned by nothing. The cluster walk in applyGuards below is still a copy of
// the workflow's, because that loop sits in the dedup step itself, beside its
// agent() call, and cannot be cut out pure.
//
// Usage: node eval/replay-dedup.mjs [--verbose]

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { FIXTURES_DIR } from "./paths.mjs";
import { loadRegion } from "./extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(
  readFileSync(join(FIXTURES_DIR, "dedup", "pr-1911-clusters.json"), "utf8"),
);
const verbose = process.argv.includes("--verbose");

const WORKFLOW = join(HERE, "..", "..", "..", "workflow-src", "tribunal.js");
const { mergeableWithin } = loadRegion(WORKFLOW, "DEDUP-VETO", ["mergeableWithin"]).api;

function applyGuards(candidates, rawClusters) {
  const claimed = new Set();
  const clusters = [];
  let crossFileClustersRejected = 0;
  let proximityVetoed = 0;
  for (const idxs of rawClusters) {
    const seen = new Set();
    const valid = idxs.filter((i) => {
      if (!Number.isInteger(i) || i < 0 || i >= candidates.length || claimed.has(i) || seen.has(i))
        return false;
      seen.add(i);
      return true;
    });
    const byFile = new Map();
    for (const i of valid) {
      const key = candidates[i].file || "";
      if (!byFile.has(key)) byFile.set(key, []);
      byFile.get(key).push(i);
    }
    const sameFileGroups = [...byFile.values()].filter((g) => g.length >= 2);
    if (byFile.size > 1) crossFileClustersRejected += valid.length - sameFileGroups.flat().length;
    for (const group of sameFileGroups) {
      const anchor = candidates[group[0]];
      const kept = group.filter((i, pos) => pos === 0 || mergeableWithin(anchor, candidates[i]));
      proximityVetoed += group.length - kept.length;
      if (kept.length < 2) continue;
      kept.forEach((i) => claimed.add(i));
      clusters.push(kept);
    }
  }
  return { clusters, claimed, crossFileClustersRejected, proximityVetoed };
}

function mergeLocations(members) {
  return members.map((m) => ({ file: m.file, line: m.line, title: m.title, lens: m.lens }));
}
// --- end mirror ---

const { candidates, rawClusters } = fixture;
const res = applyGuards(candidates, rawClusters);
const merged = res.clusters.map((idxs) => mergeLocations(idxs.map((i) => candidates[i])));
const survivingCount = candidates.length - res.claimed.size + res.clusters.length;

console.log("dedup guard replay — PR #1911 clusters\n");
console.log(`  candidates                 ${candidates.length}`);
console.log(`  clusters proposed          ${rawClusters.length} (${rawClusters.flat().length} members)`);
console.log(`  clusters kept              ${res.clusters.length}`);
console.log(`  crossFileClustersRejected  ${res.crossFileClustersRejected}`);
console.log(`  proximityVetoed            ${res.proximityVetoed}`);
console.log(`  candidates reaching panel  ${survivingCount}`);

if (verbose) {
  for (const [n, group] of res.clusters.entries()) {
    console.log(`\n  cluster ${n}:`);
    for (const m of merged[n]) console.log(`    ${m.file}:${m.line}  ${m.title.slice(0, 70)}`);
  }
}

const failures = [];

// 1 + 2: every kept cluster is single-file.
for (const [n, group] of merged.entries()) {
  const files = new Set(group.map((m) => m.file));
  if (files.size !== 1) failures.push(`cluster ${n} spans ${files.size} files: ${[...files].join(", ")}`);
}

// The four files the old code silently dropped must each still be represented
// among the candidates that reach the panel.
// Found by the sha256 of the path: the fixture keeps the client's real paths,
// and naming them here would publish them. Zero or several matches leave a
// path no candidate has, so it fails.
const fixtureFile = (hash, label) => {
  const hits = [...new Set(candidates.map((c) => c.file).filter((f) => f && createHash("sha256").update(f).digest("hex").startsWith(hash)))];
  return hits.length === 1 ? hits[0] : `<${hits.length} fixture files for ${label}>`;
};
const RESCUED = [
  fixtureFile("39fc0c0b520f31f9", "the bulk void-invoices script"),
  fixtureFile("30a35593914a4a23", "the payment-links update script"),
  fixtureFile("3ccdba056945273c", "the reverse-cancelled-order script"),
  fixtureFile("1bd4ef53e7399019", "the bulk rebuild-invoices script"),
];
const survivingIdx = candidates.map((_, i) => i).filter((i) => !res.claimed.has(i));
const survivingFiles = new Set([
  ...survivingIdx.map((i) => candidates[i].file),
  ...merged.flat().map((m) => m.file),
]);
for (const f of RESCUED) {
  if (!survivingFiles.has(f)) failures.push(`${f} was dropped by the dedup pass — this is the PR #1911 bug`);
}

// 3: the one legitimate same-file, same-defect cluster still merges.
const DEDUPE_FILE = fixtureFile("a82fc8817f96aff2", "the contact-merge script");
if (!merged.some((g) => g.length >= 2 && g.every((m) => m.file === DEDUPE_FILE)))
  failures.push("the genuine dedupe-contacts.ts duplicate pair no longer merges — the guards over-veto");

// 4: a merged candidate keeps every member's location.
for (const [n, group] of merged.entries()) {
  if (group.length !== res.clusters[n].length)
    failures.push(`cluster ${n}: mergedLocations has ${group.length} entries for ${res.clusters[n].length} members`);
  if (group.some((m) => !m.file)) failures.push(`cluster ${n}: a mergedLocations entry has no file`);
}

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nall dedup guard expectations hold");
