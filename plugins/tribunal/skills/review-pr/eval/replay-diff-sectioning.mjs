#!/usr/bin/env node
// replay-diff-sectioning.mjs — the per-file diff splitter and the cited-file
// scope widener, tested against the shapes real patches actually take.
//
// Why this needs its own replay. Sectioning exists to stop handing every agent
// the whole patch (11 receivers x a 14-file diff on PR #1968; ~100 on #1911),
// and its failure mode is silent by construction: a file that drops out of a
// lens's section set produces no error, no warning and no finding — the lens
// simply never sees the code and the run looks clean. Nothing downstream can
// notice. So the guarantee has to be established here, before dispatch: the
// splitter is TOTAL over the PR's file list, or peer-review.js declines to
// section at all and everyone gets the full diff as before.
//
// Like eval/replay-gates.mjs, this evaluates the REAL code sliced out of
// workflows/peer-review.js by its markers — never a copy.
//
// Usage: node eval/replay-diff-sectioning.mjs

import { resolve } from "node:path";
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

const { api, violations } = loadRegion(WORKFLOW, "DIFF-SECTIONING", [
  "splitDiffByFile",
  "citedFiles",
  "stripCodeFence",
  "addedLines",
  "addedRanges",
  "anyLocationAdded",
]);
const { splitDiffByFile, citedFiles, stripCodeFence, addedLines, addedRanges, anyLocationAdded } = api;

