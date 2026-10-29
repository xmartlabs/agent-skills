#!/usr/bin/env node
// replay-refuter-package.mjs — pin the coverage measurement's own arithmetic.
//
// eval/refuter-package.mjs decides whether the single largest in-run lever is
// worth a Tier-1 agent, and it decided it twice with different answers, because
// three normalisation bugs each biased coverage DOWNWARD:
//
//   1. the PR's changed-file list was approximated from the row's finding paths
//      (a much shorter list), which handicapped the one variant that models what
//      the harness computes today;
//   2. `file:line` suffixes were kept, so a finding's own file resolved against
//      no tree and became an uncovered read AND an unsized package entry;
//   3. reads were compared un-canonicalised, so `cancel-orders.ts` and
//      `scripts/orders/cancel-orders.ts` were treated as different files —
//      the same file on both sides, scored as a miss.
//
// Coverage went 35% → 55% on the deciding variant across those three fixes,
// which is the difference between "not worth trying" and "worth one agent". The
// asserts below are the ones that would have caught each.
//
// Usage: node eval/replay-refuter-package.mjs [--verbose]

import {
  citedTokens,
  normalizeReadPath,
  extractReads,
  resolveToken,
  assemblePackage,
  scoreCoverage,
  extractRangedReads,
  distanceFromLine,
  VARIANTS
} from "./refuter-package.mjs";

const VERBOSE = process.argv.includes("--verbose");
const failures = [];
let checks = 0;

function eq(label, got, want) {
  checks++;
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g === w) {
    if (VERBOSE) console.log(`  ok   ${label}`);
    return;
  }
  failures.push(`${label}\n       got  ${g}\n       want ${w}`);
}
const ok = (label, cond) => eq(label, !!cond, true);

// --- 1. path normalisation ---------------------------------------------------

eq("line suffix stripped", normalizeReadPath("scripts/orders/cancel-orders.ts:233"), "scripts/orders/cancel-orders.ts");
eq("line range stripped", normalizeReadPath("src/a.ts:10-40"), "src/a.ts");
eq("absolute path reduced to repo-relative", normalizeReadPath("/Users/x/repo/src/services/v2/orders.ts"), "src/services/v2/orders.ts");
eq("git show ref:path", normalizeReadPath("37e7fbea:prisma/schema.prisma"), "prisma/schema.prisma");
eq("bare basename with an extension is kept", normalizeReadPath("schema.prisma"), "schema.prisma");
// Extensions are required. The looser pattern used by the redundancy count
// admits non-files and inflates it (43% vs 37% on the same data); here a
// non-file is not a miss, it is noise, and must not be scored either way.
eq("a directory is not a read", normalizeReadPath("src/services/v2"), null);
eq("a flag is not a read", normalizeReadPath("--no-verify"), null);
eq("empty", normalizeReadPath(""), null);

// --- 2. read extraction ------------------------------------------------------

const line = (content) => JSON.stringify({ message: { content } });
const transcript = [
  line([{ type: "tool_use", name: "Read", input: { file_path: "/repo/src/services/v2/orders.ts" } }]),
  line([{ type: "tool_use", name: "Read", input: { file_path: "/repo/src/services/v2/orders.ts" } }]),
  line([
    {
      type: "tool_use",
      name: "Bash",
      input: { command: "git show abc123:prisma/schema.prisma | grep -n 'model Account'" }
    }
  ]),
  // One bash call touching two files: taking only the first match undercounts
  // what the agent needed, which is the number this whole module turns on.
  line([
    {
      type: "tool_use",
      name: "Bash",
      input: { command: "sed -n '1,50p' src/a.ts && sed -n '1,50p' src/b.ts" }
    }
  ]),
  line("a plain string message, not a tool call"),
  "{ not json"
].join("\n");

const reads = extractReads(transcript);
eq("reads: one entry per distinct path", reads.map((r) => r.path), [
  "src/services/v2/orders.ts",
  "prisma/schema.prisma",
  "src/a.ts",
  "src/b.ts"
]);
eq("reads: repeat access is counted, not deduped", reads[0].count, 2);
eq("reads: both files of one bash call are captured", reads.slice(2).map((r) => r.count), [1, 1]);
eq("reads: malformed lines are skipped, not fatal", extractReads("{oops\n" + transcript).length, 4);

// --- 3. token resolution -----------------------------------------------------

const REPO_FILES = [
  "src/services/v2/orders.ts",
  "src/services/v2/orders.update.ts",
  "src/lib/helpers/utilities.ts",
  "src/legacy/utilities.ts",
  "prisma/schema.prisma",
  "scripts/orders/cancel-orders.ts"
];

eq("resolve: exact", resolveToken("prisma/schema.prisma", REPO_FILES), "prisma/schema.prisma");
eq("resolve: unique suffix", resolveToken("cancel-orders.ts", REPO_FILES), "scripts/orders/cancel-orders.ts");
// Two files share this basename. Guessing the first would credit the package
// with a file the refuter never opened — an invented coverage point.
eq("resolve: ambiguous suffix resolves to nothing", resolveToken("utilities.ts", REPO_FILES), null);
eq("resolve: unknown", resolveToken("nope.ts", REPO_FILES), null);

eq(
  "citedTokens: paths with extensions only, deduped",
  citedTokens("see src/services/v2/orders.ts and src/services/v2/orders.ts plus src/services/v2 and README"),
  ["src/services/v2/orders.ts"]
);

// --- 4. the assembler --------------------------------------------------------

const FINDING = {
  title: "orders.update.ts drops the flag",
  file: "src/services/v2/orders.update.ts:120",
  description:
    "The caller in src/services/v2/orders.ts passes it, but prisma/schema.prisma declares the column as optional."
};
const PR_FILES = ["src/services/v2/orders.update.ts"];

