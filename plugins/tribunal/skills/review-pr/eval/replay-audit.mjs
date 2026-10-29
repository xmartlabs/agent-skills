#!/usr/bin/env node
// replay-audit.mjs — the self-audit's invariants, run against the real metrics
// row of the run that motivated them.
//
// This is the test that the rest of the eval battery could not be. Eight
// replays and twenty-one mirror invariants were green through the 2026-07-23
// run while that run had no external voter on any of its 26 panels, 67 orphan
// artifacts, and a reconciliation covering 18 of 39 slots. Every one of those
// checks verifies the code against itself; none of them looks at what a run
// produced.
//
// So the bar here is specific: feed audit-run.mjs the ACTUAL degraded row and
// require that invariants 1, 2, 3 and 5 fail on it. An invariant that does not
// catch the defect it was written for is decoration, and a checker made of
// decoration is worse than no checker — it gets read as a clean bill.
//
// Usage: node eval/replay-audit.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, utimesSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { FIXTURES_DIR, lib, SKILL_DIR } from "./paths.mjs";

const AUDIT = lib("audit-run.mjs");
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const run = (metrics, dir = "") =>
  JSON.parse(
    execFileSync("node", [AUDIT, dir], {
      input: JSON.stringify(metrics),
      encoding: "utf8",
    }),
  );
const ids = (r) => r.violations.map((x) => x.id).sort();

// --- the 2026-07-23 row, reduced to the fields the invariants read ----------
// Values are the real ones: 26 panels, no external voter, 67 orphans, 18 of 39
// slots reconciled.
const DEGRADED = {
  codexEnabled: true,
  geminiEnabled: true,
  externalSlots: 39,
  voteReconciliation: { checked: 18, counts: { "over-invoked": 5, "orphan-artifact": 67 } },
  findings: Array.from({ length: 26 }, (_, i) => ({
    title: `finding ${i}`,
    panelRan: true,
    hadExternalVoter: false,
    blocking: i < 12,
    adjudicated: true,
  })),
};

// Prefer the row the run actually wrote. DEGRADED above is a faithful reduction
// of it, kept as a fallback so this file still tests something if the fixture is
// ever lost — but a fixture captured from a real failure is worth more than any
// hand-built one, because nobody had to guess what a broken run looks like.
const FIXTURE = resolve(FIXTURES_DIR, "metrics-2026-07-23-degraded.json");
let subject = DEGRADED;
let subjectLabel = "reconstructed row";
if (existsSync(FIXTURE)) {
  const real = JSON.parse(
    execFileSync("cat", [FIXTURE], { encoding: "utf8" }),
  );
  // externalSlots was not in schemaVersion 2; the run logged 39 invocations.
  subject = { ...real, externalSlots: 39 };
  subjectLabel = `real row (PR #${real.pr} @ ${String(real.headSha).slice(0, 8)})`;
}
console.log(`  using: ${subjectLabel}`);

{
  const r = run(subject);
  const got = ids(r);
  check("the degraded row does NOT pass", r.passed === false, JSON.stringify(got));
  check(
    "invariant 1 fires: no external voter on any panel",
    got.includes("no-external-voter"),
    JSON.stringify(got),
  );
  check(
    "invariant 2 fires: orphan artifacts",
    got.includes("orphan-artifacts"),
    JSON.stringify(got),
  );
  check(
    "invariant 3 fires: reconciliation covered 18 of 39",
    got.includes("reconciliation-incomplete"),
    JSON.stringify(got),
  );
}

// --- a healthy row must be silent ------------------------------------------
// The other half of the bar. A checker that fires on everything is the same
// failure as one that fires on nothing: it stops being read.
{
  const healthy = {
    codexEnabled: true,
    geminiEnabled: true,
    externalSlots: 20,
    voteReconciliation: { checked: 20, counts: {} },
    agentsProjected: 100,
    agentsActual: 108,
    findings: Array.from({ length: 20 }, (_, i) => ({
      title: `finding ${i}`,
      panelRan: true,
      hadExternalVoter: true,
      blocking: i < 5,
      adjudicated: true,
    })),
  };
  const r = run(healthy);
  check("a healthy row passes clean", r.passed === true, JSON.stringify(ids(r)));
  check("it actually ran its invariants", r.checked >= 6, `checked=${r.checked}`);
}

