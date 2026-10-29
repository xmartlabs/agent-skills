#!/usr/bin/env node
// replay-refine-attribution.mjs — pin the round-0 reconstruction.
//
// eval/refine-attribution.mjs rebuilds the tally the refine gate saw, from
// prompts and journal returns still on disk. Everything it concludes rests on
// parsers that are pinned to peer-review.js's exact wording, and on one
// non-obvious bridge (a refined finding is recorded under its STRENGTHENED
// title, so the row's identity is not the identity round 0 voted on). Both are
// silent failure modes: a reworded prompt makes a parser return nothing, and
// "nothing" reads as "this run had no votes" rather than as an error.
//
// Every prompt sample below is copied from a real transcript, including the two
// external-wrapper generations, because the older one is most of the external
// votes in the corpus and requiring a slot number is what made 102 of them
// vanish — each time dropping the third voter on a 3-vote panel.
//
// Usage: node eval/replay-refine-attribution.mjs [--verbose]

import {
  parseRefuterBatchCases,
  parseSingleRefuterCase,
  parseExternalSlot,
  parseStrengthenTarget,
  parseEmbeddedRow,
  votesFromResult,
  assembleTimeline,
  lookupFinding,
  roundZeroVotes,
  attributeRefine,
  findingKey,
  normFile
} from "./refine-attribution.mjs";

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

function ok(label, cond) {
  checks++;
  if (cond) {
    if (VERBOSE) console.log(`  ok   ${label}`);
    return;
  }
  failures.push(label);
}

// --- 1. batched Claude refuter -----------------------------------------------

const BATCH_PROMPT = `You are refuting 3 INDEPENDENT code-review findings on PR #1911 in repo acme/api, one lens applied to each. REFUTE — do not confirm — each one.

Lens (same for every case below): correctness/falsification

===== CASE 1 of 3 — slot 8 =====
Finding: Account.updatedAt not bumped when currentYearOrderCount is updated
File: scripts/orders/reverse-cancelled-order.ts:438
Severity: important
Description: prisma/schema.prisma declares Account.updatedAt as \`@default(now())\`.

===== CASE 2 of 3 — slot 9 =====
Finding: --batch-size with a non-numeric value silently no-ops the run
File: scripts/invoices/bulk-rebuild-invoices.ts:344
Severity: important
Description: \`config.batchSize = Number(value)\` is never validated.

===== CASE 3 of 3 — slot 12 =====
Finding: Required-column validation checks the wrong header name
File: scripts/orders/carry-over/generate-carryover-order-import.ts:522
Severity: nit
Description: requiredColumns includes 'Associated Contact'.`;

const cases = parseRefuterBatchCases(BATCH_PROMPT);
eq("batch: three cases parsed", cases.length, 3);
eq("batch: slotIds in order", cases.map((c) => c.slotId), [8, 9, 12]);
eq("batch: second title", cases[1].title, "--batch-size with a non-numeric value silently no-ops the run");
eq("batch: third file keeps its line", cases[2].file, "scripts/orders/carry-over/generate-carryover-order-import.ts:522");
eq("batch: severity", cases.map((c) => c.severity), ["important", "important", "nit"]);

// A slotId is a FINDING id, not a vote id — peer-review.js keys
// escalationVoteBySlot and claudeRevoteBySlot by `finding.slotId`. Non-contiguous
// numbering across a batch is therefore normal and must not be renumbered.
ok("batch: slots are not assumed contiguous", cases[2].slotId === 12);

eq("batch: a prompt with no CASE blocks yields nothing", parseRefuterBatchCases("REFUTE this"), []);

// --- 2. unbatched Claude refuter ---------------------------------------------

const SINGLE_PROMPT = `REFUTE — do not confirm — this code review finding on PR #1935 in repo acme/api.
The PR diff (already fetched):
---
diff --git a/src/lib/helpers/utilities.test.ts b/src/lib/helpers/utilities.test.ts
+++ b/src/lib/helpers/utilities.test.ts
---
Lens: correctness/falsification

Finding: getStateCode assumes its input is always a string
File: src/lib/middleware/partnerMiddleware.ts:137
Severity: nit
Description: getStateCode calls \`state.trim()\` directly.`;

const single = parseSingleRefuterCase(SINGLE_PROMPT);
eq("single: title", single.title, "getStateCode assumes its input is always a string");
eq("single: file", single.file, "src/lib/middleware/partnerMiddleware.ts:137");
eq("single: no slot is claimed", single.slotId, null);

