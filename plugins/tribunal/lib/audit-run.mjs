#!/usr/bin/env node
// audit-run.mjs — check a finished peer-review run against invariants derived
// from ways this harness has actually broken.
//
// Usage:
//   node audit-run.mjs <artifact-dir> [run-start-epoch-seconds] < metrics.json
//
// The run-start argument is optional and normally omitted: the artifact
// directory is created by `mktemp -d` at the start of the run, so its own
// birth time IS the run start. That matters because the workflow script cannot
// call Date.now() (it would break resume), and a timestamp threaded through an
// agent is a timestamp that can come back wrong.
//
// Output: one line of JSON — {passed, checked, violations:[{id, detail}],
// notes:[{id, detail}]}. `passed` is a function of `violations` alone; `notes`
// records a check whose signal is real but whose accuracy is not established,
// so it stays visible without failing the run's audit.
// Exit 0 always. A violation is data for the row, not a crash: the report is
// already written and may still be useful. What must not happen is the run
// reading as clean.
//
// Why this exists. On 2026-07-23 a run produced three defects at once and every
// one of them was ALREADY WRITTEN IN ITS OWN METRICS ROW: hadExternalVoter 0 of
// 26 panels, orphan-artifact 67, reconciliation checking 18 of 39 slots. All
// three were found by a human reading /var/folders by hand, hours later. The
// harness had the evidence and no eyes.
//
// This is deliberately NOT more of the offline replay battery. Eight replays
// and twenty mirror invariants were green through all three defects, because
// they check the code against itself. These check what a run PRODUCED. That is
// the gap, and it is the only kind of check that could have caught any of them.
//
// Every invariant below names the failure it came from. An invariant with no
// such story is a guess, and guesses are how a checker becomes noise that gets
// ignored — at which point it is worse than not having one.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const dir = process.argv[2] || "";
let runStart = Number(process.argv[3] || 0);
if (!runStart && dir && existsSync(dir)) {
  const st = statSync(dir);
  // birthtime where the filesystem records it, ctime as the fallback: either is
  // the moment mktemp -d created this directory, which is the run's own start.
  runStart = Math.floor(
    (st.birthtimeMs && st.birthtimeMs > 0 ? st.birthtimeMs : st.ctimeMs) / 1000,
  );
}

let m;
try {
  m = JSON.parse(readFileSync(0, "utf8") || "{}");
} catch {
  console.log(
    JSON.stringify({
      passed: false,
      checked: 0,
      violations: [{ id: "metrics-unreadable", detail: "metrics on stdin were not valid JSON" }],
    }),
  );
  process.exit(0);
}

const findings = Array.isArray(m.findings) ? m.findings : [];
const violations = [];
const v = (id, detail) => violations.push({ id, detail });
// A second channel that records without failing. `passed` stays a function of
// `violations` alone, so a note is visible on the row and to --metrics /
// --review-health while costing no run its audit. It exists for a check whose
// signal is real but whose accuracy is not yet established.
//
// EMPTY as of 2026-09-10 (v63): its only tenant, fidelity-critic-flagged, was
// promoted back to `v()` once the rule caught real drift. The channel stays
// because the demote-measure-promote cycle it made possible is the point, not
// that one rule, and the next check with a real signal and no track record
// belongs here rather than straight into `violations`. Every audit result still
// carries a `notes` array, so a reader cannot mistake absent for empty.
const notes = [];
const n = (id, detail) => notes.push({ id, detail });
let checked = 0;
const check = (fn) => {
  checked++;
  try {
    fn();
  } catch (err) {
    v("invariant-crashed", String(err));
  }
};

// 1 — External heads were enabled and panels ran, but nothing external voted.
// The 2026-07-23 case: 26 panels, 0 external voters. The panel's entire premise
// is decorrelation; without it the run is 26 same-lineage opinions wearing the
// costume of a panel, and the report says nothing about the difference.
check(() => {
  const panels = findings.filter((f) => f.panelRan).length;
  const external = findings.filter((f) => f.hadExternalVoter).length;
  if ((m.codexEnabled || m.geminiEnabled) && panels > 0 && external === 0) {
    // Name the cause when the heads said it. On 2026-10-18 every row of an
    // afternoon carried this violation and none said the Codex account was
    // out of credits.
    const why = {};
    for (const f of findings)
      for (const d of f.failedSlotsDetail || [])
        if (d && d.source && d.source !== "claude")
          why[`${d.source}: ${d.reason}`] = (why[`${d.source}: ${d.reason}`] || 0) + 1;
    const causes = Object.entries(why).map(([k, n]) => `${k} x${n}`).join(", ");
    v(
      "no-external-voter",
      `${panels} panel(s) ran with an external head enabled and NOT ONE received an external vote — the panel ran without decorrelation${causes ? ` (${causes})` : ""}`,
    );
  }
});