// --- invariant 5: artifacts that outlived their run -------------------------
// The defect that sent a whole piece of work chasing a protocol breach that
// never happened. A stale .meta is planted in a fresh directory, exactly as a
// per-PR (rather than per-run) directory produced naturally.
{
  const dir = mkdtempSync(join(tmpdir(), "audit-stale-"));
  const fresh = join(dir, "1-codex.meta");
  writeFileSync(fresh, "RUNS: 1\n");
  const base = { codexEnabled: false, geminiEnabled: false, findings: [] };

  const clean = run(base, dir);
  check(
    "a directory holding only this run's artifacts is clean",
    !ids(clean).includes("stale-artifacts"),
    JSON.stringify(ids(clean)),
  );

  // Backdate it two hours — older than the directory itself, which is what
  // audit-run uses as the run start.
  const old = Date.now() / 1000 - 7200;
  utimesSync(fresh, old, old);
  const stale = run(base, dir);
  check(
    "invariant 5 fires: an artifact predating the run",
    ids(stale).includes("stale-artifacts"),
    JSON.stringify(ids(stale)),
  );
  check(
    "and it names the directory so it can be inspected",
    (stale.violations.find((x) => x.id === "stale-artifacts")?.detail || "").includes(dir),
  );
}

// --- invariant 4: the blocking/adjudicated regression guard ----------------
{
  const r = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [
      { title: "unexamined but blocking", panelRan: false, blocking: true, adjudicated: false },
    ],
  });
  check(
    "invariant 4 fires: blocking without adjudication",
    ids(r).includes("blocking-without-adjudication"),
    JSON.stringify(ids(r)),
  );
}

// --- invariant 6: cost-model drift -----------------------------------------
{
  const drifted = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [],
    agentsProjected: 100,
    agentsActual: 150,
  });
  check(
    "invariant 6 fires on a 33% cost miss",
    ids(drifted).includes("cost-model-drift"),
    JSON.stringify(ids(drifted)),
  );
  const absent = run({ codexEnabled: false, geminiEnabled: false, findings: [] });
  check(
    "invariant 6 stays silent when the real count is unknown",
    !ids(absent).includes("cost-model-drift"),
    "a missing agentsActual must not read as drift",
  );
}

// --- an unreadable payload is never a pass ---------------------------------
{
  const r = JSON.parse(
    execFileSync("node", [AUDIT, ""], { input: "not json at all", encoding: "utf8" }),
  );
  check("garbage in is not a pass", r.passed === false, JSON.stringify(r));
}

// --- invariant 7: a merge that lost its extra locations from the report ----
// mergeCandidates() carries every member's own file:line specifically so a
// wrong merge degrades to a noisy finding instead of a silent deletion (see
// its own comment in workflows/tribunal.js) — but that guarantee lived only
// in a prose instruction inside the synthesis prompt, sitting among ~30 other
// grounding rules in the longest single-context prompt in the harness.
// mergedLocationsMissingFromReport is computed elsewhere (inline in the
// Workflow, or lib/check-report-fidelity.mjs in-context — see
// replay-report-fidelity-check.mjs for that half); this only checks that
// audit-run.mjs actually reads it once populated.
{
  const bad = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [
      {
        title: "dup exit-0 bug",
        mergedLocations: [{ file: "a.sh", line: 5 }, { file: "b.sh", line: 12 }],
        mergedLocationsMissingFromReport: ["b.sh:12"],
      },
    ],
  });
  check(
    "invariant 7 fires: a merged location dropped from the report",
    ids(bad).includes("merged-locations-dropped"),
    JSON.stringify(ids(bad)),
  );

  const clean = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [
      {
        title: "dup exit-0 bug, printed correctly",
        mergedLocations: [{ file: "a.sh", line: 5 }, { file: "b.sh", line: 12 }],
        mergedLocationsMissingFromReport: [],
      },
      { title: "never merged", mergedLocations: [] },
      { title: "legacy row, field absent entirely" },
    ],
  });
  check(
    "invariant 7 stays silent when every extra location made it in, or there was nothing to merge",
    !ids(clean).includes("merged-locations-dropped"),
    JSON.stringify(ids(clean)),
  );
}