// The diff is embedded in this prompt, so the anchor has to be the
// Finding/File/Severity TRIPLE. A lone "Finding:" line inside a diff hunk (a PR
// that edits this very harness, say) must not be mistaken for the case block.
const DIFF_DECOY = `REFUTE — do not confirm — this code review finding on PR #1 in repo x/y.
---
+Finding: this line lives inside the diff
+some other line
---
Finding: the real one
File: src/a.ts:1
Severity: nit
Description: real.`;
eq("single: a 'Finding:' line inside the diff does not win", parseSingleRefuterCase(DIFF_DECOY).title, "the real one");
eq("single: no triple means no case", parseSingleRefuterCase("REFUTE this finding, no block here"), null);

// --- 3. external wrappers, both generations ----------------------------------

const EXT_CURRENT = `You are a model-diverse refuter (Codex/GPT slot) on a peer-review panel.

Step 1 — run: mkdir -p "\${TMPDIR:-/tmp}/peer-review-votes-1911"   then write \${TMPDIR:-/tmp}/peer-review-votes-1911/21-codex.prompt with this exact content:
---
REFUTE — do not confirm — this code review finding.

Finding: toNumber(...) || undefined discards a legitimate $0 contingency fee
File: scripts/orders/carry-over/import-orders-from-partner-csv.ts:369
Description: collapses 0 to undefined.`;

const extNew = parseExternalSlot(EXT_CURRENT);
eq("external/current: slot", extNew.slotId, 21);
eq("external/current: source", extNew.source, "codex");
eq("external/current: title", extNew.title, "toNumber(...) || undefined discards a legitimate $0 contingency fee");

const EXT_LEGACY = `You are a model-diverse refuter (Codex/GPT slot) on a peer-review panel.

Step 1 — write /tmp/codex_refute_prompt.txt with this exact content:
---
REFUTE — do not confirm — this code review finding.

Finding: Concurrency hardcoded to 1 for every worker
File: src/jobs/worker.ts:32
Description: The PR deletes JOBS_CONCURRENCY from the schema.`;

const extOld = parseExternalSlot(EXT_LEGACY);
// Guarded with `?.` on purpose: when this regresses, the parser returns null,
// and an unguarded read would crash the file before the summary line — turning
// a clean "1 of 48 failed" into a stack trace that hides the other 47.
ok("external/legacy: parses at all — the slot is optional", extOld !== null);
// `?:` and not `?.` + `??` — the expected slotId here IS null, and `??` would
// swallow the real value into the failure placeholder.
eq("external/legacy: no slot to recover", extOld ? extOld.slotId : "PARSE FAILED", null);
eq("external/legacy: source falls back to the header", extOld ? extOld.source : "PARSE FAILED", "codex");
eq(
  "external/legacy: title",
  extOld ? extOld.title : "PARSE FAILED",
  "Concurrency hardcoded to 1 for every worker"
);

const EXT_GEMINI = `You are a model-diverse refuter (Gemini slot) on a peer-review panel.

Step 1 — write /tmp/gemini_refute_prompt.txt with this exact content:
---
Finding: something
File: src/b.ts:2
Description: d.`;
eq("external/legacy: Gemini header gives the right source", parseExternalSlot(EXT_GEMINI).source, "gemini");

// Source matters beyond bookkeeping: tallyVotes caps a gemini vote's weight at
// GEMINI_CONFIDENCE_CAP, so mislabelling one as codex changes the tally.
ok("external: source is never guessed as claude", parseExternalSlot(EXT_GEMINI).source !== "claude");

// --- 4. strengthen -----------------------------------------------------------

const STRENGTHEN_PROMPT = `A code-review finding on PR #1911 (repo acme/api) was nearly refuted, but the CORE defect appears real — only its claim/severity/repro was weak. STRENGTHEN it.

Original finding: Audit trail attributes every action to an arbitrary SuperAdmin
File: scripts/contacts/dedupe-contacts.ts:384
Severity: important
Description: getAdminContactId() does findFirst with no ordering.
Refuters' objections: Verified against schema.prisma`;

