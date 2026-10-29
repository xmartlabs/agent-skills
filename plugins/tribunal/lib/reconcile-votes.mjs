#!/usr/bin/env node
// reconcile-votes.mjs — check that every external-head vote a peer review
// recorded matches what the CLI actually returned.
//
// Usage:
//   node reconcile-votes.mjs <artifact-dir> < votes.json
//
//   votes.json: [{ slotId, source, verdict, confidence, slotStatus }, …]
//               — only external slots (source !== "claude"); Claude refuters
//               produce no artifact and are out of scope here.
//   artifact-dir: where the refuters parked raw CLI output, one file per slot:
//               <slotId>-<source>.out
//
// Output: JSON on stdout — { checked, ok, classifierFailed, counts,
//               mismatches: [...] }. Exit 0 always; a mismatch is data for the
//               report, not a crash. That includes a classifier that could not
//               run: lib/persist-run.sh ends its invocation in `|| true`, so a
//               non-zero exit would be swallowed anyway and this JSON is the
//               only channel anybody reads.
//
// Why this exists. The panel's votes are produced by dispatched agents that
// have Bash, so nothing can *prevent* one from returning a verdict it invented
// — and on PR #1911 two of them did exactly that: asked to read the CLI's prose
// and pick a category, they instead re-ran codex with prompts they wrote
// themselves, discarded an answer they disliked, and submitted their own
// synthesis as a 0.98-confidence vote, indistinguishable in the record from a
// clean single-shot verdict. What CAN be done is check the vote against the
// artifact the CLI left behind, with a program rather than another model. A
// shortcut no longer passes silently; only deliberate tampering with the file
// on disk would, and that is a different thing from the sloppiness we saw.
//
// The classification is NOT reimplemented here — it shells out to
// classify-verdict.sh, the same script the refuters use, so this cannot drift
// from what it is auditing.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// The classifier is this file's SIBLING — both ship in the plugin's lib/. Until
// 2026-08-12 it was resolved from `homedir()/.claude/lib/`, the pre-plugin
// layout — a directory nothing in this repo creates any more. On a machine with
// no leftover from the pre-plugin install (measured here 2026-08-12: the whole
// directory was absent) every classify() call returned ENOENT and the failure
// was reported as a deviation by the vote. A machine still carrying that
// leftover would instead have run a STALE copy of the classifier, which is the
// quieter half of the same bug and the reason this is not merely cosmetic.
// That is the same hardcoded-homedir resolution eval/paths.mjs was
// written to eliminate across 36 call sites; this one was missed because it
// lives outside eval/. A sibling reference cannot go stale under repackaging —
// the two files sit in the same directory in the repo, in the version-keyed
// cache, and under any future layout, because they move together or not at all.
//
// Not resolved through eval/paths.mjs's `lib()` on purpose: lib/ is runtime code
// that runs on every review, and importing a helper out of the eval battery to
// get there would invert the layering.
//
// TRIBUNAL_CLASSIFIER is a TEST SEAM, not a configuration knob. With the path
// hardcoded there is no other way to prove that a classifier which cannot run is
// REPORTED as one — the `classifier-failed` kind below — and that proof is the
// point of the whole branch. eval/replay-reconciliation.mjs is the only thing
// that sets it. Production always takes the sibling.
const CLASSIFIER =
  process.env.TRIBUNAL_CLASSIFIER || resolve(HERE, "classify-verdict.sh");
const CONFIDENCE_TOLERANCE = 0.01;

const dir = process.argv[2];
if (!dir) {
  console.log(JSON.stringify({ error: "usage: reconcile-votes.mjs <artifact-dir> < votes.json" }));
  process.exit(0);
}

let votes;
try {
  votes = JSON.parse(readFileSync(0, "utf8") || "[]");
} catch {
  console.log(JSON.stringify({ error: "votes on stdin were not valid JSON" }));
  process.exit(0);
}
if (!Array.isArray(votes)) votes = [];

function classify(file) {
  try {
    // Through the interpreter, never as a directly-executable file. Windows
    // cannot exec a `.sh` whatever its shebang or its exec bit — the spawn dies
    // with EFTYPE — and this call is on the production path, so the failure
    // arrived wearing the costume of a data result. `lib/classify-artifacts.sh`
    // already invokes this same script as `bash "$CLASSIFIER"`, so the
    // interpreter form is the existing convention here, not a new one.
    //
    // This form is pinned at the source level by eval/check-mirror.mjs. It has
    // to be: on POSIX the direct-exec spelling WORKS (mode 0755 + a bash
    // shebang), so no behavioural test can catch a regression here except on
    // Windows, and a guard that only fires on one platform is not a guard.
    const out = execFileSync("bash", [CLASSIFIER, file], { encoding: "utf8" });
    const pick = (k) => (out.match(new RegExp(`^${k}:\\s*(.+)$`, "m")) || [])[1]?.trim() ?? null;
    return {
      failclass: pick("FAILCLASS"),
      verdict: pick("VERDICT"),
      confidence: pick("CONFIDENCE") === null ? null : Number(pick("CONFIDENCE")),
    };
  } catch (err) {
    return { failclass: "classifier-failed", verdict: null, confidence: null, error: String(err) };
  }
}