// --- invariants 8-10b: the four other report-fidelity checks ---------------
// Same rule as invariant 7 above, over the sibling fields
// (killReasonMissingFromReport, mergedFromAnnotationMissingFromReport,
// groundedDiagnosticMissingFromReport) added in the same batch, and
// unexaminedWordingInReport added later. This only
// checks that audit-run.mjs reads them once populated — see
// replay-report-fidelity-check.mjs for whether the fields themselves are
// computed correctly.
{
  const bad = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [
      { title: "killReason dropped", killReasonMissingFromReport: ["out-of-scope"] },
      { title: "mergedFrom dropped", mergedFromAnnotationMissingFromReport: ["(found by:", "lensA"] },
      { title: "grounded dropped", groundedDiagnosticMissingFromReport: ["TS2345: x"] },
      { title: "demoted called unexamined", unexaminedWordingInReport: ["NOT ADJUDICATED"] },
    ],
  });
  check(
    "invariant 8 fires: killReason category never made it into the report",
    ids(bad).includes("kill-reason-not-cited"),
    JSON.stringify(ids(bad)),
  );
  check(
    "invariant 9 fires: mergedFrom annotation dropped from the report",
    ids(bad).includes("merged-from-annotation-dropped"),
    JSON.stringify(ids(bad)),
  );
  check(
    "invariant 10 fires: typecheck-confirmed diagnostic never cited",
    ids(bad).includes("grounded-diagnostic-not-cited"),
    JSON.stringify(ids(bad)),
  );
  check(
    "invariant 10b fires: a gate-demoted finding whose panel voted was called unexamined",
    ids(bad).includes("gate-demoted-called-unexamined") &&
      bad.violations.some((x) => x.detail.includes("demoted called unexamined (NOT ADJUDICATED)")),
    JSON.stringify(bad.violations),
  );

  const clean = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [
      {
        title: "all compliant",
        killReasonMissingFromReport: [],
        mergedFromAnnotationMissingFromReport: [],
        groundedDiagnosticMissingFromReport: [],
        unexaminedWordingInReport: [],
      },
      { title: "legacy row, fields absent entirely" },
    ],
  });
  check(
    "invariants 8-10b stay silent on compliant/legacy rows",
    ![
      "kill-reason-not-cited",
      "merged-from-annotation-dropped",
      "grounded-diagnostic-not-cited",
      "gate-demoted-called-unexamined",
    ].some(
      (id) => ids(clean).includes(id),
    ),
    JSON.stringify(ids(clean)),
  );
}

// --- invariant 11: the fidelity critic is RECORDED, and does not fail -----
// Back in `violations` on 2026-09-10 (v63), after one release in `notes`. The
// demotion a day earlier was written as conditional: five firings, all the same
// false positive, all caused by the critic's payload lacking the field the
// report cited, and v55 rebuilt the payload so that class cannot recur. BE PR
// #2025 is the first firing since, and it is real drift, not a missing field:
// the report inverted the ordering its own finding described.
// These assertions moved because the BEHAVIOUR moved, deliberately; if they are
// ever red without an intended change, the code is what moved.
// Unlike 7-10, `fidelityCritic` is a row-level field (one per run), not a
// per-finding one, and its violations are a MODEL's judgment recorded as
// data, not a deterministic fact. This only checks that audit-run.mjs reads
// it once populated — whether the critic itself catches a real mismatch is
// not answerable offline (see replay-report-fidelity-check.mjs's header and
// workflows/tribunal.js's fidelity-critic comment for that boundary).
{
  const bad = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [],
    fidelityCritic: {
      ran: true,
      violations: [
        { rule: "prior-review-match", findingTitle: "f1", detail: "false match" },
      ],
    },
  });
  check(
    "invariant 11 records the flag as a VIOLATION",
    ids(bad).includes("fidelity-critic-flagged"),
    JSON.stringify(ids(bad)),
  );
  check(
    "and NOT as a note",
    !(bad.notes || []).some((n) => n.id === "fidelity-critic-flagged"),
    JSON.stringify(bad.notes || []),
  );
  check(
    "so a run whose only flag is this one FAILS its audit",
    bad.passed === false,
    JSON.stringify({ passed: bad.passed, violations: ids(bad) }),
  );
  check(
    "the violation carries the critic's own wording, not just an id",
    (bad.violations || []).some((x) =>
      /prior-review-match/.test(x.detail || ""),
    ),
    JSON.stringify(bad.violations || []),
  );

  const cleanRan = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [],
    fidelityCritic: { ran: true, violations: [] },
  });
  const notRan = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: [],
    fidelityCritic: { ran: false, violations: [] },
  });
  const absent = run({ codexEnabled: false, geminiEnabled: false, findings: [] });
  const noteIds = (r) => (r.notes || []).map((n) => n.id);
  check(
    "invariant 11 stays silent when the critic ran clean, didn't run, or the field is absent (legacy row)",
    [cleanRan, notRan, absent].every(
      (r) =>
        !ids(r).includes("fidelity-critic-flagged") &&
        !noteIds(r).includes("fidelity-critic-flagged"),
    ),
    JSON.stringify({ cleanRan: noteIds(cleanRan), notRan: noteIds(notRan), absent: noteIds(absent) }),
  );
  check(
    "every audit result carries a notes array, so a reader cannot mistake absent for empty",
    [bad, cleanRan, notRan, absent].every((r) => Array.isArray(r.notes)),
  );
}