const inPr = assemblePackage(FINDING, { prFiles: PR_FILES, repoFiles: REPO_FILES, variant: "cited-in-pr" });
// The own file must be present WITHOUT its line suffix, or it resolves against
// no tree and the package silently loses its most important entry.
ok("assemble: own file present, line stripped", inPr.paths.includes("src/services/v2/orders.update.ts"));
eq("assemble: cited-in-pr stops at the PR boundary", inPr.paths.length, 1);

const anywhere = assemblePackage(FINDING, { prFiles: PR_FILES, repoFiles: REPO_FILES, variant: "cited-anywhere" });
ok("assemble: cited-anywhere reaches a non-PR call site", anywhere.paths.includes("src/services/v2/orders.ts"));
ok("assemble: and the cited schema", anywhere.paths.includes("prisma/schema.prisma"));

const withSchema = assemblePackage(
  { title: "column is nullable", file: "src/a.ts", description: "the enum in the schema says otherwise" },
  { prFiles: [], repoFiles: REPO_FILES, variant: "cited-anywhere+schema" }
);
ok("assemble: a prisma-shaped claim pulls in schema.prisma unprompted", withSchema.paths.includes("prisma/schema.prisma"));
const noSchema = assemblePackage(
  { title: "off-by-one in a loop", file: "src/a.ts", description: "the index starts at 1" },
  { prFiles: [], repoFiles: REPO_FILES, variant: "cited-anywhere+schema" }
);
// 85 KB / ~21k tokens is more than the rest of a package put together, and the
// measurement says the biggest variant LOSES money per turn. Unconditional
// inclusion is the mistake this guard exists to prevent.
ok("assemble: a non-prisma claim does not", !noSchema.paths.includes("prisma/schema.prisma"));

eq("assemble: variants are declared, not implied", VARIANTS.length, 3);
// A typo must be loud. Before this guard the variant name was only compared
// against "cited-in-pr", so any unrecognised string silently produced the WIDEST
// package — reporting one variant's numbers under another's name.
ok(
  "assemble: an unknown variant throws instead of silently widening",
  (() => {
    try {
      assemblePackage(FINDING, { prFiles: PR_FILES, repoFiles: REPO_FILES, variant: "nonsense" });
      return false;
    } catch (e) {
      return /unknown package variant/.test(e.message);
    }
  })()
);

// --- 5. coverage scoring -----------------------------------------------------

const score = scoreCoverage(
  ["src/a.ts", "src/b.ts"],
  [
    { path: "src/a.ts", count: 6 },
    { path: "src/c.ts", count: 1 }
  ]
);
// Accesses, not distinct files: a file opened six times is six turns the package
// removes, and counting it once understates the mechanism being measured.
eq("score: accesses counted, not files", [score.accesses, score.accessesCovered], [7, 6]);
eq("score: distinct counted too", [score.distinctRead, score.distinctCovered], [2, 1]);
eq("score: what is missing is named", score.outOfPackage, ["src/c.ts"]);
eq("score: not fully covered", score.fullyCovered, false);
eq(
  "score: fully covered",
  scoreCoverage(["src/a.ts"], [{ path: "src/a.ts", count: 2 }]).fullyCovered,
  true
);
// An agent that read nothing is not 100% covered — it is outside the population,
// and folding it in as a success would inflate "fully covered" for free.
const nothing = scoreCoverage(["src/a.ts"], []);
eq("score: read nothing is flagged, not scored as perfect", [nothing.readNothing, nothing.fullyCovered], [true, false]);

// --- 6. ranged reads (the region question) ----------------------------------

const rangedTranscript = [
  line([{ type: "tool_use", name: "Read", input: { file_path: "src/a.ts", offset: 100, limit: 50 } }]),
  // No offset: a whole-file read says nothing about WHICH region was needed, so
  // it must not become a range starting at line 1.
  line([{ type: "tool_use", name: "Read", input: { file_path: "src/a.ts" } }]),
  line([{ type: "tool_use", name: "Bash", input: { command: "sed -n '200,260p' src/b.ts" } }]),
  // grep -n is the LOCATE step, not a read of content — 1,827 of them in the
  // corpus, the single most common call. Counting one as a read of the line it
  // reports would credit a region package with content the agent never got.
  line([{ type: "tool_use", name: "Bash", input: { command: "grep -n 'foo' src/c.ts" } }])
].join("\n");

const rr = extractRangedReads(rangedTranscript);
eq("ranged: only the two real ranges", rr.map((r) => [r.path, r.start, r.end]), [
  ["src/a.ts", 100, 149],
  ["src/b.ts", 200, 260]
]);

eq("distance: inside the window is 0", distanceFromLine({ start: 100, end: 150 }, 120), 0);
eq("distance: above the window", distanceFromLine({ start: 100, end: 150 }, 40), 60);
eq("distance: below the window", distanceFromLine({ start: 100, end: 150 }, 200), 50);
eq("distance: boundaries count as inside", [distanceFromLine({ start: 100, end: 150 }, 100), distanceFromLine({ start: 100, end: 150 }, 150)], [0, 0]);
// A finding with no line cannot be measured against a region — null, never 0,
// because 0 would read as "the window already contained it".
eq("distance: no line is unmeasurable, not a hit", distanceFromLine({ start: 1, end: 9 }, 0), null);

console.log(
  failures.length
    ? `\nreplay-refuter-package: ${checks - failures.length}/${checks} checks pass\n\n` +
        failures.map((f) => `  FAIL ${f}`).join("\n") +
        "\n"
    : `\nreplay-refuter-package: ${checks}/${checks} checks pass\n`
);
process.exit(failures.length ? 1 : 0);