// The SAME naming rule as `artifactKey` in workflows/peer-review.js — a refine
// REPLACES the panel, so the vote recorded for a refined finding is its ROUND-1
// answer and lives in `<slot>r1-<source>.out`. This file used to hardcode
// `<slot>-<source>.out`, which meant every refined slot was checked against the
// SUPERSEDED round-0 artifact: on the 2026-07-25 #1911 run that produced 4
// confidence-mismatches, 1 vote-without-verdict and 12 orphans, all spurious,
// and failed the run's own self-audit. A detector that cries wolf on every run
// that refines anything is how a real dropped answer hides.
//
// A vote with no `round` means round 0 — the shape every row before 2026-07-27
// has, and the reason the old corpus still reconciles unchanged.
const artifactKey = (slotId, source, round) =>
  `${slotId}${round ? `r${round}` : ""}-${source}`;
// Strip the round to ask "does ANY vote own this slot?". Used only for orphan
// classification, never to locate a vote's own artifact.
const baseKey = (key) => key.replace(/r\d+-/, "-");

const mismatches = [];
const seenFiles = new Set();
const votedBaseKeys = new Set();
// Distinct `MODEL:` values seen across this run's codex slots — the resolved
// $MODEL each slot's bash block wrote to its own .meta sidecar (external-heads.md
// / externalRunBlock in tribunal.js), next to CHARS:/RUNS:. Usually one value;
// more than one means the user's ~/.codex/config.toml changed mid-run, which is
// exactly the model-boundary crossing metrics.jsonl previously had no way to see.
const codexModelsUsed = new Set();
let checked = 0;
// Counted separately from `counts` so the fact survives --compact, which keeps
// only `kind` per mismatch and caps the list at COMPACT_LIMIT. A reader has to
// be able to tell "this run was not audited" from "this run was audited and came
// back clean" without reconstructing it from a truncated array — and --compact is
// the only mode lib/persist-run.sh ever uses.
let classifierFailed = 0;