const st = parseStrengthenTarget(STRENGTHEN_PROMPT);
eq("strengthen: original title", st.title, "Audit trail attributes every action to an arbitrary SuperAdmin");
eq("strengthen: original file", st.file, "scripts/contacts/dedupe-contacts.ts:384");
// "Original finding:" not "Finding:" — a strengthen prompt has no "Finding:"
// line, so reusing the refuter parser on it silently matches nothing.
eq("strengthen: refuter parser does not match a strengthen prompt", parseSingleRefuterCase(STRENGTHEN_PROMPT), null);

// --- 5. journal results ------------------------------------------------------

eq(
  "result: batch shape demuxes by slotId",
  votesFromResult({
    verdicts: [
      { slotId: 8, verdict: "NOT_REFUTED", coreStands: true },
      { slotId: 9, verdict: "REFUTED" }
    ]
  }).map((v) => v.slotId),
  [8, 9]
);
eq(
  "result: single shape has no slot",
  votesFromResult({ verdict: "REFUTED", confidence: 0.9, source: "codex" }).map((v) => v.slotId),
  [null]
);
eq("result: null", votesFromResult(null), []);
eq("result: a strengthen return is not a vote", votesFromResult({ title: "x", severity: "nit" }), []);
eq(
  "result: a verdict entry without a numeric slotId keeps null rather than 0",
  votesFromResult({ verdicts: [{ verdict: "REFUTED" }] })[0].slotId,
  null
);

// --- 6. embedded row --------------------------------------------------------

const PERSIST_PROMPT = `Run a sequence of scripts EXACTLY as given, in order.

Run EXACTLY this bash:
cat <<'METRICSJSON' > "/tmp/peer-review-votes-1935/metrics.json"
{"schemaVersion":3,"ts":null,"pr":1935,"headSha":"a75eb53d","paneled":1,"findings":[{"title":"a","file":"src/a.ts","refined":false}]}
METRICSJSON
bash ~/.claude/skills/peer-review/persist.sh`;

const row = parseEmbeddedRow(PERSIST_PROMPT);
eq("row: pr", row.pr, 1935);
eq("row: findings survive the heredoc", row.findings.length, 1);
// The run carries its own row, which is the whole point: PR #1935 has ten rows
// at ONE headSha within hours, so pairing a run to a row by nearest timestamp
// validated one run's reconstruction against another run's panel.
eq("row: ts is null in the embedded copy", row.ts, null);
eq("row: a truncated line is absent, not half-read", parseEmbeddedRow('{"schemaVersion":3,"pr":19'), null);
eq("row: no metrics line at all", parseEmbeddedRow("Run this bash: echo hi"), null);

// --- 7. the strengthen alias, end to end ------------------------------------
//
// The bridge that decides whether this whole module measures anything. Round 0
// votes on the ORIGINAL title; the strengthen pass renames the finding; the row
// records the NEW name. Without the alias, looking the row up finds only the
// round-1 votes, and a refine appears to have "survived round 0" — which is a
// contradiction, not a measurement.

const R0 = { verdict: "REFUTED", confidence: 0.9, source: "claude", coreStands: true };
const R1 = { verdict: "REFUTED", confidence: 0.95, source: "claude", coreStands: false };
const agents = [
  {
    agentId: "a1",
    role: "claude-refuter",
    ts: "2026-07-24T10:00:00Z",
    cases: [{ slotId: 3, title: "original title", file: "src/x.ts:10", severity: "important" }],
    votes: [{ slotId: 3, vote: R0 }]
  },
  {
    agentId: "a2",
    role: "strengthen",
    ts: "2026-07-24T10:05:00Z",
    cases: [{ title: "original title", file: "src/x.ts:10", severity: "important" }],
    votes: [],
    strengthenedTo: { title: "tightened title", file: "src/x.ts" }
  },
  {
    agentId: "a3",
    role: "claude-refuter",
    ts: "2026-07-24T10:09:00Z",
    cases: [{ slotId: 3, title: "tightened title", file: "src/x.ts", severity: "nit" }],
    votes: [{ slotId: 3, vote: R1 }]
  }
];

const tl = assembleTimeline(agents);
const viaRow = lookupFinding(tl, "tightened title", "src/x.ts");
ok("alias: the row's post-refine title resolves", viaRow.entry !== null);
ok("alias: and it is marked as resolved through the alias", viaRow.viaAlias === true);
eq("alias: round 0 is the pre-strengthen vote", roundZeroVotes(viaRow.entry).map((v) => v.confidence), [0.9]);
eq("alias: attribution reads coreStands from round 0", attributeRefine({ coreStands: true, borderline: false }), "coreStands-only");

