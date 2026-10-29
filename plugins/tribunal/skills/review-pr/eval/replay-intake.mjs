#!/usr/bin/env node
// replay-intake.mjs — who faces a panel, who gets dropped when the run hits its
// ceiling, and what a finding nobody contested is allowed to be called.
//
// The bug this locks down. On the pinned PR #1911 run, three `important`
// findings fell through every branch of needsPanel (not critical, not
// behaviour-changing, not high-risk), faced no adversarial slot at all, and
// reached the report marked blocking — blocking by DEFAULT, because a finding
// no panel ran on trivially "survives". SKILL.md already said nothing reaches
// Blocking without surviving Step 2.5; the code did not implement it.
//
// The opposite hypothesis was checked first and turned out to be wrong, which
// is why the fix widens intake instead of narrowing it: 5 of 22 panels went to
// `nit` findings, but all five were `refined: true` and `survived: false` —
// they entered ABOVE nit and the panel demoted them. There was no waste to
// reclaim. Assertion 3 pins that so the measurement is not re-litigated from
// the final severities alone.
//
// The functions under test are LIFTED FROM THE WORKFLOW SOURCE and evaluated
// here, not reimplemented. A hand-copied predicate is a second interpretation
// of the rule, and it agrees with the first right up until the rule changes.
//
// Usage: node eval/replay-intake.mjs

import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const METRICS = join(HERE, "..", "metrics.jsonl");
const src = readFileSync(WORKFLOW, "utf8");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- lift the real predicates ----------------------------------------------
function lift(startPattern, label) {
  const at = src.indexOf(startPattern);
  if (at === -1) throw new Error(`could not find ${label} in the workflow`);
  let i = src.indexOf("{", at);
  let depth = 0;
  for (let j = i; j < src.length; j++) {
    if (src[j] === "{") depth++;
    else if (src[j] === "}") {
      depth--;
      if (depth === 0) return src.slice(at, j + 1);
    }
  }
  throw new Error(`unbalanced braces reading ${label}`);
}

const needsPanelSrc = lift("function needsPanel(finding) {", "needsPanel");
// Stubbed: retest targets are the operator's --context, not a property of the
// finding, and their routing is covered by its own mirror invariant.
const needsPanel = new Function(
  "isRetestTarget",
  `${needsPanelSrc}; return needsPanel;`,
)((f) => !!f.__retest);

const priorityLine = src.match(/const panelPriority = \(f\) =>[\s\S]*?;\n/);
if (!priorityLine) throw new Error("could not find panelPriority");
const panelPriority = new Function(
  "isRetestTarget",
  `${priorityLine[0]} return panelPriority;`,
)((f) => !!f.__retest);

const F = (over = {}) => ({
  slotId: over.slotId ?? Math.abs(Math.round(Number(over.n ?? 1))),
  title: over.title || "t",
  severity: "important",
  isBehaviorChanging: false,
  touchesHighRiskSurface: false,
  lens: "generic",
  ...over,
});

// --- 1. intake -------------------------------------------------------------
check("nit never earns a panel", needsPanel(F({ severity: "nit" })) === false);
check("critical earns a panel", needsPanel(F({ severity: "critical" })) === true);
check(
  "plain important now earns a panel (the PR #1911 bypass)",
  needsPanel(
    F({ severity: "important", isBehaviorChanging: false, touchesHighRiskSurface: false }),
  ) === true,
  "three findings reached the report as blocking through this hole",
);
check(
  "a nit is still a nit even when high-risk",
  needsPanel(F({ severity: "nit", touchesHighRiskSurface: true })) === false,
);
check(
  "a bot comment earns a panel regardless of severity",
  needsPanel(F({ severity: "nit", lens: "bot-comments" })) === true,
);
check(
  "an operator-named claim earns a panel regardless of severity",
  needsPanel(F({ severity: "nit", __retest: true })) === true,
);

