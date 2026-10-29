#!/usr/bin/env node
// replay-vote-quotes.mjs: which REFUTED votes have their contradicting quote
// looked up at the head commit, how a lookup result becomes quoteFound, and
// that a quote not in the file sets the vote aside. Runs the shipping
// VOTE-QUOTES and PANEL-GATES regions of workflows/tribunal.js.
//
// Usage: node eval/replay-vote-quotes.mjs

import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = join(HERE, "..", "..", "..", "workflow-src", "tribunal.js");
const { api: Q } = loadRegion(WORKFLOW, "VOTE-QUOTES", ["checkableVoteQuotes", "voteQuoteVerdicts"]);
const { api: G } = loadRegion(WORKFLOW, "PANEL-GATES", ["factualRefutationFails"]);

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? `: ${detail}` : ""}`);
    failures.push(name);
  }
};

const refuted = (o = {}) => ({ verdict: "REFUTED", introducedByPr: "yes", killReason: "not-triggered", contradiction: { file: "src/a.ts", line: 10, quote: "if (!x) return;" }, ...o });
const states = [
  { finding: { file: "src/a.ts" }, votes: [refuted(), { verdict: "NOT_REFUTED", introducedByPr: "yes" }, null] },
  { finding: { file: "src/b.ts" }, votes: [refuted({ refutationType: "PRE_DECIDED" }), refuted({ introducedByPr: undefined }), refuted({ contradiction: { file: "src/b.ts", quote: "  " } })] },
  { finding: { file: "src/c.ts" }, skipped: true, votes: [refuted()] },
  { finding: { file: "src/d.ts" }, votes: [refuted({ contradiction: { quote: "await db.save(row);" } })] },
];

console.log("\n--- which votes are looked up ---");
const picked = Q.checkableVoteQuotes(states);
check("only v2 REFUTED, non-PRE_DECIDED votes with a quote, on panels that ran", picked.length === 2 && picked[0].s === 0 && picked[0].i === 0 && picked[1].s === 3, JSON.stringify(picked.map((p) => [p.s, p.i])));
check("a quote with no file is looked up in the finding's file", picked[1].anchor.file === "src/d.ts" && picked[1].anchor.line === null);
check("no states, no lookups", Q.checkableVoteQuotes(null).length === 0);

console.log("\n--- a lookup result, as quoteFound ---");
const v = Q.voteQuoteVerdicts(
  [
    { findingIndex: 0, status: "found" },
    { findingIndex: 1, status: "found-shifted" },
    { findingIndex: 2, status: "not-found" },
    { findingIndex: 3, status: "file-not-at-ref" },
    { findingIndex: 4, status: "malformed" },
    { findingIndex: 9, status: "not-found" },
  ],
  5,
);
check("found and found-shifted are true, not-found and file-not-at-ref false, anything else unset", JSON.stringify(v.found) === JSON.stringify([true, true, false, false, null]) && !v.brokenLookup, JSON.stringify(v.found));
const broken = Q.voteQuoteVerdicts([{ findingIndex: 0, status: "not-found" }, { findingIndex: 1, status: "lookup-failed" }], 2);
check("one failed lookup marks nothing", broken.brokenLookup && broken.found.every((f) => f === undefined));
const allMissing = Q.voteQuoteVerdicts([{ findingIndex: 0, status: "file-not-at-ref" }, { findingIndex: 1, status: "file-not-at-ref" }], 2);
check("every file missing at once reads as a broken lookup, not as invented quotes", allMissing.brokenLookup && allMissing.found.every((f) => f === undefined));

console.log("\n--- what the tally does with it ---");
check("a quote not at the commit sets the vote aside", G.factualRefutationFails(refuted({ quoteFound: false })) === "quote-not-at-commit");
check("a quote found counts", G.factualRefutationFails(refuted({ quoteFound: true })) === null);
check("a quote never looked up counts, as before", G.factualRefutationFails(refuted()) === null);

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall vote-quote expectations hold");
