#!/usr/bin/env node
// replay-gates.mjs — run the SHIPPING panel-decision rules over every panel
// this harness has ever recorded, for zero tokens.
//
// Why this exists. Every threshold in the panel (T_KILL, DELTA, the escalation
// gate, the refine gate) used to be validated the only way available: spend a
// real review and read the metrics row afterwards. A large run costs ~950k
// output tokens and ~40 minutes, which made "does this gate change do what I
// think" a question nobody could afford to ask twice. But `metrics.jsonl`
// already persists, per finding, the exact votes that were cast — source,
// verdict, confidence, coreStands — so the whole decision path is reproducible
// cold. This replays it.
//
// Single source of truth, not a mirror. eval/replay-dedup.mjs has to
// re-implement the logic it tests, because workflows/peer-review.js runs in the
// Workflow sandbox and cannot be imported (no `import`, no fs). That copy can
// drift from what ships and nothing would notice. Here the rules are SLICED OUT
// of peer-review.js by the `===== BEGIN/END PANEL-GATES =====` markers and
// evaluated verbatim, the same trick eval/replay-evidence-resolver.mjs gets for
// free by importing a real module. If someone edits the gate, this file tests
// the edit — never a stale transcription of it.
//
// Usage: node eval/replay-gates.mjs [--verbose]
//
// Exit code comes from the GOLDENS and the fidelity check, not from the corpus
// diff. The corpus diff is descriptive by design: "17 panels would now escalate"
// is a fact to read, not a threshold to pass.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { RECORDED_METRICS_PATH, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const METRICS = RECORDED_METRICS_PATH;
const VERBOSE = process.argv.includes("--verbose");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- 1. Extract the pure regions ------------------------------------------
let loaded;
try {
  loaded = loadRegion(WORKFLOW, "PANEL-GATES", [
    "tallyVotes",
    "placeSurvivor",
    "factualRefutationFails",
    "contestedPreDecision",
    "preDecidedCoversFile",
    "samePath",
    "shouldEscalate",
    "needsRefine",
    "shouldDebate",
    "VERDICT_SPLIT",
    "UNDER_CONFIDENT",
    "T_KILL",
    "DELTA",
    "GEMINI_CONFIDENCE_CAP",
    "ESCALATE_CONFIDENCE_FLOOR",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api: G, regions, violations, body: GATES_SRC } = loaded;

console.log(`\nPanel gates extracted from ${WORKFLOW}`);
console.log(
  `  ${regions.length} region(s), ${GATES_SRC.split("\n").length} lines · ` +
    `T_KILL=${G.T_KILL} DELTA=${G.DELTA} ` +
    `GEMINI_CAP=${G.GEMINI_CONFIDENCE_CAP} FLOOR=${G.ESCALATE_CONFIDENCE_FLOOR}`,
);

console.log(`\n--- purity of the extracted regions ---`);
check("no region references run state", violations.length === 0, violations.join(" | "));

// --- 2. Goldens: the boundaries, asserted ---------------------------------
// The corpus below says what WOULD happen on real data; these say what MUST
// happen at the edges, where a threshold change does its damage and where the
// corpus happens to be thin.
const v = (source, verdict, confidence, extra = {}) => ({
  source,
  verdict,
  confidence,
  slotStatus: "OK",
  ...extra,
});
const panelOf = (votes) => votes.map((x) => ({ source: x.source }));
const tally = (votes) => G.tallyVotes(votes, panelOf(votes));

console.log(`\n--- goldens ---`);
{
  // The exact case that motivated the 2026-07-24 change: one claude keep, one
  // codex refute, equal confidence. refuteFraction lands on 0.50, which is
  // >= T_KILL 0.45, so the finding dies on a coin-flip panel.
  const t = tally([
    v("claude", "NOT_REFUTED", 0.85, { coreStands: true }),
    v("codex", "REFUTED", 0.85),
  ]);
  check("1-1 tie at equal confidence kills", t.survived === false, `rf=${t.refuteFraction}`);
  check("1-1 tie is a verdict split", G.VERDICT_SPLIT(t) === true);
  check(
    "1-1 tie escalates on a NON-high-risk surface",
    G.shouldEscalate(t, { touchesHighRiskSurface: false }, 1) === true,
  );
}
{
  // Unanimous refutation is not a split and must not spend an escalation vote.
  const t = tally([v("claude", "REFUTED", 0.9), v("codex", "REFUTED", 0.96)]);
  check("unanimous refute does not escalate", G.shouldEscalate(t, { touchesHighRiskSurface: true }, 1) === false);
  check("unanimous refute kills", t.survived === false);
}
{
  // A panel already carrying 2 claude voters has spent its escalation.
  const t = tally([
    v("claude", "NOT_REFUTED", 0.8, { coreStands: true }),
    v("claude", "REFUTED", 0.8),
  ]);
  check("split with 2 claude voters does not re-escalate", G.shouldEscalate(t, {}, 2) === false);
}
{
  // Nobody voted: unverified. It must not be read as "cleared", and it must not
  // trigger an escalation vote to adjudicate a panel that has no evidence at all.
  const t = G.tallyVotes([null, null], [{ source: "claude" }, { source: "codex" }]);
  check("unverified survives (not silently killed)", t.survived === true);
  check("unverified does not escalate", G.shouldEscalate(t, { touchesHighRiskSurface: true }, 1) === false);
  check("unverified is never borderline", t.borderline === false);
}
{
  // Gemini's self-declared confidence is capped mechanically. A 1.0 gemini
  // refute against a 0.8 claude keep must NOT reach the same weight it claims.
  const t = tally([v("claude", "NOT_REFUTED", 0.8), v("gemini", "REFUTED", 1.0)]);
  const uncapped = 1.0 / (1.0 + 0.8);
  check(
    "gemini confidence is capped below its claim",
    Math.abs(t.refuteFraction - uncapped) > 1e-9 &&
      Math.abs(t.refuteFraction - G.GEMINI_CONFIDENCE_CAP / (G.GEMINI_CONFIDENCE_CAP + 0.8)) < 1e-9,
    `rf=${t.refuteFraction}`,
  );
}
{
  // An UNAVAILABLE head abstains: out of the numerator AND the denominator, so
  // it cannot dilute the live vote.
  const votes = [
    v("claude", "REFUTED", 0.9),
    { source: "codex", slotStatus: "UNAVAILABLE", failureReason: "timeout" },
  ];
  const t = G.tallyVotes(votes, panelOf(votes));
  check("abstention leaves refuteFraction undiluted", t.refuteFraction === 1);
  check("abstention is recorded as a failed slot", t.failedSlots.length === 1);
  check("abstention is not an independent voter", t.hadIndependentVoter === false);
}
{
  // Refine fires on killed-but-core-stands, and never on a pre-decided kill.
  const core = tally([v("claude", "REFUTED", 0.9, { coreStands: true }), v("codex", "REFUTED", 0.9)]);
  check("refine fires when the core stands", G.needsRefine(core) === true);
  const pre = tally([
    v("claude", "REFUTED", 0.9, {
      coreStands: true,
      refutationType: "PRE_DECIDED",
      decisionQuestioned: "DEFERRED: this exact defect, per the thread",
    }),
    v("codex", "REFUTED", 0.9),
  ]);
  check("refine never fires on a pre-decided kill", G.needsRefine(pre) === false);
  // PR #1911 (2026-07-29): a PRE_DECIDED vote with no cited decision must NOT
  // get the pre-decided bypass — it is discarded back to an ordinary REFUTED
  // vote, so a core-stands kill with no real citation still goes to refine.
  const preNoCitation = tally([
    v("claude", "REFUTED", 0.9, { coreStands: true, refutationType: "PRE_DECIDED" }),
    v("codex", "REFUTED", 0.9),
  ]);
  check(
    "an uncited PRE_DECIDED vote does not set preDecided",
    preNoCitation.preDecided === false,
  );
  check(
    "refine fires on a core-stands kill with an uncited PRE_DECIDED vote",
    G.needsRefine(preNoCitation) === true,
  );
  const alive = tally([v("claude", "NOT_REFUTED", 0.9), v("codex", "NOT_REFUTED", 0.9)]);
  check("refine does not fire on a survivor", G.needsRefine(alive) === false);
}
{
  // v63 (2026-09-10). A PRE_DECIDED vote whose citation the gate rejected is an
  // ABSTENTION: out of the numerator and the denominator, exactly like an
  // UNAVAILABLE slot. Until this release the gate computed that rejection and
  // the tally ignored it, so the vote killed the finding anyway — BE PR #2025
  // died that way at 0.4619 against 0.45, and two of PR #1911's hand-verified
  // bugs before it.
  //
  // Every case below pairs the discarded vote with a NOT_REFUTED one, so the
  // discard is the only thing that can move refuteFraction from 1 to 0. A
  // fixture where both votes REFUTE would leave the fraction at 1 either way
  // and measure nothing.
  const keep = () => v("codex", "NOT_REFUTED", 0.9);
  const DECISIONS = [
    {
      category: "DEFERRED",
      summary: "ledger flush timing deferred by the author",
      source: "thread",
      files: ["scripts/invoices/other-file.ts"],
    },
  ];

  const uncited = G.tallyVotes(
    [v("claude", "REFUTED", 0.9, { refutationType: "PRE_DECIDED" }), keep()],
    [],
    "scripts/invoices/bulk-late-fee-application.ts",
    DECISIONS,
  );
  check(
    "a PRE_DECIDED vote citing nothing does not move refuteFraction",
    uncited.refuteFraction === 0,
    `rf=${uncited.refuteFraction}`,
  );
  check(
    "and the finding lives",
    uncited.survived === true && uncited.unverified === false,
  );
  check(
    "and the discard names WHICH condition failed, not just that one did",
    uncited.discardedVotes.length === 1 &&
      uncited.discardedVotes[0].reason === "no-decision-cited",
    JSON.stringify(uncited.discardedVotes),
  );

  const wrongFile = G.tallyVotes(
    [
      v("claude", "REFUTED", 0.9, {
        refutationType: "PRE_DECIDED",
        decisionQuestioned: "ledger flush timing deferred by the author",
      }),
      keep(),
    ],
    [],
    "scripts/invoices/bulk-late-fee-application.ts",
    DECISIONS,
  );
  check(
    "a PRE_DECIDED vote citing a decision scoped to another file does not count either",
    wrongFile.refuteFraction === 0 && wrongFile.survived === true,
    `rf=${wrongFile.refuteFraction}`,
  );
  check(
    "and it is distinguishable from the uncited case by its reason alone",
    wrongFile.discardedVotes.length === 1 &&
      wrongFile.discardedVotes[0].reason === "citation-does-not-cover-finding",
    JSON.stringify(wrongFile.discardedVotes),
  );

  // The control. Same vote, same decision, same weights — only the decision's
  // file scope changes. Without this, every assertion above is also satisfied
  // by a rule that discards EVERY PRE_DECIDED vote, which is not the rule.
  const covered = G.tallyVotes(
    [
      v("claude", "REFUTED", 0.9, {
        refutationType: "PRE_DECIDED",
        decisionQuestioned: "ledger flush timing deferred by the author",
      }),
      keep(),
    ],
    [],
    "scripts/invoices/other-file.ts",
    DECISIONS,
  );
  check(
    "a PRE_DECIDED vote whose citation DOES cover the finding counts normally",
    covered.refuteFraction === 0.5 &&
      covered.discardedVotes.length === 0 &&
      covered.preDecided === true,
    `rf=${covered.refuteFraction} discarded=${covered.discardedVotes.length} preDecided=${covered.preDecided}`,
  );

  // Every vote discarded. The fail-safe direction is that the finding lives.
  const allGone = G.tallyVotes(
    [
      v("claude", "REFUTED", 0.9, { refutationType: "PRE_DECIDED" }),
      v("claude", "REFUTED", 0.8, { refutationType: "PRE_DECIDED" }),
    ],
    [],
    "src/thing.ts",
    DECISIONS,
  );
  check(
    "a panel of nothing but discarded votes leaves the finding alive",
    allGone.refuteFraction === 0 && allGone.survived === true,
    `rf=${allGone.refuteFraction}`,
  );
  // These three read `valid`, not `tallyable`, on purpose: `unverified` means
  // every head failed to ANSWER, and the voter flags describe the panel the run
  // assembled. Pinned so the choice reads as deliberate if anyone widens the
  // filter later.
  check(
    "a discarded vote is still an answer, so the finding is not 'unverified'",
    allGone.unverified === false,
  );
  check(
    "and the panel it assembled is still what the voter flags report",
    allGone.hadIndependentVoter === true && uncited.hadExternalVoter === true,
    `indep=${allGone.hadIndependentVoter} external=${uncited.hadExternalVoter}`,
  );
}
{
  // v81 (BE #2167-B4, 2026-10-18). The finding sat on the Queue definition in
  // queue.ts and restated the author's BY-DESIGN decision, which the harvester
  // scoped to the workers that register that queue's schedulers. The file scope
  // set the vote aside, rightly by its rule, and the finding alone turned the run
  // to COMMENT. The tally is unchanged; contestedPreDecision is what routes such
  // a finding to the pre-decided section and out of the verdict.
  const keep = () => v("codex", "NOT_REFUTED", 0.9);
  const D = [
    {
      category: "BY-DESIGN",
      summary:
        "BY-DESIGN: retryUntilRedisReady is not added to the accounts, orders, catalog_search or worker.ts workers; each accounts scheduler registration already has its own try/catch",
      source: "bob, 2026-10-16",
      files: ["src/jobs/profiles_worker.ts", "src/jobs/orders_worker.ts", "src/jobs/worker.ts"],
    },
  ];
  const PR = ["src/jobs/queue.ts", ...D[0].files];
  const finding = {
    file: "src/jobs/queue.ts",
    title: "Accounts schedulers bypass the new startup retry",
    description: "signupRemindersQueue schedulers are registered at boot without retryUntilRedisReady",
  };
  const vote = v("claude", "REFUTED", 0.8, {
    refutationType: "PRE_DECIDED",
    decisionQuestioned: D[0].summary,
  });
  const t = G.tallyVotes([vote, keep()], [], finding.file, D);
  check(
    "the 2167 shape: the vote is still set aside and the finding still survives the panel",
    t.survived === true &&
      t.discardedVotes.length === 1 &&
      t.discardedVotes[0].reason === "citation-does-not-cover-finding",
    JSON.stringify(t.discardedVotes),
  );
  const c = G.contestedPreDecision(t.discardedVotes, finding, D, PR);
  check(
    "and it is marked possibly pre-decided, with the decision that matched",
    c && c.decisionSource === D[0].source && c.files.join() === D[0].files.join() && c.voteSource === "claude",
    JSON.stringify(c),
  );
  check(
    "not when the decision names a file outside this PR",
    G.contestedPreDecision(t.discardedVotes, finding, D, PR.slice(0, 2)) === null,
  );
  check(
    "not when the finding is outside this PR",
    G.contestedPreDecision(t.discardedVotes, finding, D, PR.slice(1)) === null,
  );
  check(
    "not when the finding's own words share nothing with the decision, whatever the refuter quoted",
    G.contestedPreDecision(t.discardedVotes, { file: finding.file, title: "Invoice totals rounded twice", description: "" }, D, PR) === null,
  );
  check(
    "not for a vote that cited nothing",
    G.contestedPreDecision([{ source: "claude", reason: "no-decision-cited", decisionQuestioned: null }], finding, D, PR) === null,
  );
  check(
    "not with no harvested decisions",
    G.contestedPreDecision(t.discardedVotes, finding, [], PR) === null,
  );
  check(
    "a path matches on a whole segment only",
    G.samePath("src/jobs/worker.ts", "worker.ts") &&
      !G.samePath("src/jobs/profiles_worker.ts", "worker.ts") &&
      G.preDecidedCoversFile("x", "src/jobs/profiles_worker.ts", [{ summary: "x", files: ["worker.ts"] }]) === false,
  );
}
{
  // The debate is the LAST gate — it runs on the final (post-refine) tally, and
  // it is the only one that sees a round-1 coin flip. Its surface conjunct came
  // off on 2026-07-24 because the ties it needs to catch are not high-risk.
  const tie = tally([
    v("claude", "NOT_REFUTED", 0.85, { coreStands: true }),
    v("codex", "REFUTED", 0.85),
  ]);
  check("debate fires on a borderline tie", G.shouldDebate(tie) === true, `rf=${tie.refuteFraction}`);
  check(
    "debate fires regardless of surface",
    G.shouldDebate(tie) === true && tie.borderline === true,
  );
  check(
    "debate does not fire on a clean kill",
    G.shouldDebate(tally([v("claude", "REFUTED", 0.9), v("codex", "REFUTED", 0.95)])) === false,
  );
  check(
    "debate does not fire on a clean survival",
    G.shouldDebate(tally([v("claude", "NOT_REFUTED", 0.9), v("codex", "NOT_REFUTED", 0.95)])) === false,
  );
  const none = G.tallyVotes([null], [{ source: "claude" }]);
  check("debate does not fire on an unverified panel", G.shouldDebate(none) === false);
}
{
  // borderline IMPLIES a verdict split — arithmetic, not a corpus accident, and
  // the reason shouldDebate needs no VERDICT_SPLIT conjunct. refuteFraction is
  // refuteWeight/(refuteWeight+keepWeight), so it only lands strictly between 0
  // and 1 when both sides have weight. Any unanimous panel is 0 or 1 exactly,
  // and |0 - 0.45| and |1 - 0.45| both exceed DELTA.
  let counterexample = null;
  for (const conf of [0, 0.1, 0.3, 0.5, 0.6, 0.7, 0.9, 1]) {
    for (const src of ["claude", "codex", "gemini"]) {
      for (const verdict of ["REFUTED", "NOT_REFUTED"]) {
        for (const n of [1, 2, 3]) {
          const t = tally(Array.from({ length: n }, () => v(src, verdict, conf)));
          if (t.borderline) counterexample = `${n}x ${src} ${verdict} @${conf}`;
        }
      }
    }
  }
  check(
    "no unanimous panel is ever borderline (borderline => split)",
    counterexample === null,
    counterexample,
  );
}

// --- 3. Corpus replay ------------------------------------------------------
const rows = readFileSync(METRICS, "utf8")
  .split("\n")
  .filter(Boolean)
  .map((l, i) => {
    try {
      return JSON.parse(l);
    } catch {
      console.log(`  (skipping unparseable metrics line ${i + 1})`);
      return null;
    }
  })
  .filter(Boolean);

// Which votes the recorded tally was actually computed over, when a row keeps
// more than one round. A REFINE replaces the panel, so its round-1 votes
// supersede round 0 and the tally is the final round's alone. Every other
// multi-round shape is ADDITIVE — the later votes joined the same panel — and
// the tally is the union.
//
// This is read off the corpus, not assumed: of the 3 multi-round panels on
// record, the two with `refined: false` (#1994, #1970) record the union, and
// the one with `refined: true` (#1540) records the final round. Replaying every
// one of them the same way is what made this assert fail — as the union it
// missed #1540, as the final round it missed the other two.
//
// Workflow rows never exercise this: they keep only the surviving set. It shows
// up on in-context rows, where `round` is a schema'd vote field precisely so
// both can be kept. Votes with no `round` are round 0, which is every
// pre-schema row.
const tallyVotesFor = (f) => {
  const votes = f.votes;
  if (!f.refined) return votes;
  const round = (v) => (typeof v?.round === "number" ? v.round : 0);
  const last = Math.max(...votes.map(round));
  return votes.filter((v) => round(v) === last);
};

const panels = [];
for (const r of rows) {
  if (!Array.isArray(r.findings)) continue;
  for (const f of r.findings) {
    if (!f || typeof f !== "object") continue;
    if (!f.panelRan || !Array.isArray(f.votes) || f.votes.length === 0) continue;
    panels.push({
      pr: r.pr,
      ts: r.ts,
      f,
      tallyVotes: tallyVotesFor(f),
      decisions: Array.isArray(r.inThreadDecisions) ? r.inThreadDecisions : null,
    });
  }
}

console.log(`\n--- corpus: ${panels.length} recorded panels over ${rows.length} runs ---`);

// Fidelity. The recorded `votes` array holds only the votes actually CAST
// (failed heads live in failedSlots), which is exactly `tally.valid` — so a
// faithful replay must reproduce the recorded refuteFraction to the digit. Any
// row that does not is EXCLUDED from every conclusion below rather than quietly
// averaged in: a reconstruction that disagrees with the record is not evidence
// about the gate, it is evidence about the reconstruction.
//
// One class of row is EXPECTED to disagree from v63 (2026-09-10) on, and it is
// separated out here rather than excused by a looser tolerance. A REFUTED vote
// claiming PRE_DECIDED with nothing cited no longer counts in the tally, so
// every row carrying one was recorded under arithmetic that no longer applies.
// Those rows are named and counted in their own check below; every OTHER row
// must still reproduce to the digit, which is what this one is for.
let faithful = 0;
const unfaithful = [];
const rewritten = [];
// A row from v81 on carries the decisions its gate checked, so the replay can
// run the file-scope half of the rule too. A discard the row itself recorded,
// with the same reasons, is the arithmetic that row was written under, so it is
// compared to the digit like any other; only a discard the row never knew about
// marks it as rewritten.
const sameDiscards = (f, t) =>
  Array.isArray(f.discardedVotes) &&
  f.discardedVotes.map((d) => d.reason).sort().join() === t.discardedVotes.map((d) => d.reason).sort().join();
for (const p of panels) {
  const t = p.decisions
    ? G.tallyVotes(p.tallyVotes, panelOf(p.tallyVotes), p.f.file, p.decisions)
    : tally(p.tallyVotes);
  const recorded = p.f.refuteFraction;
  p.tally = t;
  if (t.discardedVotes.length && !sameDiscards(p.f, t)) {
    rewritten.push(p);
    p.faithful = false;
    continue;
  }
  const ok =
    p.f.unverified || typeof recorded !== "number"
      ? true
      : Math.abs(t.refuteFraction - recorded) < 1e-6;
  p.faithful = ok;
  if (ok) faithful++;
  else unfaithful.push(p);
}
check(
  `tally replay reproduces the recorded refuteFraction (${faithful}/${panels.length - rewritten.length})`,
  unfaithful.length === 0,
  unfaithful
    .slice(0, 5)
    .map((p) => `#${p.pr} "${p.f.title?.slice(0, 40)}" recorded=${p.f.refuteFraction} replayed=${p.tally.refuteFraction.toFixed(3)}`)
    .join(" | "),
);

// The retroactive measurement, pinned. These rows were recorded before the rule
// existed, so the count is a property of the stored corpus: it moves when the
// corpus grows, and if it moves without a new run then the predicate widened
// and caught something it should not have.
//
// Measured 2026-09-10 over 317 recorded panels: six, all on PR #1911, all for
// the same reason. ONE comes back to life — "No sanity check before generating
// mass-delete SQL if partner export is empty", 0.5826 to 0.0000, killed then,
// alive now, and hand-verified after the fact as a real unrebutted bug. Three
// move and still die; two had survived anyway. One recovered finding out of six
// touched, which is worth saying plainly: this is not a mass amnesty.
//
// The rule's OTHER half — a citation naming a decision that does not cover the
// finding's file — cannot be measured from this corpus at all, because the
// harvested `decisions` list is not persisted on the row and
// `preDecidedCoversFile` is permissive when it is empty. Its live instance is
// PR #2025 (2026-09-09), and that one is provable from the record without the
// decisions: the finding carries `refined: true`, `needsRefine` requires
// `!preDecided`, so the gate had already rejected that citation while the vote
// killed the finding anyway at 0.4619 against a 0.45 threshold.
check(
  `exactly the known uncited-PRE_DECIDED rows are rewritten (${rewritten.length})`,
  rewritten.length === 6 &&
    rewritten.every((p) => p.pr === 1911) &&
    rewritten.every((p) =>
      p.tally.discardedVotes.every((d) => d.reason === "no-decision-cited"),
    ),
  rewritten
    .map((p) => `#${p.pr} "${p.f.title?.slice(0, 32)}" ${p.f.refuteFraction?.toFixed(3)}->${p.tally.refuteFraction.toFixed(3)}`)
    .join(" | "),
);
check(
  "one of the six comes back to life, which is the whole reason for the rule",
  rewritten.filter((p) => p.f.survived === false && p.tally.survived === true)
    .length === 1,
  rewritten.map((p) => `${p.f.survived}->${p.tally.survived}`).join(" "),
);

// `borderline` is the debate gate's whole input, so it gets its own fidelity
// assert rather than riding on refuteFraction's: a hoisted DELTA or T_KILL that
// silently changed would move this and not that.
const borderlineMismatch = panels.filter(
  (p) =>
    p.faithful &&
    typeof p.f.borderline === "boolean" &&
    p.tally.borderline !== p.f.borderline,
);
check(
  `tally replay reproduces the recorded borderline flag`,
  borderlineMismatch.length === 0,
  borderlineMismatch
    .slice(0, 5)
    .map((p) => `#${p.pr} rf=${p.f.refuteFraction} recorded=${p.f.borderline} replayed=${p.tally.borderline}`)
    .join(" | "),
);

// The theorem again, this time on real data rather than constructed tallies.
const borderlineUnanimous = panels.filter(
  (p) =>
    p.faithful &&
    p.tally.borderline &&
    new Set(p.f.votes.map((x) => x.verdict)).size === 1,
);
check(
  "no recorded borderline panel is unanimous",
  borderlineUnanimous.length === 0,
  `${borderlineUnanimous.length} found`,
);

// Replayability for the ESCALATION gate specifically.
//
// The recorded vote set is the FINAL one. It equals the round-0 set only when
// the finding neither escalated nor refined — an escalation appends a vote and
// a refine re-refutes, and neither is reversible from the row. So the escalation
// question is asked only of rows where round 0 is intact, and the rest are
// counted, not guessed at.
const clean = panels.filter((p) => p.faithful && !p.f.escalated && !p.f.refined);
const skippedEsc = panels.filter((p) => p.faithful && (p.f.escalated || p.f.refined));

console.log(
  `\nEscalation gate — round-0-intact subset: ${clean.length} panels ` +
    `(${skippedEsc.length} excluded: ${panels.filter((p) => p.faithful && p.f.escalated).length} escalated, ` +
    `${panels.filter((p) => p.faithful && p.f.refined).length} refined — their round-0 votes are not recoverable)`,
);

// `touchesHighRiskSurface` is NOT persisted per finding, so the gate's
// high-risk branch (borderline / under-confident on a high-risk surface) cannot
// be replayed at all. What CAN be replayed exactly is the branch that changed
// on 2026-07-24: a verdict split now escalates on ANY surface. Everything below
// is about that branch only, and reads `false` for the high-risk flag — which
// makes the counts a LOWER BOUND on what the shipping gate fires.
const splitFires = clean.filter((p) =>
  G.shouldEscalate(p.tally, { touchesHighRiskSurface: false }, claudeVotesOf(p)),
);

function claudeVotesOf(p) {
  return p.f.votes.filter((x) => (x.source || "claude") === "claude").length;
}

const newlyEscalating = splitFires.filter((p) => !p.f.escalated);
const wouldRescue = newlyEscalating.filter((p) => p.f.survived === false);

console.log(
  `  split branch fires on ${splitFires.length} of them; ${newlyEscalating.length} were NOT escalated when the run happened`,
);
console.log(
  `  of those, ${wouldRescue.length} were recorded as KILLED — the population the change targets`,
);

// --- Where the coin-flip kills actually live -------------------------------
//
// The count above is ZERO, and that is the finding rather than a null result.
// shouldEscalate is evaluated ONCE, on the round-0 tally, before the refine
// loop. After a refine the panel is REPLACED (peer-review.js:
// `panel = [slot("claude", ...), ...revoteSources]`) and the re-refutation is
// final, so no round-0 gate ever sees a round-1 tie.
//
// Every recorded 1-1 split carrying a single Claude voter — the only shape the
// escalation gate can apply to — is a POST-REFINE state, including PR #1968's
// hand-verified false negative at refuteFraction 0.500. Widening the escalation
// surface on 2026-07-24 therefore could not reach the population that motivated
// it. The gate that DOES see these is the debate, below.
const splitPanels = panels.filter(
  (p) => p.faithful && new Set(p.f.votes.map((x) => x.verdict)).size > 1,
);
const singleClaudeSplits = splitPanels.filter((p) => claudeVotesOf(p) === 1);
const postRefine = singleClaudeSplits.filter((p) => p.f.refined);

console.log(`\nWhere the coin-flip kills actually live:`);
console.log(`  split-verdict panels overall:            ${splitPanels.length}`);
console.log(
  `  ...of which the escalation gate could apply to: ${singleClaudeSplits.length} (exactly 1 Claude voter)`,
);
console.log(
  `  ...of which are POST-REFINE states:     ${postRefine.length}` +
    (singleClaudeSplits.length === postRefine.length && postRefine.length > 0
      ? "  <-- ALL of them"
      : ""),
);
console.log(
  `  killed among those:                     ${postRefine.filter((p) => p.f.survived === false).length}`,
);
const band = postRefine
  .map((p) => p.f.refuteFraction)
  .filter((x) => typeof x === "number")
  .sort((a, b) => a - b);
if (band.length)
  console.log(
    `  their refuteFraction band:              ${band[0].toFixed(2)}–${band[band.length - 1].toFixed(2)} (T_KILL=${G.T_KILL})`,
  );

// --- Debate gate: the one that runs on the FINAL tally ---------------------
//
// shouldDebate is the last gate and the only one evaluated after the round-1
// re-vote, which is exactly where the ties above live. Its surface conjunct
// came off on 2026-07-24 for that reason. Unlike the escalation replay, this
// one needs no round-0 reconstruction: the recorded tally IS the tally the
// debate gate saw, so every panel is answerable.
const wouldDebate = panels.filter((p) => p.faithful && G.shouldDebate(p.tally));
const alreadyDebated = wouldDebate.filter((p) => p.f.debate?.fired);
const newlyDebating = wouldDebate.filter((p) => !p.f.debate?.fired);
const newlyDebatingKills = newlyDebating.filter((p) => p.f.survived === false);
const outcomes = panels
  .filter((p) => p.f.debate?.fired)
  .reduce((acc, p) => {
    const k = p.f.debate.outcome || "unknown";
    acc[k] = (acc[k] || 0) + 1;
    return acc;
  }, {});
const flipped = outcomes.flipped || 0;
const totalDebates = Object.values(outcomes).reduce((a, b) => a + b, 0);
const runsWithFindings = rows.filter((r) => Array.isArray(r.findings)).length;

console.log(`\nDebate gate (shouldDebate — runs on the final tally):`);
console.log(`  panels the gate now fires on:           ${wouldDebate.length}`);
console.log(`  ...already debated under the old gate:  ${alreadyDebated.length}`);
console.log(
  `  ...NEWLY debating:                      ${newlyDebating.length}, of which ${newlyDebatingKills.length} are recorded KILLS`,
);
console.log(
  `  recorded debate outcomes so far:        ${JSON.stringify(outcomes)}` +
    (totalDebates
      ? `  (${Math.round((flipped / totalDebates) * 100)}% flip rate)`
      : ""),
);
console.log(
  `  marginal cost:                          ${(
    (2 * newlyDebating.length) /
    Math.max(1, runsWithFindings)
  ).toFixed(2)} agents/run averaged over ${runsWithFindings} runs (2 agents per debate)`,
);

if (VERBOSE && newlyDebating.length) {
  console.log(`\n  newly-debating panels:`);
  for (const p of newlyDebating)
    console.log(
      `    PR #${p.pr}  rf=${p.f.refuteFraction.toFixed(3)}  ` +
        `${p.f.survived ? "survived" : "KILLED  "}  ${(p.f.severity || "?").padEnd(9)} ${p.f.title?.slice(0, 55)}`,
    );
}

// Regression check on the refine gate, which this round did NOT change.
// It can only be asked of rows that did not refine: for those the recorded
// tally IS the tally the gate saw, so needsRefine() must agree it had no reason
// to fire — unless the run ran out of refine budget, which the row does not
// record per finding. Rows that DID refine are unaskable (their recorded tally
// is the post-refine one), and are reported rather than assumed.
const notRefined = panels.filter((p) => p.faithful && !p.f.refined);
const wouldRefineAnyway = notRefined.filter((p) => G.needsRefine(p.tally));
console.log(
  `\nRefine gate — ${notRefined.length} non-refined panels; ` +
    `${wouldRefineAnyway.length} where needsRefine() is true on the recorded tally`,
);
console.log(
  `  (a non-zero count here is expected, not a defect: refineBudget exhaustion ` +
    `is per-run and not recorded per finding — see refinesSkippedForBudget)`,
);

// --- 4. What this replay does NOT establish --------------------------------
console.log(`
What this does NOT establish, stated so nobody reads more into it:
  · It replays which findings a gate ROUTES, never what the extra agent would
    have SAID. The flip rate above is how often the adjudicator DISAGREED with
    the panel — not how often it was right. Whether the newly-debating kills are
    real defects is a corpus question (eval/README.md), not a replay one.
  · Round-0 tallies for refined findings are gone: the row keeps the FINAL vote
    set. So this cannot say how often round 0 itself lands 1-1 — only that the
    ties which survive into the record are round-1 ties.
  · The high-risk branch is unreplayable: touchesHighRiskSurface is not
    persisted per finding, so every count above is a lower bound.
  · ${skippedEsc.length} panels are excluded because escalating or refining overwrote the
    round-0 vote set they would have been judged on.
  · Nothing here checks dispatch, batching, prompts or persistence. Those are
    Tier-1 territory (a pinned micro-run on PR #1935, ~20k tokens).
`);

console.log(`\n--- fact/judgement split (v91) ---`);
{
  const q = { file: "a.ts", line: 3, quote: "if (!x) return;" };
  const v2 = (verdict, conf, extra = {}) =>
    v("claude", verdict, conf, { introducedByPr: "yes", proposedSeverity: "important", ...extra });
  const judged = tally([v2("REFUTED", 0.9, { killReason: "pre-existing" }), v2("NOT_REFUTED", 0.6)]);
  check("a v2 judgement kill is set aside, so the finding survives", judged.survived === true && judged.discardedVotes[0]?.reason === "judgement-kill-reason", JSON.stringify(judged.discardedVotes));
  const unquoted = tally([v2("REFUTED", 0.9, { killReason: "incorrect-premise" }), v2("NOT_REFUTED", 0.6)]);
  check("a v2 factual kill without a quote is set aside", unquoted.survived === true && unquoted.discardedVotes[0]?.reason === "no-contradicting-quote");
  const factual = tally([v2("REFUTED", 0.9, { killReason: "not-triggered", contradiction: q }), v2("NOT_REFUTED", 0.6)]);
  check("a v2 factual kill with a quote counts", factual.survived === false && factual.discardedVotes.length === 0, `rf=${factual.refuteFraction}`);
  const invented = v2("REFUTED", 0.9, { refutationType: "PRE_DECIDED", decisionQuestioned: "deferred by the author" });
  const noDecisions = G.tallyVotes([invented, v2("NOT_REFUTED", 0.6)], [], "a.ts", [], false);
  check("a v2 PRE_DECIDED vote abstains when the refuters were given no decisions", noDecisions.survived === true && !noDecisions.preDecided && noDecisions.discardedVotes[0]?.reason === "no-decisions-given", JSON.stringify(noDecisions.discardedVotes));
  const given = G.tallyVotes([invented, v2("NOT_REFUTED", 0.6)], [], "a.ts", [], true);
  check("with decisions given it is judged by its citation as before", given.preDecided === true && given.discardedVotes.length === 0);
  const legacyPre = G.tallyVotes([{ ...invented, introducedByPr: undefined }, v("claude", "NOT_REFUTED", 0.6)], [], "a.ts", [], false);
  check("a legacy PRE_DECIDED vote is unaffected by decisionsGiven", legacyPre.preDecided === true);
  const legacy = tally([v("claude", "REFUTED", 0.9, { killReason: "pre-existing" }), v("claude", "NOT_REFUTED", 0.6)]);
  check("a legacy judgement kill still counts as before", legacy.survived === false && legacy.discardedVotes.length === 0);

  const f = { severity: "important" };
  const pre = (n) => v2("NOT_REFUTED", 0.8, { introducedByPr: n });
  check("a majority saying pre-existing places it there", G.placeSurvivor([pre("no"), pre("no"), pre("yes")], f).placement === "pre-existing");
  check("one voter of two cannot call it pre-existing", G.placeSurvivor([pre("no"), pre("yes")], f).placement === null);
  check("an added line vetoes pre-existing", G.placeSurvivor([pre("no"), pre("no")], f, true).placement === null);
  check("a single live vote cannot call it pre-existing", G.placeSurvivor([pre("no")], f).placement === null);
  check("a single live vote cannot call it intended", G.placeSurvivor([v2("NOT_REFUTED", 0.8, { intendedBy: { file: "CLAUDE.md", rule: "x is by design" } })], f).placement === null);
  const rule = { file: "CLAUDE.md", rule: "Never hardcode HTTP status numbers" };
  check("a majority quoting a convention rule places it as intended", G.placeSurvivor([v2("NOT_REFUTED", 0.8, { intendedBy: rule }), v2("NOT_REFUTED", 0.8, { intendedBy: rule })], f).intendedBy?.rule === rule.rule);
  const low = G.placeSurvivor([v2("NOT_REFUTED", 0.8, { proposedSeverity: "nit" }), v2("NOT_REFUTED", 0.8, { proposedSeverity: "nit" })], f);
  check("a majority's lower severity is recorded and places nothing", low.placement === null && low.proposedSeverity === "nit" && !("severity" in low));
  const up = G.placeSurvivor([v2("NOT_REFUTED", 0.8, { proposedSeverity: "critical" }), v2("NOT_REFUTED", 0.8, { proposedSeverity: "critical" })], f);
  check("a higher proposal is not recorded", up.proposedSeverity === null);
  const split = G.placeSurvivor([v2("NOT_REFUTED", 0.8, { proposedSeverity: "nit" }), v2("NOT_REFUTED", 0.8)], f);
  check("one of two proposing lower is not a majority", split.proposedSeverity === null);
  check("legacy votes place nothing", G.placeSurvivor([v("claude", "NOT_REFUTED", 0.8), v("codex", "NOT_REFUTED", 0.8)], f).placement === null);
}

console.log(`\n--- wiring of the possibly-pre-decided route (v81) ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "it is computed only for a finding the panel kept, from this PR's decisions and files",
    /possiblyPreDecided: survived\s*\?\s*contestedPreDecision\(\s*tally\.discardedVotes,\s*finding,\s*prContext\.inThreadDecisions,\s*prContext\.files,?\s*\)\s*:\s*null/.test(src),
  );
  check(
    "such a finding is not a survivor for the verdict, and is reported as pre-decided",
    /const surviving = results\.filter\(\s*\(v\) => v\.survived && !v\.possiblyPreDecided/.test(src) &&
      src.includes("const preDecided = [...refuted.filter((v) => v.preDecided), ...contested];") &&
      /verdictFor\(\s*(\/\/[^\n]*\n\s*)+(withRiskPathSurface\()?survivingFindingsForSynthesis[,)]/.test(src) &&
      /const survivingFindingsForSynthesis = verifiedGateDemoteUnanchored\(\s*surviving\.map/.test(src),
  );
  check(
    "the report writer gets the mark",
    src.includes("POSSIBLY PRE-DECIDED: <which decision it questioned>") &&
      /possiblyPreDecided: \{\s*file: v\.finding\.file,/.test(src),
  );
  check(
    "the row keeps the mark and the decisions the gate checked",
    src.includes("possiblyPreDecided: v.possiblyPreDecided || null,") && /^  inThreadDecisions: \(prContext\.inThreadDecisions \|\| \[\]\)\.map/m.test(src),
  );
}

console.log(`\n--- wiring of placeSurvivor (v91) ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "every tally call tells the gate whether the refuters were given decisions",
    (src.match(/tallyVotes\(\s*votes,\s*panel,\s*(?:r\.)?finding\.file,\s*prContext\.inThreadDecisions,\s*Boolean\(combinedDecisions\),?\s*\)/g) || []).length === 5,
  );
  check(
    "placement is computed only for a kept finding, with the added-line and re-anchored vetoes",
    /placement: survived\s*\?\s*placeSurvivor\(\s*tally\.valid,\s*workingFinding,(\s*\/\/[^\n]*)*\s*findingLineAdded\(workingFinding\) \|\| Boolean\(workingFinding\.reanchored\),?\s*\)\s*:\s*null/.test(src),
  );
  check(
    "a placed finding leaves the verdict and keeps a report section",
    /const surviving = results\.filter\(\s*\(v\) => v\.survived && !v\.possiblyPreDecided && !v\.placement\?\.placement,?\s*\)/.test(src) &&
      src.includes("<placed_aside note=") &&
      src.includes("### Pre-existing or required by a convention (not counted toward the verdict)"),
  );
  check(
    "the row sees it",
    src.includes("placement: v.placement || null,") && src.includes("placedAside: placedAside.length,"),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (${panels.length} panels replayed, 0 tokens).\n`);
