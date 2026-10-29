#!/usr/bin/env node
// replay-clean-sweep.mjs — the zero-panel sweep's gate and parser, sliced out
// of the shipping workflow (CLEAN-SWEEP region).
//
// WHY IT EXISTS. The external heads only enter through panel slots, so before
// 2026-08-14 the clean verdict — the one that green-lights a merge — was the
// only verdict no second model family ever checked (FE PR #1516: five lenses
// to zero candidates, "clean" report, 11 real defects). The sweep closes that;
// this replay pins the two decisions that make it safe:
//   · the GATE fires only on zero panel-eligible findings with a healthy,
//     installed Codex — never stacking a sweep on a review that already has
//     external votes, never dispatching into a head known to be down;
//   · the PARSER treats anything that is neither the strict FINDING format nor
//     the exact clean marker as UNPARSEABLE — a sweep whose answer cannot be
//     read must never be recorded as a second-family confirmation. That is the
//     same "a failed head is an abstention, never agreement" rule the panels
//     already live by.
//
// Usage: node eval/replay-clean-sweep.mjs

import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nclean sweep — CLEAN-SWEEP region of tribunal.js`);

const { api, violations } = loadRegion(WORKFLOW, "CLEAN-SWEEP", [
  "shouldRunCleanSweep",
  "parseSweepFindings",
  "sweepArtifactLooksReal",
  "sweepFamilyFor",
]);
const { shouldRunCleanSweep, parseSweepFindings, sweepArtifactLooksReal, sweepFamilyFor } = api;

check(
  "the region is pure (no run state, no dispatch)",
  violations.length === 0,
  violations.join("; "),
);

// --- gate ---
const g = (input) => shouldRunCleanSweep(input);
const PINNED = "/tmp/peer-review-wt-2101-XXXX";
check(
  "gate: fires on zero eligible with healthy installed codex and a pinned tree",
  g({
    panelEligibleCount: 0,
    codexInstalled: true,
    codexHealthy: true,
    externalCwd: PINNED,
  }).run === true,
);
// Added 2026-09-11. Without a pinned tree the CLI runs in the ambient cwd,
// which is whatever branch the operator happens to be on. That is not a
// degraded sweep, it is a confident answer about the wrong code: on PRs #2101
// and #2094 it read a checkout that did not contain the PR at all, and the run
// still reported a clean verdict. Skipping is the honest outcome.
check(
  "gate: refuses to sweep an unpinned tree rather than reviewing the wrong code",
  g({ panelEligibleCount: 0, codexInstalled: true, codexHealthy: true }).run === false &&
    g({ panelEligibleCount: 0, codexInstalled: true, codexHealthy: true }).reason ===
      "no-pinned-head",
);
check(
  "gate: never fires when any finding will face a panel",
  g({ panelEligibleCount: 1, codexInstalled: true, codexHealthy: true }).run === false &&
    g({ panelEligibleCount: 1, codexInstalled: true, codexHealthy: true }).reason === "panels-exist",
);
check(
  "gate: skips with reason when codex is not installed",
  g({ panelEligibleCount: 0, codexInstalled: false, codexHealthy: false }).reason ===
    "codex-not-installed",
);
check(
  "gate: skips with reason when codex is unhealthy (sticky routing)",
  g({ panelEligibleCount: 0, codexInstalled: true, codexHealthy: false }).reason ===
    "codex-unhealthy",
);

// --- parser ---
const clean = parseSweepFindings(
  "I looked hard at the diff and the call sites.\n\nNO FINDINGS ABOVE NIT.\n\nNits:\nnit stuff here",
);
check(
  "parser: exact clean marker parses as clean, no findings",
  clean.clean === true && clean.unparseable === false && clean.findings.length === 0,
);

const withFindings = parseSweepFindings(
  [
    "Some preamble the model wrote.",
    "FINDING: important | Count push races the commit | src/services/v2/accounts.ts:585 | two invocations on one account leave the partner below the committed value",
    "EVIDENCE: const actualCount = countData",
    "FINDING: nit | Padded names keep inner whitespace | src/_helpers/v2/textFormat.ts:41 | 'Jane ' + ' Doe' yields a doubled space",
    "EVIDENCE: `${input.firstName ?? ''} ${input.lastName ?? ''}`.trim() ||",
  ].join("\n"),
);
check(
  "parser: strict FINDING/EVIDENCE pairs parse with severity, file, line, quote",
  withFindings.findings.length === 2 &&
    withFindings.findings[0].severity === "important" &&
    withFindings.findings[0].file === "src/services/v2/accounts.ts" &&
    withFindings.findings[0].line === 585 &&
    withFindings.findings[0].evidence.startsWith("const actualCount") &&
    withFindings.findings[1].severity === "nit" &&
    withFindings.clean === false &&
    withFindings.unparseable === false,
);
check(
  "parser: a FINDING line with no EVIDENCE line gets the explicit no-quote marker",
  parseSweepFindings(
    "FINDING: important | X | a.ts:1 | because\nsome other prose",
  ).findings[0].evidence === "No relevant quotes found",
);

// The one that must never regress: prose that is neither format is UNPARSEABLE,
// not clean. Recording an unreadable sweep as confirmation would let an
// infrastructure failure argue for merging — the exact inversion the panel's
// abstention rule exists to prevent.
const vague = parseSweepFindings(
  "The change looks good to me overall, nothing major stands out.",
);
check(
  "parser: an answer in neither format is unparseable, NEVER clean",
  vague.unparseable === true && vague.clean === false,
);
check(
  "parser: empty/missing artifact is unparseable, NEVER clean",
  parseSweepFindings("").unparseable === true && parseSweepFindings("").clean === false &&
    parseSweepFindings(null).unparseable === true,
);

// --- the poll's break predicate (2026-09-11) -------------------------------
// The poll loop used to break on any non-empty text. The mechanical reader
// returns prose about the command when the emitted bash exits non-zero, and
// that prose is non-empty: one run broke out after a single 100s slice, then
// tore down the worktree the CLI was still reading. Only the sweep's own shape
// counts as an answer.
console.log(`\n--- a poll answer is evidence, not narration ---`);
{
  const NARRATION = "Command executed with exit code 1. No output was printed.";
  const REAL_FINDING = [
    "FINDING: important | Count push races the commit | src/services/v2/accounts.ts:585 | two invocations leave the partner low",
    "EVIDENCE: const actualCount = countData",
  ].join("\n");

  check(
    "the reader's own exit-code prose is NOT an artifact",
    sweepArtifactLooksReal(NARRATION) === false,
    "the exact string that ended a run early still breaks the poll",
  );
  check(
    "the exact clean marker IS an artifact",
    sweepArtifactLooksReal("NO FINDINGS ABOVE NIT.") === true,
  );
  check(
    "a FINDING line IS an artifact",
    sweepArtifactLooksReal(REAL_FINDING) === true,
  );
  check(
    "an empty or absent artifact is NOT one",
    sweepArtifactLooksReal("") === false &&
      sweepArtifactLooksReal(null) === false &&
      sweepArtifactLooksReal(undefined) === false,
  );
  // Control: the group above is only worth anything if a degenerate predicate
  // fails it. Run the same expectations against one that says yes to
  // everything — it has to get the narration and the empty artifact wrong.
  const CASES = [
    [NARRATION, false],
    ["NO FINDINGS ABOVE NIT.", true],
    [REAL_FINDING, true],
    ["", false],
    [null, false],
  ];
  const wrong = (p) => CASES.filter(([input, want]) => p(input) !== want);
  check(
    "control: a predicate answering true to everything fails this group",
    wrong(() => true).length === 3 && wrong(sweepArtifactLooksReal).length === 0,
    `always-true got ${wrong(() => true).length} wrong, shipping predicate got ${wrong(sweepArtifactLooksReal).length}`,
  );
}

// --- the sweep's outcome has to REACH the reader (2026-09-04) --------------
// The gate and the parser were already right: an unreadable sweep is recorded
// as failed, never as agreement. What was missing is that none of it left the
// script. `cleanSweep` went to metrics.jsonl and to log(), and log() does not
// reach the caller, so the human reading an APPROVE saw the same report
// whether a second model family confirmed the clean verdict, answered
// unreadably, or never ran.
//
// These are static assertions over the shipping prompt, because the synthesis
// prompt is one template literal over live run state and cannot be evaluated
// standalone. They pin the interpolation, not the model's obedience to it:
// whether a report actually carries the line is a claim only a real run
// settles.
console.log(`\n--- the sweep outcome reaches the synthesis prompt ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const start = src.indexOf("let report = await agent(");
  const end = src.indexOf('label: "synthesize"', start);
  const prompt = start !== -1 && end !== -1 ? src.slice(start, end) : "";

  check(
    "the synthesis prompt block was located",
    prompt.length > 0,
    `start=${start} end=${end}`,
  );
  check(
    "the sweep outcome is interpolated into the synthesis prompt",
    /\bcleanSweep\b/.test(prompt),
  );
  check(
    "it arrives as a payload block the report can quote",
    prompt.includes("<clean_verdict_sweep"),
  );
  check(
    "a grounding rule spells out what each outcome means",
    /"ran": false/.test(prompt) && /"unparseable": true/.test(prompt),
  );
  check(
    "the rule forbids reading a failed or skipped sweep as agreement",
    /never report an unrun, failed or skipped sweep as agreement/i.test(prompt),
  );
  check(
    "both the block and the rule are gated on the sweep having a value",
    (prompt.match(/\n\s*cleanSweep\n\s*\?/g) || []).length >= 2,
  );

  // VR-23, same defect one branch over and closed the same way on 2026-09-08.
  // A finding whose strengthen pass the budget could not pay for was decided on
  // its first, weaker form; the skip reached `refinesSkippedForBudget` on the
  // metrics row and stopped there, so the reader saw a verdict with no sign it
  // had been reached more cheaply than the others.
  check(
    "findings whose refine the budget skipped reach the report too",
    prompt.includes("<refine_skipped_for_budget"),
  );
  check(
    "…and a grounding rule requires the fact be attached to each of them",
    /verdict on its weaker first form/.test(prompt),
  );
  check(
    "…gated the same way, so an ordinary run carries neither the block nor the rule",
    (prompt.match(/refinesSkippedForBudget\.length\n\s*\?/g) || []).length >= 2,
  );
}