// --- invariant 12: the probe that answered nothing (2026-09-04) ------------
// The gap it closes is the reason it needs its own invariant rather than a
// widening of invariant 1: invariant 1 is gated on `codexEnabled`, and a probe
// whose markers never arrived leaves `codexEnabled` false — the same value a
// machine without codex writes. Both rows below have panels, no external
// voter, and codexEnabled false. Invariant 1 is silent on both, correctly.
// Only invariant 12 can tell them apart.
{
  console.log(`\n--- invariant 12: unreadable external-head probe ---`);
  const withPanels = [
    { panelRan: true, hadExternalVoter: false, blocking: false },
    { panelRan: true, hadExternalVoter: false, blocking: false },
  ];

  const lostProbe = run({
    codexEnabled: false,
    geminiEnabled: false,
    externalProbeAnswered: false,
    findings: withPanels,
  });
  const noCliInstalled = run({
    codexEnabled: false,
    geminiEnabled: false,
    externalProbeAnswered: true,
    findings: withPanels,
  });
  const neverProbed = run({
    codexEnabled: false,
    geminiEnabled: false,
    externalProbeAnswered: null,
    findings: withPanels,
  });
  const legacyRow = run({
    codexEnabled: false,
    geminiEnabled: false,
    findings: withPanels,
  });

  check(
    "invariant 12 fires: the probe returned no readable answer",
    ids(lostProbe).includes("external-probe-unreadable"),
    JSON.stringify(ids(lostProbe)),
  );
  check(
    "invariant 12 stays silent when the probe answered and the CLIs are simply absent",
    !ids(noCliInstalled).includes("external-probe-unreadable"),
    JSON.stringify(ids(noCliInstalled)),
  );
  check(
    "invariant 12 stays silent when no probe was dispatched (null)",
    !ids(neverProbed).includes("external-probe-unreadable"),
    JSON.stringify(ids(neverProbed)),
  );
  check(
    "invariant 12 stays silent on a legacy row with no such key",
    !ids(legacyRow).includes("external-probe-unreadable"),
    JSON.stringify(ids(legacyRow)),
  );
  check(
    "the gap is real: invariant 1 cannot separate these two rows",
    !ids(lostProbe).includes("no-external-voter") &&
      !ids(noCliInstalled).includes("no-external-voter"),
    JSON.stringify({ lost: ids(lostProbe), absent: ids(noCliInstalled) }),
  );
}

// Invariant 13. Before "unadjudicated" existed there was nothing to assert on:
// an adjudicator that came back dead threw inside its thunk, the thunk became a
// null slot, and the finding was dropped before any row was written. So the
// row's own findings array is the wrong place to look for the old bug — the
// point of the value is that the degraded finding now REACHES the row at all,
// and this invariant is what stops it arriving quietly.
{
  console.log(`\n--- invariant 13: a debate fired and settled nothing ---`);
  const stuck = run({
    findings: [
      { title: "negative amount accepted", debate: { fired: true, outcome: "unadjudicated" } },
      { title: "unpaginated query", debate: { fired: true, outcome: "held" } },
    ],
  });
  const judged = run({
    findings: [
      { title: "negative amount accepted", debate: { fired: true, outcome: "flipped" } },
      { title: "unpaginated query", debate: { fired: true, outcome: "held" } },
    ],
  });
  const noDebate = run({
    findings: [{ title: "x", debate: { fired: false, outcome: null } }],
  });
  const legacy = run({ findings: [{ title: "x" }] });

  check(
    "invariant 13 fires on an unadjudicated debate",
    ids(stuck).includes("debate-unadjudicated"),
    JSON.stringify(ids(stuck)),
  );
  check(
    "…and names the finding, so the row says WHICH one went unjudged",
    (stuck.violations || [])
      .filter((x) => x.id === "debate-unadjudicated")
      .some((x) => /negative amount accepted/.test(x.detail)),
    JSON.stringify(stuck.violations),
  );
  check(
    "it stays silent when every debate produced a ruling",
    !ids(judged).includes("debate-unadjudicated"),
    JSON.stringify(ids(judged)),
  );
  check(
    "it stays silent when no debate fired",
    !ids(noDebate).includes("debate-unadjudicated"),
  );
  check(
    "it stays silent on a legacy row with no debate object",
    !ids(legacy).includes("debate-unadjudicated"),
  );
}