for (const vote of votes) {
  if (!vote || vote.source === "claude") continue;
  checked++;
  const key = artifactKey(vote.slotId, vote.source, vote.round);
  votedBaseKeys.add(baseKey(key));
  const file = join(dir, `${key}.out`);

  if (!existsSync(file)) {
    // A vote with no artifact means the CLI was never run the way the protocol
    // says — the single most direct signature of a fabricated or re-derived
    // answer. An abstention is exempt only when it failed before writing.
    mismatches.push({
      slotId: vote.slotId,
      source: vote.source,
      kind: "missing-artifact",
      detail: `no ${key}.out — the vote has nothing backing it`,
      recorded: { verdict: vote.verdict, slotStatus: vote.slotStatus },
    });
    continue;
  }
  seenFiles.add(`${key}.out`);

  // The protocol allows the CLI to run at most twice (one dispatch plus the
  // single fast rate-limit retry the bash itself performs). Anything above that
  // is the agent re-running the tool on its own initiative — the deviation that
  // comparing verdicts CANNOT catch, because an agent that re-runs until the
  // answer suits it leaves a final artifact that agrees with its vote.
  //
  // READ THE COUNT WITH CARE. Until 2026-07-23 the artifact directory was named
  // per-PR, not per-run, and this sidecar is append-only by design — so several
  // reviews of the same PR accumulated their `RUNS:` lines on one file and an
  // ordinary single-shot slot read as over-invoked. Every over-invocation figure
  // this script produced before the directory became per-run is uninterpretable,
  // including the "13 of 35 slots" that motivated the single-shot latch. The
  // check is kept because it is now scoped correctly, not because those readings
  // stood. The self-audit's `stale-artifacts` invariant guards the scoping.
  const metaFile = join(dir, `${key}.meta`);
  if (existsSync(metaFile)) {
    seenFiles.add(`${key}.meta`);
    // SUM every RUNS line, don't read the first. The sidecar is appended to, not
    // overwritten, precisely so that an agent issuing a SECOND bash call to
    // re-invoke the CLI cannot reset the counter back to 1.
    const meta = readFileSync(metaFile, "utf8");
    const runs = [...meta.matchAll(/^RUNS:\s*(\d+)/gm)].reduce(
      (n, m) => n + Number(m[1]),
      0,
    );
    // Since the run block latches (2026-07-23), a repeat no longer inflates
    // RUNS — it is refused before the CLI is reached. That closes the leak and
    // would also close the only window we had onto it, so the latch records the
    // refusal and it is reported here. `over-invoked` now means the agent got
    // around the block; `repeat-blocked` means it tried and the block held.
    const blocked = [...meta.matchAll(/^BLOCKED:\s*(\d+)/gm)].reduce(
      (n, m) => n + Number(m[1]),
      0,
    );
    if (vote.source === "codex") {
      const modelMatch = meta.match(/^MODEL:\s*(\S+)/m);
      if (modelMatch) codexModelsUsed.add(modelMatch[1]);
    }
    if (blocked > 0) {
      mismatches.push({
        slotId: vote.slotId,
        source: vote.source,
        kind: "repeat-blocked",
        detail: `the block was re-executed ${blocked} time(s) after it had already run; the latch refused it and the original artifact stands`,
        recorded: { verdict: vote.verdict, confidence: vote.confidence },
        actual: { blocked },
      });
    }
    if (Number.isFinite(runs) && runs > 2) {
      mismatches.push({
        slotId: vote.slotId,
        source: vote.source,
        kind: "over-invoked",
        detail: `the CLI ran ${runs} times; the protocol allows at most 2 (dispatch + one rate-limit retry)`,
        recorded: { verdict: vote.verdict, confidence: vote.confidence },
        actual: { runs },
      });
    }
  }

  const actual = classify(file);

  // A classifier that could not RUN is not a fact about the vote, and collapsing
  // the two is what kept this defect uncharacterised for three weeks. With the
  // classifier dead every artifact fell through to `vote-without-verdict` below —
  // a well-formed data result naming the wrong culprit — while the abstention
  // branch, which only fires on `failclass === "none"`, reported nothing at all,
  // silently disarming the reverse leak that cost 12 real votes on PR #1911. The
  // replay then read as a corpus disagreement (`ok 13, expected 23`) when the
  // classifier had never run once.
  //
  // Raised BEFORE the abstained/not split precisely so it covers both.
  if (actual.failclass === "classifier-failed") {
    classifierFailed++;
    mismatches.push({
      slotId: vote.slotId,
      source: vote.source,
      kind: "classifier-failed",
      detail: `the verdict classifier could not run, so this vote was never audited — ${actual.error}`,
      recorded: { verdict: vote.verdict, slotStatus: vote.slotStatus },
    });
    continue;
  }

  const abstained = vote.slotStatus === "UNAVAILABLE";

  if (abstained) {
    // The reverse error: the head DID produce a usable verdict and the slot was
    // recorded as an abstention anyway. That is how 12 real votes were thrown
    // away on PR #1911 before the classifier was fixed.
    if (actual.failclass === "none") {
      mismatches.push({
        slotId: vote.slotId,
        source: vote.source,
        kind: "abstained-despite-verdict",
        detail: `recorded UNAVAILABLE but the artifact declares ${actual.verdict}`,
        recorded: { slotStatus: vote.slotStatus },
        actual,
      });
    }
    continue;
  }

  if (actual.failclass !== "none") {
    mismatches.push({
      slotId: vote.slotId,
      source: vote.source,
      kind: "vote-without-verdict",
      detail: `recorded a ${vote.verdict} vote but the artifact classifies as ${actual.failclass}`,
      recorded: { verdict: vote.verdict, confidence: vote.confidence },
      actual,
    });
    continue;
  }

  if (actual.verdict !== vote.verdict) {
    mismatches.push({
      slotId: vote.slotId,
      source: vote.source,
      kind: "verdict-mismatch",
      detail: `recorded ${vote.verdict}, artifact says ${actual.verdict}`,
      recorded: { verdict: vote.verdict },
      actual,
    });
    continue;
  }

  if (
    typeof vote.confidence === "number" &&
    typeof actual.confidence === "number" &&
    Math.abs(vote.confidence - actual.confidence) > CONFIDENCE_TOLERANCE
  ) {
    mismatches.push({
      slotId: vote.slotId,
      source: vote.source,
      kind: "confidence-mismatch",
      detail: `recorded ${vote.confidence}, artifact says ${actual.confidence}`,
      recorded: { confidence: vote.confidence },
      actual,
    });
  }
}