// --- the dispatch itself (added 2026-09-11) ---
//
// Everything above this line is about the gate and the parser, and every one of
// those checks passed on every broken run — because neither was ever the broken
// part. The parser correctly reported `unparseable` on an empty artifact for
// weeks while the reason the artifact was empty went unasserted: the command
// reviewed the wrong tree and was killed before it answered. Nothing here
// looked at the dispatch at all.
//
// These are static assertions over the emitted block, in the same spirit as the
// synthesis-prompt block above: they pin what the harness SENDS, which is the
// half a replay can settle. Whether Codex then answers is a claim only a real
// run settles.
console.log(`\n--- the sweep dispatch (what the harness actually sends) ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const start = src.indexOf("const sweepBlock = [");
  const end = src.indexOf('label: "heads:codex:sweep"', start);
  const block = start !== -1 && end !== -1 ? src.slice(start, end) : "";

  check("the sweep block was located", block.length > 0, `start=${start} end=${end}`);

  // The defect of 2026-09-11: the refuter head carried `-C` and this one did
  // not, so Codex ran wherever the session happened to be. A prose instruction
  // to the driver is not a pin.
  check(
    "the CLI is pinned to a directory in the command, not in prose",
    /codex exec[^`]*-C "/.test(block),
    "no -C on the sweep's codex exec",
  );
  check(
    "the pinned directory is the harness's own resolved tree",
    /SWEEP_CWD="\$\{externalCwd\}"|-C "\$\{externalCwd\}"/.test(block) ||
      block.includes('SWEEP_CWD="${externalCwd}"'),
    "the -C value does not come from externalCwd",
  );

  // The second defect: the driver's tool backgrounds a long command at 120s and
  // its turn-end kills what it backgrounded, so a blocking launch can never
  // reach SWEEP_TIMEOUT_S. The launch has to outlive the agent that starts it.
  check(
    "the CLI is launched detached, so it outlives the driver's turn",
    /\(\s*\$NOHUP .*&\s*\)/.test(block),
    "the codex exec is not wrapped in a backgrounded subshell",
  );
  check(
    "nohup is resolved, not assumed, the same way timeout already is",
    /NOHUP=\$\(command -v nohup \|\| true\)/.test(block),
    "nohup is hardcoded or missing, which breaks where it is not installed",
  );
  check(
    "the bookkeeping is written by the background shell, not by the driver",
    /RUNS: 1" >> "\$META"/.test(block) && !/RC=\$\?; RUNS=\$\(\(RUNS\+1\)\)/.test(block),
    "RUNS is still written in the driver's own shell, where it never runs",
  );

  // The third defect of 2026-09-11, and the one that cost the most: the prompt
  // built `THE DIFF:` from `prContext.diff`, which is EMPTY by design on PRs of
  // 12 files or fewer — the fetch step leaves it empty because `diffPath` holds
  // the diff on disk. Empty is falsy, so the sweep shipped its "too large"
  // fallback and sent Codex exploring the repository: 86 exec calls, CLAUDE.md
  // and agent definitions read, then a hang on a test runner. Every refuter head
  // already reads the diff from the path; the sweep now does too.
  check(
    "the sweep names the fetched diff file instead of inlining prContext.diff",
    block.includes("${diffPath}"),
    "the sweep prompt does not interpolate diffPath",
  );
  check(
    "it no longer sends the model hunting through the working directory",
    !block.includes("read the changed files in the working directory"),
    "the fallback that produced the 86-exec run is still in the prompt",
  );
  check(
    "the fallback is truthful about WHY there is no diff file",
    !/diff too large to inline/.test(block) &&
      /diff file was not written/.test(block),
    "the empty-diffPath fallback still blames the diff's size",
  );
  check(
    "the prompt forbids running the test suite or a build",
    /Do NOT run the test suite, a build, or any other long-running command/.test(
      block,
    ),
    "nothing stops the sweep from spending its timeout in a test runner",
  );
  check(
    "the sweep runs at medium effort, not high",
    /model_reasoning_effort="medium"/.test(block) &&
      !/model_reasoning_effort="high"/.test(block),
    "the sweep's codex exec is still pinned to high",
  );
}