// 2 — Artifacts nobody voted on. This is the signature of the split directory:
// heads wrote their answers somewhere the classifier did not read.
check(() => {
  const orphans = (m.voteReconciliation?.counts || {})["orphan-artifact"] || 0;
  if (orphans > 0)
    v(
      "orphan-artifacts",
      `${orphans} CLI artifact(s) belong to no recorded vote — the answers were written somewhere the tally never read`,
    );
});

// 3 — The audit covered far fewer slots than the run dispatched. 18 of 39 read
// as a clean bill of health on less than half the panel.
check(() => {
  const audited = m.voteReconciliation?.checked;
  const expected = m.externalSlots;
  if (typeof audited === "number" && typeof expected === "number" && expected > 0) {
    if (audited < expected - 1)
      v(
        "reconciliation-incomplete",
        `reconciliation checked ${audited} of ${expected} external slot(s) — the unchecked remainder is not evidence of anything`,
      );
  }
});

// 3b — The reconciliation RAN but audited nothing, because the verdict
// classifier itself could not be spawned (a Windows spawn failure). Check 3 above cannot see
// this: `checked` counts votes ATTEMPTED, not votes audited, so a run where
// every classification died still reports `checked === externalSlots` and
// passes. The result would be `selfAudit.passed === true`, zero violations,
// persisted — a clean bill of health for a vote audit that was inert end to end,
// which is the one outcome this file's header says must never happen.
//
// Would, not did: the defect is unreachable on a Claude-only panel, because
// lib/persist-run.sh only spawns the reconciler when the run recorded external
// votes. It goes live the day a codex or gemini head is installed, which is why
// this invariant lands with the fix rather than after the first bad run.
//
// Deliberately separate from `reconciliation-incomplete` rather than folded into
// it. That one means "we looked at some slots and not others"; this one means
// "we looked at none of them, and the instrument is what broke". Collapsing the
// two would repeat the substitution the classifier-spawn fix removed one layer down, where a
// dead classifier was reported as `vote-without-verdict` — a claim about votes
// nobody read.
check(() => {
  const failed = m.voteReconciliation?.classifierFailed || 0;
  if (failed > 0)
    v(
      "classifier-unavailable",
      `the verdict classifier could not run for ${failed} vote(s) — those votes are UNAUDITED, neither confirmed clean nor known deviant, and nothing in this run's reconciliation is evidence about them`,
    );
});

// 4 — Regression guard for the fix made the same morning: a finding nothing
// contested cannot be blocking. It held on the run that exposed it; it stays
// pinned because it is one line away from silently regressing.
check(() => {
  const bad = findings.filter((f) => f.blocking && !f.adjudicated);
  if (bad.length)
    v(
      "blocking-without-adjudication",
      `${bad.length} finding(s) marked blocking that no panel and no typecheck examined: ${bad.map((f) => f.title).slice(0, 3).join("; ")}`,
    );
});

// 5 — Artifacts older than the run. The directory used to be named per-PR, so
// five reviews of one PR shared it and the append-only invocation counter summed
// across all of them — which is what made ordinary slots read as `over-invoked`
// and sent a whole piece of work chasing a protocol breach that never happened.
check(() => {
  if (!dir || !runStart || !existsSync(dir)) return;
  let stale = 0;
  let oldest = null;
  for (const f of readdirSync(dir)) {
    if (!f.endsWith(".meta") && !f.endsWith(".out")) continue;
    const mt = statSync(join(dir, f)).mtimeMs / 1000;
    if (mt < runStart) {
      stale++;
      if (oldest === null || mt < oldest) oldest = mt;
    }
  }
  if (stale > 0)
    v(
      "stale-artifacts",
      `${stale} artifact(s) in ${dir} predate this run (oldest ${new Date(oldest * 1000).toISOString()}) — counts read from them include earlier runs`,
    );
});

// 6 — The agent-cost model vs what the run actually spent. The model was
// calibrated on ONE run, so it will drift; the point is that the drift gets
// reported rather than quietly believed.
check(() => {
  const projected = m.agentsProjected;
  const actual = m.agentsActual;
  if (typeof projected === "number" && typeof actual === "number" && actual > 0) {
    const err = Math.abs(actual - projected) / actual;
    if (err > 0.2)
      v(
        "cost-model-drift",
        `projected ~${projected} agents, spent ${actual} (${Math.round(err * 100)}% off) — recalibrate AGENTS_PER_PANEL / AGENTS_PER_REFINE against this run`,
      );
  }
});