console.log(`\nDIFF-SECTIONING region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const hunk = (path, body) =>
  `diff --git a/${path} b/${path}
index 1111111..2222222 100644
--- a/${path}
+++ b/${path}
@@ -1,3 +1,4 @@
${body}`;

console.log(`\n--- added lines (the pre-existing veto, v91) ---`);
{
  const section = hunk("src/a.ts", " a\n-b\n+c\n+d\n e") + "\n@@ -10,2 +11,3 @@\n x\n+++y\n+z";
  const lines = [...addedLines(section)].join(",");
  check("header lines before the first hunk are not counted, and both hunks are", lines === "2,3,12,13", lines);
  check("an added line that starts with ++ is still an added line", addedLines(section).has(12));
  const runs = JSON.stringify(addedRanges(section));
  check("ranges merge consecutive lines", runs === "[[2,3],[12,13]]", runs);
  check("a deletion-only section adds nothing", addedRanges(hunk("src/b.ts", " a\n-b\n c")).length === 0);
}

console.log(`\n--- splitter: totality ---`);
{
  const diff = [
    hunk("src/services/v2/orders.ts", "+const a = 1;"),
    hunk("src/jobs/orders_worker.ts", "+const b = 2;"),
    hunk("prisma/schema.prisma", "+model Foo {}"),
  ].join("\n");
  const { sections, unparsed } = splitDiffByFile(diff);
  check("every file gets a section", sections.size === 3, `${sections.size}`);
  check("no unparsed headers", unparsed.length === 0);
  check(
    "a section carries its own hunk and nothing else",
    sections.get("src/jobs/orders_worker.ts").includes("+const b = 2;") &&
      !sections.get("src/jobs/orders_worker.ts").includes("+const a = 1;"),
  );
  check(
    "concatenating every section loses no line",
    [...sections.values()].join("\n").split("\n").length ===
      diff.split("\n").length,
  );
}
{
  // A rename: `a/<old> b/<new>`. prContext.files reports the POST-image name,
  // so the section has to be keyed by the b-side or the file looks unsectioned
  // and the whole run falls back — a silent loss of the saving, not of safety.
  const diff = `diff --git a/src/old-name.ts b/src/new-name.ts
similarity index 92%
rename from src/old-name.ts
rename to src/new-name.ts
--- a/src/old-name.ts
+++ b/src/new-name.ts
@@ -1 +1 @@
-const x = 1;
+const x = 2;`;
  const { sections } = splitDiffByFile(diff);
  check("rename is keyed by the post-image name", sections.has("src/new-name.ts"));
  check("rename is NOT keyed by the pre-image name", !sections.has("src/old-name.ts"));
}
{
  // A quoted path (spaces / control chars). Guessing the split point here is
  // how a file silently ends up in the wrong section, so the header is refused
  // and reported — which makes sectioningAvailable false for the whole run.
  const diff = `diff --git "a/src/a b.ts" "b/src/a b.ts"
--- "a/src/a b.ts"
+++ "b/src/a b.ts"
@@ -1 +1 @@
+x`;
  const { sections, unparsed } = splitDiffByFile(diff);
  check("a quoted header is refused, not guessed", unparsed.length === 1);
  check("a refused header contributes no section", sections.size === 0);
}
{
  const { sections, unparsed } = splitDiffByFile("");
  check("empty diff yields nothing and does not throw", sections.size === 0 && unparsed.length === 0);
}
{
  // Content that merely LOOKS like a header, inside a hunk body. A diff of a
  // file that itself contains diffs (a fixture, a doc) must not split there —
  // the line would be prefixed by +/-/space in a real patch, never bare.
  const diff = hunk(
    "eval/fixtures/sample.patch",
    "+ diff --git a/fake.ts b/fake.ts\n+ more",
  );
  const { sections } = splitDiffByFile(diff);
  check(
    "an indented/prefixed header line does not split a section",
    sections.size === 1 && sections.has("eval/fixtures/sample.patch"),
    `${[...sections.keys()].join(",")}`,
  );
}

console.log(`\n--- cited files: scope widening for cross-file findings ---`);
const KNOWN = [
  "src/controllers/v2/partner.ts",
  "src/jobs/orders_worker.ts",
  "prisma/schema.prisma",
];
{
  // The PR #1968 shape: the finding only bites because of a guard in a sibling
  // file. Confining the refuter to partner.ts and telling it to default to
  // REFUTED guarantees a refutation — this is the widening that stops it.
  const f = {
    file: "src/controllers/v2/partner.ts",
    description:
      "The worker's 'failed' listener in orders_worker.ts flips the row to error on the first retry.",
    evidence: "if (isFinalAttempt) { ... }",
  };
  const cited = citedFiles(f, KNOWN);
  check("a cited sibling file is returned", cited.includes("src/jobs/orders_worker.ts"));
  check("the finding's own file is not repeated", !cited.includes(f.file));
}
{
  const f = {
    file: "src/controllers/v2/partner.ts",
    description: "See totally-made-up.ts and node_modules/whatever.js for the guard.",
    evidence: "",
  };
  check(
    "a filename not in this PR cannot widen scope",
    citedFiles(f, KNOWN).length === 0,
    JSON.stringify(citedFiles(f, KNOWN)),
  );
}
{
  const f = {
    file: "src/controllers/v2/partner.ts",
    description: "The column added in schema.prisma is nullable, so the write can pass null.",
    evidence: "",
  };
  check(
    "a basename resolves to its full path in the PR",
    citedFiles(f, KNOWN).includes("prisma/schema.prisma"),
  );
}
{
  const f = { file: "a.ts", description: "", evidence: "" };
  check("no prose yields no citations", citedFiles(f, KNOWN).length === 0);
  check("a null file list does not throw", citedFiles(f, null).length === 0);
}
{
  const f = {
    file: "src/controllers/v2/partner.ts",
    description:
      "orders_worker.ts and orders_worker.ts again, plus src/jobs/orders_worker.ts",
    evidence: "orders_worker.ts",
  };
  check(
    "repeated citations are deduplicated",
    citedFiles(f, KNOWN).length === 1,
    JSON.stringify(citedFiles(f, KNOWN)),
  );
}

console.log(`\n--- stripCodeFence: decorated JSON from a free-text agent ---`);
{
  // The exact payload the evidence resolver returned on the 2026-07-24 PR #1935
  // Tier-1 run. JSON.parse threw on it, the best-effort catch swallowed the
  // failure, and 3 resolved candidates were recorded as "resolver unavailable".
  const real =
    '```json\n{"ran":true,"results":[{"id":0,"matchKind":"not-found"},{"id":1,"matchKind":"shifted"}]}\n```';
  const parsed = JSON.parse(stripCodeFence(real));
  check("the real PR #1935 fenced payload parses", parsed.ran === true && parsed.results.length === 2);
}
{
  check(
    "unfenced JSON is returned untouched",
    JSON.parse(stripCodeFence('{"ran":true}')).ran === true,
  );
  check(
    "a bare ``` fence (no language tag) is stripped",
    JSON.parse(stripCodeFence('```\n{"ran":false}\n```')).ran === false,
  );
  check(
    "surrounding whitespace does not defeat the fence check",
    JSON.parse(stripCodeFence('\n\n  ```json\n{"a":1}\n```  \n')).a === 1,
  );
  check(
    "an unterminated fence still yields its body",
    JSON.parse(stripCodeFence('```json\n{"a":2}')).a === 2,
  );
  check("empty input does not throw", stripCodeFence("") === "");
  check("null input does not throw", stripCodeFence(null) === "");
  check(
    "a fenced payload containing ``` inside a string keeps the last fence as the terminator",
    JSON.parse(stripCodeFence('```json\n{"s":"a ``` b"}\n```')).s === "a ``` b",
  );
}