// --- the poll that reads the sweep back (added 2026-09-11) ---
//
// The launch is only half of it. The poll loop broke on any non-empty text, so
// the reader's own "Command executed with exit code 1. No output was printed."
// ended the wait after one slice and the run tore down the worktree underneath
// the CLI. Two things had to change together: the emitted bash must not exit
// non-zero just because the artifact is not there yet, and the loop must break
// on the artifact's shape rather than on the reader's prose.
console.log(`\n--- the poll reads the artifact, not the exit code ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  // The slice runs past the agent call on purpose: the emitted bash and the
  // break condition that consumes its answer are one decision, and pinning only
  // the prompt would leave the half that actually ended the run unasserted.
  const start = src.indexOf("const SWEEP_POLL_SLICE_S");
  const end = src.indexOf("const parsed = parseSweepFindings(", start);
  const poll = start !== -1 && end !== -1 ? src.slice(start, end) : "";

  check("the poll prompt was located", poll.length > 0, `start=${start} end=${end}`);

  // Recover the emitted lines in order: the wait is one array of one-line
  // template literals, shared by the launch dispatch and every later reader.
  const arrStart = poll.indexOf("const sweepReadBash = [");
  const arr = arrStart === -1 ? "" : poll.slice(arrStart, poll.indexOf('].join("\\n");', arrStart));
  const bash = arr
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.startsWith("`") && l.endsWith("`,"))
    .map((l) => l.slice(1, -2));

  check("the emitted bash was recovered", bash.length > 0, `lines=${bash.length}`);
  check(
    "the launch dispatch runs the first wait itself, and every later reader runs the same wait",
    /Block 2:",\s*sweepReadBash,/.test(poll) && /Do not summarize, judge, or repair the content\.`,\s*``,\s*sweepReadBash,/.test(poll),
  );
  check(
    "readers start at the second slice, and only while the first answer is neither finished nor a real artifact",
    /for \(let attempt = 2; attempt <= sweepPollAttempts; attempt\+\+\) \{\s*if \(\s*readBack &&\s*\(readBack\.finished === true \|\| sweepArtifactLooksReal\(readBack\.text\)\)\s*\)\s*break;/.test(poll),
  );
  check(
    "the bash ends with `exit 0`, so a missing artifact is silence, not a failure",
    bash[bash.length - 1] === "exit 0",
    `last emitted line is ${JSON.stringify(bash[bash.length - 1])}`,
  );
  check(
    "the reader is told never to narrate the command's outcome into {text}",
    /never put a description of the command's outcome or its exit code in \{text\}/.test(
      poll,
    ),
    "nothing tells the reader that an absent artifact means an empty string",
  );
  check(
    "the loop breaks on the artifact's shape, via sweepArtifactLooksReal",
    /sweepArtifactLooksReal\(readBack\.text\)/.test(poll),
    "the break condition does not consult the predicate",
  );
  check(
    "…and no longer on any non-empty prose",
    !/\(readBack\.text \|\| ""\)\.trim\(\) \|\| readBack\.finished/.test(poll),
    "the old text-is-non-empty break condition is still there",
  );
}