// --- 2. the ceiling drops the least-contested work -------------------------
{
  const pool = [
    F({ n: 1, title: "plain important" }),
    F({ n: 2, title: "critical", severity: "critical" }),
    F({ n: 3, title: "operator-named", severity: "nit", __retest: true }),
    F({ n: 4, title: "high-risk important", touchesHighRiskSurface: true }),
  ];
  const rank = (maxPanels) =>
    pool
      .map((f, i) => ({ f, i }))
      .sort((a, b) => panelPriority(b.f) - panelPriority(a.f) || a.i - b.i)
      .slice(0, maxPanels)
      .sort((a, b) => a.i - b.i)
      .map((x) => x.f.title);

  check(
    "operator-named claim outranks everything",
    rank(1)[0] === "operator-named",
    JSON.stringify(rank(1)),
  );
  check(
    "critical and high-risk are kept over a plain important",
    !rank(3).includes("plain important"),
    JSON.stringify(rank(3)),
  );
  check(
    "under the ceiling nothing is dropped, and input order is restored",
    rank(99).join("|") === pool.map((f) => f.title).join("|"),
    JSON.stringify(rank(99)),
  );
}

// --- 3. the measurement that redirected this fix ---------------------------
// Guards the CONCLUSION, not just the code: if a future run shows paneled nits
// that were NOT demoted by their own panel, the "no waste to reclaim" reading
// stops holding and intake deserves a second look.
if (existsSync(METRICS)) {
  const rows = readFileSync(METRICS, "utf8")
    .trim()
    .split("\n")
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
  // Only rows that pass their own self-audit may support a conclusion. The
  // 2026-07-23 run is why: it recorded 12 paneled nits with half of them not
  // demoted, which reads as intake waste — but that run had NO external voter on
  // any of its 26 panels, so its panel outcomes are not evidence about intake or
  // about anything else. Recalibrating on a degraded run is a mistake this
  // project has already made once; the audit is now what stops it.
  const usable = rows.filter((r) => {
    if (r.pr !== 1911 || !r.pinned) return false;
    if (r.selfAudit && r.selfAudit.passed === false) return false;
    const panels = (r.findings || []).filter((f) => f.panelRan).length;
    const ext = (r.findings || []).filter((f) => f.hadExternalVoter).length;
    return !((r.codexEnabled || r.geminiEnabled) && panels > 0 && ext === 0);
  });
  const run = usable.pop();
  if (run) {
    const paneledNits = run.findings.filter((f) => f.panelRan && f.severity === "nit");
    check(
      "PR #1911: every paneled nit was demoted BY its panel, not fed to one",
      paneledNits.length > 0 && paneledNits.every((f) => f.refined && !f.survived),
      `${paneledNits.length} paneled nits, ${paneledNits.filter((f) => f.refined && !f.survived).length} demoted`,
    );
  } else {
    console.log(
      "  skip  no PR #1911 pinned row that passes its own self-audit — the intake conclusion is unverified, not confirmed",
    );
  }
}

// --- 4. an unadjudicated finding cannot be blocking ------------------------
// The invariant does not depend on needsPanel being right — that is its point.
{
  const blocking = (v) =>
    v.survived &&
    v.finding.severity !== "nit" &&
    ((v.panelRan && !v.unverified) ||
      (v.grounded && v.grounded.verdict === "confirmed"));

  check(
    "survived + important + no panel + no typecheck → NOT blocking",
    blocking({
      survived: true,
      panelRan: false,
      grounded: { verdict: null },
      finding: { severity: "important" },
    }) === false,
  );
  // The live 2026-07-23 run: the session limit killed every head mid-panel and
  // 16 findings landed blocking with zero votes. A panel that was DISPATCHED is
  // not a panel that ADJUDICATED, and the first version of this rule could not
  // tell the two apart.
  check(
    "a panel whose heads all died adjudicates nothing",
    blocking({
      survived: true,
      panelRan: true,
      unverified: true,
      grounded: { verdict: null },
      finding: { severity: "critical" },
    }) === false,
    "16 findings on the live run were blocking on the strength of an empty panel",
  );
  check(
    "survived a real panel → blocking",
    blocking({
      survived: true,
      panelRan: true,
      grounded: { verdict: null },
      finding: { severity: "important" },
    }) === true,
  );
  check(
    "typecheck-confirmed skips the panel and is still blocking",
    blocking({
      survived: true,
      panelRan: false,
      grounded: { verdict: "confirmed" },
      finding: { severity: "critical" },
    }) === true,
    "grounded findings skip the panel by design — the invariant must not eat them",
  );
  check(
    "the workflow computes blocking this way",
    src.includes("(v.panelRan && !v.unverified) ||"),
    "the shipped rule drifted from the one asserted here",
  );
  check(
    "the report is told what adjudicated:false forbids",
    src.includes('MUST NOT appear under "Blocking"'),
  );
}

console.log(`\nintake: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`);
process.exit(failures.length ? 1 : 0);