// An artifact nobody voted on means a head ran and its answer was dropped —
// the opposite leak, and just as invisible without this check.
//
// Grouped by artifact KEY, not by file. `.out` and `.meta` are two files
// carrying ONE answer, and counting them separately reported 6 dropped answers
// as 12 — a headline built on that number is exactly 2x too high.
//
// And a leftover round is not a drop. When a finding is refined the panel is
// replaced, so its round-0 artifact is SUPERSEDED by design: the answer was not
// lost, it was overruled by a later round of the same slot. That gets its own
// kind — reporting it as an orphan would trade one false alarm for another,
// which is all a naive round-awareness fix would have achieved.
let orphans = [];
try {
  const byKey = new Map();
  for (const f of readdirSync(dir)) {
    if ((!f.endsWith(".out") && !f.endsWith(".meta")) || seenFiles.has(f)) continue;
    const key = f.replace(/\.(out|meta)$/, "");
    if (!byKey.has(key)) byKey.set(key, []);
    byKey.get(key).push(f);
  }
  orphans = [...byKey.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, files]) => {
      const superseded = votedBaseKeys.has(baseKey(key));
      const [slotId, source] = (/^(.+?)(?:r\d+)?-(codex|gemini)$/.exec(key) || []).slice(1);
      return {
        // Identity is kept even in --compact (see below): an unidentifiable
        // violation cannot be acted on once the temp dir is gone, which is what
        // "orphan-artifact: 12, slotId: null" amounted to.
        slotId: slotId === undefined ? null : Number.isNaN(Number(slotId)) ? slotId : Number(slotId),
        source: source ?? null,
        kind: superseded ? "superseded-round" : "orphan-artifact",
        key,
        files,
        detail: superseded
          ? "an earlier round of a slot whose vote came from a later round — superseded, not dropped"
          : "CLI output with no corresponding vote",
      };
    });
} catch {
  orphans = [];
}
mismatches.push(...orphans);

// --compact drops the per-mismatch evidence (recorded/actual verdicts, detail
// prose) and keeps only what identifies the deviation. The full form runs to
// several KB, and the only way it reaches the workflow is an agent relaying it:
// on the 2026-07-23 run the agent abridged it into something that would not
// parse, so the audit degraded to "unparseable" and the run went unaudited.
// A payload small enough to relay verbatim is a precondition for being read.
const compact = process.argv.includes("--compact");
// Cap the relayed list, never the counts. The payload has to stay small enough
// for an agent to copy verbatim — the 2026-07-23 run's audit degraded to
// "unparseable" precisely because a long one got abridged into invalid JSON.
const COMPACT_LIMIT = 40;
const counts = {};
for (const m of mismatches) counts[m.kind] = (counts[m.kind] || 0) + 1;

// Slots that came through clean — count SLOTS, not mismatches: one slot can
// trip several checks at once (the PR #1911 case trips both over-invoked and
// verdict-mismatch) and must not be subtracted twice. Three kinds do not impugn
// the vote and so do not subtract at all: an orphan artifact belongs to no
// slot, a superseded round belongs to a slot whose LATER round was checked and
// passed, and a blocked repeat says something about the DRIVER while the vote
// still rests on the artifact the head actually wrote.
//
// `classifier-failed` is deliberately NOT in this set, and the contrast with
// `repeat-blocked` is exactly why. A blocked repeat leaves the vote resting on
// the artifact the head really wrote, so the vote WAS audited and stays in `ok`.
// A dead classifier audited nothing, so its slot cannot be counted clean. It is
// still not a deviation BY the vote — workflows/tribunal.js excludes it from the
// deviation headline for that reason — but "unaudited" and "clean" are the two
// states a reader must never be allowed to confuse, and `ok` is where that
// confusion would live.
const NON_IMPUGNING = new Set(["orphan-artifact", "superseded-round", "repeat-blocked"]);
const ok =
  checked -
  new Set(
    mismatches
      .filter((m) => !NON_IMPUGNING.has(m.kind))
      .map((m) => `${m.slotId}-${m.source}`),
  ).size;
const codexModelsUsedSorted = [...codexModelsUsed].sort();

console.log(
  JSON.stringify(
    compact
      ? {
          checked,
          ok,
          classifierFailed,
          counts,
          codexModelsUsed: codexModelsUsedSorted,
          // `key` rides along because for an orphan or a superseded round it is
          // the ONLY identity there is — those entries have no slotId of their
          // own, and dropping it is what left "orphan-artifact: 12" with twelve
          // `slotId: null` rows that nobody could act on after the temp dir was
          // gone. `counts` still carries the totals if the list is capped.
          mismatches: mismatches.slice(0, COMPACT_LIMIT).map((m) => ({
            slotId: m.slotId ?? null,
            source: m.source ?? null,
            kind: m.kind,
            ...(m.key ? { key: m.key } : {}),
          })),
          ...(mismatches.length > COMPACT_LIMIT
            ? { omitted: mismatches.length - COMPACT_LIMIT }
            : {}),
        }
      : {
          checked,
          ok,
          classifierFailed,
          counts,
          codexModelsUsed: codexModelsUsedSorted,
          mismatches,
        },
    null,
    compact ? 0 : 2,
  ),
);