// Invariant 1 names the cause when the heads recorded one (2026-10-18: Codex out of credits).
{
  console.log(`\n--- invariant 1: the missing external vote says why ---`);
  const f = (reasons) => ({ panelRan: true, hadExternalVoter: false, failedSlotsDetail: reasons.map((reason) => ({ source: "codex", reason })) });
  const dry = run({ codexEnabled: true, findings: [f(["quota"]), f(["quota"])] });
  const silent = run({ codexEnabled: true, findings: [{ panelRan: true, hadExternalVoter: false }] });
  check("it carries the head's failure class and count", dry.violations.some((x) => x.id === "no-external-voter" && x.detail.endsWith("(codex: quota x2)")), JSON.stringify(dry.violations));
  check("and stays as it was when no cause was recorded", silent.violations.some((x) => x.id === "no-external-voter" && x.detail.endsWith("without decorrelation")));
}

// Invariant 15 (v81): a verified relay that degraded must not read as a clean run.
{
  console.log(`\n--- invariant 15: a verified relay degraded ---`);
  const relay = (label, over = {}) => ({ label, model: "haiku", firstModel: "haiku", retries: 0, verified: true, scriptError: false, bytes: 100, reason: null, ...over });
  const lost = run({ findings: [], relays: [relay("section-diff"), relay("blast-radius", { verified: false, retries: 1, model: "sonnet", bytes: null, reason: "checksum mismatch: the script printed a, the copy is b" })] });
  const failed = run({ findings: [], relays: [relay("fetch-pr", { verified: false, scriptError: true, bytes: null, reason: "FATAL: gh auth" })] });
  const clean = run({ findings: [], relays: [relay("section-diff"), relay("fetch-pr", { retries: 1, model: "sonnet" })] });
  const legacy = run({ findings: [] });
  check("it fires on a relay whose two copies failed, and names it with the reason", ids(lost).includes("relay-unverified") && lost.violations.some((x) => /blast-radius \(two copies failed verification: checksum mismatch/.test(x.detail)), JSON.stringify(lost.violations));
  check("it fires on a script that failed, and says so", failed.violations.some((x) => x.id === "relay-unverified" && /fetch-pr \(the script failed: FATAL: gh auth\)/.test(x.detail)));
  check("a retry that verified is not a violation", !ids(clean).includes("relay-unverified"), JSON.stringify(ids(clean)));
  check("a row from before relays existed is not one either", !ids(legacy).includes("relay-unverified"));
}

console.log("\n--- 16: a command over the Windows limit ---");
{
  const base = { findings: [], bashOverLimit: [{ label: "evidence-resolver", chars: 7420 }] };
  const win = run({ ...base, platform: "win32" });
  check("on Windows it is a violation that names the dispatch", ids(win).includes("bash-over-windows-limit") && /evidence-resolver \(7420\)/.test(win.violations.find((x) => x.id === "bash-over-windows-limit").detail), JSON.stringify(win.violations));
  const mac = run({ ...base, platform: "darwin" });
  check("elsewhere it is a note, and the run still passes", !ids(mac).includes("bash-over-windows-limit") && mac.notes.some((x) => x.id === "bash-over-windows-limit"), JSON.stringify(mac));
  check("with nothing over the limit neither fires", !ids(run({ findings: [], platform: "win32" })).includes("bash-over-windows-limit") && run({ findings: [], bashOverLimit: [], platform: "win32" }).notes.length === 0);
}

console.log(`\nself-audit: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`);
process.exit(failures.length ? 1 : 0);