// The cut is belt-and-braces, and the corpus never exercises it (round-1 votes
// land under the strengthened key, so they are already a different entry). It is
// kept because that separation is a property of the CURRENT prompt wording: if a
// strengthen pass ever returns the title unchanged, the timestamp cut is the
// only thing standing between round 0 and round 1.
const sameTitleAgents = [
  {
    agentId: "b1",
    role: "claude-refuter",
    ts: "2026-07-24T10:00:00Z",
    cases: [{ slotId: 1, title: "same", file: "src/y.ts", severity: "important" }],
    votes: [{ slotId: 1, vote: R0 }]
  },
  {
    agentId: "b2",
    role: "strengthen",
    ts: "2026-07-24T10:05:00Z",
    cases: [{ title: "same", file: "src/y.ts", severity: "important" }],
    votes: [],
    strengthenedTo: { title: "same", file: "src/y.ts" }
  },
  {
    agentId: "b3",
    role: "claude-refuter",
    ts: "2026-07-24T10:09:00Z",
    cases: [{ slotId: 1, title: "same", file: "src/y.ts", severity: "important" }],
    votes: [{ slotId: 1, vote: R1 }]
  }
];
const tl2 = assembleTimeline(sameTitleAgents);
const e2 = lookupFinding(tl2, "same", "src/y.ts").entry;
eq("cut: both votes are on the entry", e2.votes.length, 2);
eq("cut: only the pre-strengthen one is round 0", roundZeroVotes(e2).map((v) => v.confidence), [0.9]);
eq("cut: an unrenamed strengthen registers no alias", tl2.aliases.size, 0);

// Two strengthen passes: the cut sits at the FIRST, or a later one would pull
// round-1 votes back into round 0.
const twoStrengthen = assembleTimeline([
  ...sameTitleAgents,
  {
    agentId: "b4",
    role: "strengthen",
    ts: "2026-07-24T10:02:00Z",
    cases: [{ title: "same", file: "src/y.ts", severity: "important" }],
    votes: [],
    strengthenedTo: null
  }
]);
eq(
  "cut: earliest strengthen wins",
  lookupFinding(twoStrengthen, "same", "src/y.ts").entry.strengthenTs,
  "2026-07-24T10:02:00Z"
);

// --- 8. identity ------------------------------------------------------------

eq("key: line numbers are not identity", normFile("src/a.ts:438"), "src/a.ts");
eq("key: whitespace collapses", findingKey("a  b\nc", "src/a.ts"), findingKey("a b c", "src/a.ts"));
eq("key: case-insensitive", findingKey("A B", "src/A.ts"), findingKey("a b", "src/a.ts"));
ok("key: different files are different findings", findingKey("t", "src/a.ts") !== findingKey("t", "src/b.ts"));

// A vote that cannot be attributed to a case is DROPPED, never assigned to the
// nearest finding. The caller counts these — silently attaching a vote to the
// wrong finding would corrupt exactly the tally being reconstructed.
const orphan = assembleTimeline([
  { agentId: "c1", role: "claude-refuter", ts: "t", cases: [], votes: [{ slotId: 5, vote: R0 }] }
]);
eq("orphan: an unattributable vote creates no finding", orphan.findings.size, 0);

// A batch whose slotIds do not line up with its cases must not fall back to
// "the only case" — that fallback exists for single dispatches only.
const mismatched = assembleTimeline([
  {
    agentId: "c2",
    role: "claude-refuter",
    ts: "t",
    cases: [
      { slotId: 1, title: "one", file: "src/a.ts", severity: "nit" },
      { slotId: 2, title: "two", file: "src/b.ts", severity: "nit" }
    ],
    votes: [{ slotId: 99, vote: R0 }]
  }
]);
eq("orphan: a wrong slotId in a multi-case batch is dropped", mismatched.findings.size, 0);

console.log(
  failures.length
    ? `\nreplay-refine-attribution: ${checks - failures.length}/${checks} checks pass\n\n` +
        failures.map((f) => `  FAIL ${f}`).join("\n") +
        "\n"
    : `\nreplay-refine-attribution: ${checks}/${checks} checks pass\n`
);
process.exit(failures.length ? 1 : 0);