console.log(`\n--- an unconfirmed clean verdict is not an approval ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const flat = src.replace(/\s+/g, " ");
  check(
    "the recorded verdict degrades when the sweep did not confirm",
    /sweepUnconfirmed/.test(src) &&
      /verdictGuessRaw === "APPROVE" && \(?sweepUnconfirmed\b/.test(
        src.replace(/\s+/g, " "),
      ),
    "APPROVE still survives an unparseable or unpinned sweep",
  );
  // Since v79 the report does not choose its verdict: verdictGuess is computed
  // before the synthesis dispatch and printed into the template. BE #2171
  // (2026-10-17) is why the old "may NOT be APPROVE" rule went: the writer read
  // `clean: false` with a paneled candidate as an unconfirmed sweep and wrote
  // COMMENT while the row said APPROVE.
  check(
    "…and the report is handed that same verdict, decided before it is written",
    src.indexOf("const verdictGuess =") !== -1 &&
      src.indexOf("const verdictGuess =") < src.indexOf("let report = await agent(") &&
      src.includes("### Verdict suggestion: ${verdictGuess}"),
    "the synthesis prompt lets the report choose a verdict the row does not record",
  );
  check(
    "…and a paneled sweep candidate is not described as an unconfirmed sweep",
    /"clean": false with "candidatesRaised" above 0 is NOT an unconfirmed sweep/.test(flat) &&
      !/may NOT be APPROVE/.test(src),
    "the prompt still lets clean:false read as 'the sweep did not confirm'",
  );
}

// --- v100: who sweeps, and sweep/bot anchors ---
{
  const WT = "/tmp/peer-review-wt-1-XXXX";
  const fam = (codexInstalled, codexHealthy, cwd, eligible = 0) =>
    sweepFamilyFor(
      shouldRunCleanSweep({ panelEligibleCount: eligible, codexInstalled, codexHealthy, externalCwd: cwd }),
    );
  check("family: codex when Codex can sweep", fam(true, true, WT) === "codex");
  check("family: nobody when Codex is not installed or unhealthy", fam(false, false, WT) === null && fam(true, false, WT) === null);
  check("family: nobody on an unpinned tree, Codex or not", fam(true, true, null) === null && fam(false, false, null) === null);
  check("family: nobody when a finding will face a panel", fam(false, false, WT, 1) === null && fam(true, true, WT, 1) === null);

  const p = parseSweepFindings(
    [
      "FINDING: important | Missing guard | src/a.ts:10 | a null id reaches the write",
      "EVIDENCE: await write(id)",
      "VERIFIED: src/b.ts:4 | export function write(id: string) {",
      "VERIFIED: src/c.ts:9 | write(req.query.id)",
      "FINDING: nit | Name | src/a.ts:20 | unclear",
      "EVIDENCE: const x = 1",
    ].join("\n"),
  );
  check(
    "parser: VERIFIED lines after EVIDENCE become verifiedBy",
    p.findings[0].verifiedBy.length === 2 &&
      p.findings[0].verifiedBy[1].file === "src/c.ts" &&
      p.findings[0].verifiedBy[1].line === 9 &&
      p.findings[0].verifiedBy[1].quote === "write(req.query.id)",
    JSON.stringify(p.findings[0].verifiedBy),
  );
  check("parser: a finding with no VERIFIED line gets an empty verifiedBy", p.findings[1].verifiedBy.length === 0);
  {
    const { verifiedGateDemoteUnanchored } = loadRegion(WORKFLOW, "VERIFIED-GATE", ["verifiedGateDemoteUnanchored"]).api;
    const survivor = (f) => ({ ...f, lens: "codex-sweep", adjudicated: true });
    const [anchored] = verifiedGateDemoteUnanchored([survivor(p.findings[0])]);
    const [bare] = verifiedGateDemoteUnanchored([survivor({ ...p.findings[0], verifiedBy: [] })]);
    check(
      "a surviving sweep finding with a VERIFIED line passes the verifiedBy gate; one without is demoted",
      !anchored.verifiedByGateNote && Boolean(bare.verifiedByGateNote),
    );
  }

  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "the sweep prompt asks for VERIFIED lines",
    (src.match(/`VERIFIED: <file>:<line> \| <word-for-word quote of the code the claim depends on/g) || []).length === 1,
  );
  check(
    "no Claude hunter: sweep findings are tagged by family and the row records it",
    !src.includes("sweep:claude-hunter") &&
      !src.includes("SWEEP_CLAUDE_MODEL") &&
      src.includes("lens: `${sweepFamily}-sweep`,") &&
      src.includes("family: sweepFamily,"),
  );
  check(
    "a VALID bot comment carries its file, riskPaths flag and verifiedBy",
    /touchesHighRiskSurface: riskPathTouched\(file, riskPathRules\(PROFILE\)\),\s*verifiedBy: Array\.isArray\(c\.verifiedBy\) \? c\.verifiedBy : \[\],\s*lens: "bot-comments",/.test(src) &&
      src.includes("const file = c.file || src.path || \"\";"),
  );
  check("bot-comment vetting is opt-in", src.includes("const vetBotComments = parsedArgs.botComments === true;"));
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("all clean-sweep expectations hold");