// 7 — A merge with more than one distinct location silently lost the extras
// from the printed report. mergeCandidates carries every member's own
// file:line specifically so a wrong merge degrades to a noisy finding instead
// of a silent deletion (see its own comment in workflows/tribunal.js) — but
// that guarantee lived only in a prose instruction inside the synthesis
// prompt ("you MUST print the additional ones... as 'also at:'"), sitting
// among ~30 other grounding rules in the longest single-context prompt in the
// harness. That is exactly the shape attention dilution drops a rule from,
// not a defect in the rule itself. This does not re-derive the prompt's
// instruction — `mergedLocationsMissingFromReport` is computed in-process
// against the model's actual output text before this row is ever written, so
// it checks what the run PRODUCED, the same bar every other invariant here
// holds to.
check(() => {
  const bad = findings.filter(
    (f) =>
      Array.isArray(f.mergedLocationsMissingFromReport) &&
      f.mergedLocationsMissingFromReport.length > 0,
  );
  if (bad.length)
    v(
      "merged-locations-dropped",
      `${bad.length} merged finding(s) lost one or more extra locations from the printed report: ${bad
        .map((f) => `${f.title} (missing ${f.mergedLocationsMissingFromReport.join(", ")})`)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 8 — killReason cited but the report never said it. Same rule as invariant 7,
// applied to a different grounding rule: the synthesis prompt tells the model
// to cite a refuted finding's killReason category (or "uncategorized" when
// absent), and nothing checked whether it actually did until now.
check(() => {
  const bad = findings.filter(
    (f) =>
      Array.isArray(f.killReasonMissingFromReport) &&
      f.killReasonMissingFromReport.length > 0,
  );
  if (bad.length)
    v(
      "kill-reason-not-cited",
      `${bad.length} refuted finding(s) whose killReason category never made it into the report: ${bad
        .map((f) => f.title)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 9 — mergedFrom had 2+ contributing lenses but the "(found by: ...)" note is
// missing from the report, or names fewer lenses than actually merged.
check(() => {
  const bad = findings.filter(
    (f) =>
      Array.isArray(f.mergedFromAnnotationMissingFromReport) &&
      f.mergedFromAnnotationMissingFromReport.length > 0,
  );
  if (bad.length)
    v(
      "merged-from-annotation-dropped",
      `${bad.length} merged finding(s) with 2+ contributing lenses lost the "(found by: ...)" annotation: ${bad
        .map((f) => f.title)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 10 — grounded.verdict "confirmed" but its diagnostic never appears verbatim
// in the report — the synthesis prompt requires the exact tsc string, never
// an invented one, and this is the check that would catch either failure.
check(() => {
  const bad = findings.filter(
    (f) =>
      Array.isArray(f.groundedDiagnosticMissingFromReport) &&
      f.groundedDiagnosticMissingFromReport.length > 0,
  );
  if (bad.length)
    v(
      "grounded-diagnostic-not-cited",
      `${bad.length} typecheck-confirmed finding(s) whose tsc diagnostic never appeared verbatim in the report: ${bad
        .map((f) => f.title)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 10b — the report called a gate-demoted finding unexamined although its panel
// voted (issue #46). Unlike 7-10 this is wording the report must not print.
check(() => {
  const bad = findings.filter(
    (f) =>
      Array.isArray(f.unexaminedWordingInReport) &&
      f.unexaminedWordingInReport.length > 0,
  );
  if (bad.length)
    v(
      "gate-demoted-called-unexamined",
      `${bad.length} gate-demoted finding(s) whose panel voted were reported as unexamined: ${bad
        .map((f) => `${f.title} (${f.unexaminedWordingInReport.join(", ")})`)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 11 — the fidelity critic flagged a mismatch between the report's prose and
// the data it was supposed to reflect (vote reasoning, prior-review
// matching). Unlike invariants 7-10, this is a MODEL's judgment recorded as
// data, not a deterministic fact — a violation here is information about
// this run's synthesis quality, same posture as every other invariant, never
// proof of anything until read.
//
// A VIOLATION AGAIN since 2026-09-10 (v63), after one release as a note. The
// demotion on 2026-09-09 was written as conditional and the condition is met,
// so this paragraph replaces the one that set it.
//
// Why it was demoted: measured over all 127 runs stored at that date, the
// vote-reasoning rule had raised five findings and every one was the SAME false
// positive (PRs 1518, 1532, 2049, 1538, 2083). The report cited `confidence`
// and `refuteFraction` figures, the critic's payload did not carry those
// fields, and the critic called correct numbers invented. Failing a run's
// self-audit on a correct report is worse than not checking. v55 rebuilt the
// payload so that class cannot recur.
//
// Why it is back: BE PR #2025 (2026-09-09), the rule's first firing at a
// post-v55 payload, and not a missing field. The finding states the DB identity
// query runs "only after host/port/confirm all already validated"; the report
// says it is printed "before the (already fail-closed) host/port/confirm checks
// fully gate execution". The report inverted the ordering its own finding
// described. That is what this rule exists to catch, so it fails the run again.
//
// A failing run now keeps the evidence: the synthesized report is written to
// `report.md` in the run's artifact dir as of the same release. Before that it
// lived only in one agent turn's context, and #2025 was adjudicable at all only
// because the critic happened to quote the disputed sentence in its own detail.
// A rule that fails a run and leaves nothing behind cannot be argued with.
check(() => {
  const violations = (m.fidelityCritic && m.fidelityCritic.violations) || [];
  if (violations.length)
    v(
      "fidelity-critic-flagged",
      `${violations.length} report-fidelity issue(s) flagged: ${violations
        .map((x) => `${x.rule}: ${x.findingTitle}`)
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 12 — the external-head probe was dispatched and did not answer. Invariant 1
// cannot catch this: it is gated on `codexEnabled`, and a probe whose markers
// never arrived leaves `codexEnabled` false, the same value a run with no CLI
// installed writes. So the run that LOST its decorrelation and the run that
// never had any read identically, and the failure disarms its own detector.
// `externalProbeAnswered` is null when no probe was dispatched, which is not a
// violation — only an explicit false is.
check(() => {
  if (m.externalProbeAnswered === false)
    v(
      "external-probe-unreadable",
      "the external-head probe returned no readable answer, so codex/gemini availability was UNKNOWN and defaulted to off — this run's panel had no external decorrelation, and the reason was not that the CLIs are absent",
    );
});

// 13 — a debate fired and settled nothing. The adjudicator came back with no
// usable decision, so the finding kept the panel's mechanical outcome and was
// never judged on argument. Before `"unadjudicated"` existed this did not
// degrade, it deleted: reading `ruling.decision` off a null return throws
// inside the per-finding thunk, a thrown thunk becomes a null slot, and
// `verified.filter(Boolean)` drops it — the finding left no trace in the report
// or on the row. The value now exists so the outcome is representable; this
// invariant is what stops it being quiet.
check(() => {
  const stuck = findings.filter((f) => f?.debate?.outcome === "unadjudicated");
  if (stuck.length)
    v(
      "debate-unadjudicated",
      `${stuck.length} finding(s) had a debate fire and return no ruling, so each kept its panel's mechanical verdict unjudged: ${stuck
        .map((f) => f.title || f.slotId || "(untitled)")
        .slice(0, 3)
        .join("; ")}`,
    );
});

// 14 — the report wrote a verdict line other than the harness verdict, and the
// workflow rewrote it. The line --post submits is now right, but the prose
// around it was written to argue the other verdict. BE #2171, 2026-10-17
// (wf_9357d96b-553): the row said APPROVE, the report said COMMENT and called
// the review "unfinished, not clean" after the panel had killed the sweep's
// only candidate.
check(() => {
  const c = m.reportVerdictCorrected;
  if (c && typeof c === "object")
    v(
      "report-verdict-corrected",
      `the report's verdict line said ${c.from || "nothing readable"} (${c.reason}) and was rewritten to the harness verdict ${c.to}; the prose around it may still argue the other verdict`,
    );
});

// 15 — a verified relay degraded. From v80 every copy a mechanic makes of a
// script's output (section-diff, blast-radius, fetch-pr, the two resolvers) is
// checked by checksum and retried once on sonnet; after a second bad copy, or
// when the script itself failed, the step runs without what it relayed and
// `verified` is false. The run is still coherent, which is exactly why it has
// to be said: the 2026-10-18 v79 runs lost the PR's sections silently.
check(() => {
  const bad = (Array.isArray(m.relays) ? m.relays : []).filter((r) => r && r.verified === false);
  if (bad.length)
    v(
      "relay-unverified",
      `${bad.length} relay(s) degraded: ${bad
        .map((r) => `${r.label} (${r.scriptError ? "the script failed" : "two copies failed verification"}: ${String(r.reason || "").slice(0, 120)})`)
        .join("; ")}`,
    );
});

// 16 — a dispatch embedded a Bash command over the length the Bash tool on
// Windows accepts (BASH_COMMAND_LIMIT, 6,000 characters; 7,264 ran and 7,418 did
// not). The workflow records it on the row as bashOverLimit instead of failing,
// because macOS and Linux run it. On a Windows run it is a violation: that
// command failed, and from v81 to v94 the persist step did exactly this on every
// run without a row to say so. Elsewhere it is a note, a defect waiting for the
// first Windows user.
check(() => {
  const over = Array.isArray(m.bashOverLimit) ? m.bashOverLimit : [];
  if (!over.length) return;
  const detail = `${over.length} dispatch(es) embedded a Bash command over 6,000 characters: ${over
    .slice(0, 5)
    .map((o) => `${o.label} (${o.chars})`)
    .join(", ")}`;
  if (m.platform === "win32") v("bash-over-windows-limit", detail);
  else n("bash-over-windows-limit", detail);
});

console.log(
  JSON.stringify({ passed: violations.length === 0, checked, violations, notes }),
);