{
  // 2026-10-19: the merged headline sat on an unchanged
  // line and its member on an added one, and the real finding was set aside.
  const sections = new Map([["src/a.ts", { added: [[2, 2], [10, 12]] }]]);
  check("the veto sees the headline's own added line", anyLocationAdded(sections, { file: "src/a.ts", line: 11 }));
  check("an unchanged headline with no members is not vetoed", !anyLocationAdded(sections, { file: "src/a.ts", line: 3 }));
  check(
    "an unchanged headline whose merged member is on an added line is vetoed",
    anyLocationAdded(sections, { file: "src/a.ts", line: 3, mergedLocations: [{ file: "src/a.ts", line: 3 }, { file: "src/a.ts", line: 2 }] }),
  );
  check(
    "members on unchanged lines, other files or no line do not veto",
    !anyLocationAdded(sections, { file: "src/a.ts", line: 3, mergedLocations: [{ file: "src/b.ts", line: 2 }, { file: "src/a.ts", line: null }] }),
  );
  // BE #1951 (2026-10-19): the lens cited :604, the quote sits on added :704.
  const quoted = (r) => ({ file: "src/a.ts", line: 3, evidenceResolution: { evidenceLocated: true, resolvedFile: "src/a.ts", resolvedLine: 11, ambiguousCount: 1, ...r } });
  check("a quote the resolver found on an added line vetoes, whatever line is claimed", anyLocationAdded(sections, quoted({ matchedLineKind: "added" })));
  check("a quote found on a context line does not", !anyLocationAdded(sections, quoted({ matchedLineKind: "context" })));
  check("an ambiguous quote does not", !anyLocationAdded(sections, quoted({ matchedLineKind: "added", ambiguousCount: 2 })));
  check("a quote found in another file does not", !anyLocationAdded(sections, quoted({ matchedLineKind: "added", resolvedFile: "src/b.ts" })));
}

console.log(`
What this does NOT establish:
  · It tests the splitter and the scope widener, not the ROUTING built on them
    (scopedDiffBody / lensDiffBlockFor / refuterDiffBlockFor close over run
    state and cannot be extracted). That every recipient really receives its own
    section is Tier-1: read diffExposure.sectionedReceivers on a micro-run.
  · It says nothing about whether a sectioned lens finds what a full-diff lens
    would have found. Sectioning only removes files the lens was already told to
    ignore, but that is an argument, not a measurement.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
