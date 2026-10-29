#!/usr/bin/env node
// replay-ref-evidence.mjs — the detector for a refutation that rests on code
// which is not in the tree it was supposed to be reading.
//
// The pinned worktree PREVENTS a wrong-ref read; this DETECTS one. They are
// different jobs and only the first existed. A refuter that reads the wrong
// checkout kills a real finding while sounding sound, and a killed finding
// leaves no trace in the report — so nothing, live or afterwards, could tell a
// good kill from a wrong-ref kill. The row could not even be audited by hand:
// of 706 votes stored across the whole metrics log, zero carried the reasoning
// text, because the serialiser kept the verdict and dropped the justification.
//
// FIXTURES ARE REAL. Every string below was produced by an actual refuter on
// 2026-09-11 against PR #1911 @7495b91f. The four "phantom" identifiers were
// each verified absent at that commit by reading the blob, and the six sound
// ones verified present — so this replay is scored against the repository, not
// against my reading of it.
//
// Usage: node eval/replay-ref-evidence.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { api, violations } = loadRegion(WORKFLOW, "REF-EVIDENCE-CHECK", [
  "citedIdentifiers",
  "refEvidenceVerdict",
]);
const { citedIdentifiers, refEvidenceVerdict } = api;

console.log(`\nREF-EVIDENCE-CHECK region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

// --- verbatim from the four wrong kills -------------------------------------
const WRONG = [
  "At HEAD the catch block checks `e instanceof RemoteOrderCountSyncError` and prints a distinct 'do NOT decrement the DB count again' message when only OpsHub failed.",
  "Every console.log/console.warn that references prodEmail passes it through `redactEmail()` imported from `./export-sanitize.ts`, which masks all but the first character and domain.",
  "An intra-batch dedupe pass (seenOrderIds Set, lines 616-635) already runs before createMany, pushing a success:false result for any duplicate orderId within the batch.",
];
// --- verbatim from the sound votes ------------------------------------------
const SOUND = [
  "existingOrderIds is computed only from the DB and never deduplicated against itself; OrderDetail.orderId is @unique in schema.prisma so skipDuplicates genuinely drops an intra-batch duplicate row.",
  "main()'s debug block and the Step 2/Step 3 skip warnings print the real prodEmail value on every unmatched row.",
  "writeResults() re-serializes the full cumulative this.results on every call with append=this.config.resume (default true).",
];

console.log(`\n--- what a refutation names ---`);
{
  const a = citedIdentifiers(WRONG[0]);
  check("a PascalCase error type is picked up", a.includes("RemoteOrderCountSyncError"));
  const b = citedIdentifiers(WRONG[1]);
  check("a camelCase helper is picked up", b.includes("redactEmail"));
  check("and so is the file it claims to import from", b.includes("export-sanitize.ts"));
  const c = citedIdentifiers(WRONG[2]);
  check("a camelCase local is picked up", c.includes("seenOrderIds"));
}
{
  const s = citedIdentifiers(SOUND[0]);
  check("sound votes are read the same way", s.includes("existingOrderIds"));
  check("including schema files", s.includes("schema.prisma"));
}

console.log(`\n--- under-extracting on purpose ---`);
{
  // Over-extraction is the failure that matters: a field that cries wolf gets
  // ignored, and then it is worse than absent. These are the shapes ordinary
  // prose produces.
  const prose = citedIdentifiers(
    "The OpsHub call can fail and BillingsHub is not involved; see the GitHub thread. TypeScript types are fine.",
  );
  check("a short product noun is not a citation", !prose.includes("OpsHub"));
  check("nor is a stoplisted platform name", !prose.includes("GitHub") && !prose.includes("TypeScript"));

  // This assertion MOVED on 2026-09-11. It used to demand that prose extract
  // NOTHING, and `BillingsHub` (11 chars, internal capital) failed it. That was the
  // check being wrong, not the extractor: over-extraction only hurts when it
  // produces a FALSE FLAG, and a product name the repo really uses resolves and
  // flags nothing. Asserting zero extraction was asserting a mechanism instead
  // of the property, and it would have forced a stoplist entry per vendor
  // forever.
  const harmless = refEvidenceVerdict({ ran: true, cited: prose, missing: [] });
  check(
    "a vendor name the repo really uses costs nothing",
    harmless.suspect === false,
    JSON.stringify(prose),
  );

  // The harness's own vocabulary is the opposite case and stays a hard rule: it
  // is camelCase, long, present in nearly every refutation, and lives in the
  // review harness rather than the repo under review — so it would be reported
  // missing on every vote and kill the field's credibility in one run.
  const verdictWords = citedIdentifiers(
    "Returning NOT_REFUTED with refutationType NO_REFUTATION, killReason other, slotStatus OK.",
  );
  check(
    "the harness's own vocabulary is never mistaken for repo code",
    verdictWords.length === 0,
    JSON.stringify(verdictWords),
  );
}

console.log(`\n--- the three states, kept apart ---`);
{
  const notRun = refEvidenceVerdict({ ran: false, cited: ["x"], missing: [] });
  check("a check that did not run says so", notRun.ran === false);
  // The vacuous pass: "did not run" must never read as "found nothing wrong".
  check("and does NOT report a clean result", notRun.suspect === null);

  const clean = refEvidenceVerdict({ ran: true, cited: ["existingOrderIds"], missing: [] });
  check("a vote citing only real code is not suspect", clean.suspect === false);
  check("and records how much it cited", clean.cited === 1);

  const dirty = refEvidenceVerdict({ ran: true, cited: ["seenOrderIds", "newOrders"], missing: ["seenOrderIds"] });
  check("a vote citing absent code IS suspect", dirty.suspect === true);
  check("and names what was missing, not just a count", dirty.missing.includes("seenOrderIds"));
  check("a null in the missing list is discarded, not counted", refEvidenceVerdict({ ran: true, cited: ["a"], missing: [null, ""] }).suspect === false);
}

console.log(`\n--- scored against the real repository ---`);
{
  // Verified by reading the blobs at 7495b91f: absent there, present in the
  // ambient checkout. This is the discrimination the detector exists for.
  const PHANTOM = new Set(["RemoteOrderCountSyncError", "redactEmail", "seenOrderIds", "export-sanitize.ts"]);
  const flagged = WRONG.map((t) => {
    const cited = citedIdentifiers(t);
    return refEvidenceVerdict({ ran: true, cited, missing: cited.filter((i) => PHANTOM.has(i)) });
  });
  check("all three wrong kills are flagged", flagged.every((f) => f.suspect === true), JSON.stringify(flagged.map((f) => f.missing)));

  const quiet = SOUND.map((t) => {
    const cited = citedIdentifiers(t);
    return refEvidenceVerdict({ ran: true, cited, missing: cited.filter((i) => PHANTOM.has(i)) });
  });
  check("and no sound vote is", quiet.every((q) => q.suspect === false), JSON.stringify(quiet.map((q) => q.missing)));
}

console.log(`\n--- the evidence is actually kept this time ---`);
{
  const fs = await import("node:fs");
  const src = fs.readFileSync(WORKFLOW, "utf8");
  // The whole detector is worthless if the row keeps dropping the text, which
  // is exactly how refutationType was lost for every finding ever recorded.
  // Anchored on `discardedVotes`, which is unique to the METRICS row's vote
  // serialiser. The first version of this check matched `votes: v.votes.map`
  // anywhere and passed before the field had been added at all — it was
  // resolving against a DIFFERENT votes.map (the synthesis payload, which
  // already kept the text). A check that green-lights the change it is meant to
  // guard, before the change exists, is worse than no check.
  check(
    "the METRICS row's vote serialiser carries the reasoning text",
    /discardedVotes[\s\S]{0,300}votes:\s*v\.votes\.map\(\(vote\)\s*=>\s*\(\{[\s\S]{0,1400}reason:\s*vote\.reason/.test(src),
    "without this the detector can never be run over a past row",
  );
  check("and the run records the audit's own result", /^\s{2}refEvidence,$/m.test(src));
  // The check must run BEFORE synthesis or its result cannot reach the document
  // people actually read — it would land only in a metrics row nobody opens.
  check(
    "the audit runs before the report is synthesised",
    src.indexOf("let refEvidence =") < src.indexOf("let report = await agent("),
  );
  check(
    "a suspect vote reaches the posted report, not just the row",
    /report = withBanners\([\s\S]{0,220}refEvidenceBanner\(\)/.test(src) &&
      /const reportAsPosted = stripDraftCommentsFromBody\(report\);/.test(src),
  );
  check(
    "and the banner names the missing identifiers rather than a count",
    /s\.missing\.map\(/.test(src),
  );
  // A banner that fires when the check never ran would be worse than silence.
  check(
    "the banner stays silent when the audit did not run",
    /if \(!refEvidence\.ran \|\| refEvidence\.suspectVotes === 0\) return "";/.test(src),
  );
}

console.log("");
if (failures.length) {
  console.log(`replay-ref-evidence: ${failures.length} FAILURE(S)`);
  for (const f of failures) console.log(`  - ${f}`);
  process.exit(1);
}
console.log("replay-ref-evidence: all checks passed");
