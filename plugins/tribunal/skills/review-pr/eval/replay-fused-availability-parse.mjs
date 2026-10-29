#!/usr/bin/env node
// replay-fused-availability-parse.mjs — the parser for Fusion A/B's combined
// dispatch (check-external-heads + setup-worktree + grounded-typecheck, one
// agent instead of up to three, 2026-07-28), tested against the shapes that
// dispatch's own bash actually produces.
//
// Why this needs its own replay. The fusion means one text blob now carries
// three things that used to arrive as three separate agent returns: CLI
// availability, the head worktree's path (or WORKTREE_FAILED), and — only
// when grounded-typecheck ran inline — its own sentinels appended after that.
// A parsing bug here is silent by construction: `externalCwd` staying null
// when a worktree really was created just means external heads run in the
// base checkout, degraded, with a log line easy to miss in a long run.
//
// Like eval/replay-gates.mjs and eval/replay-diff-sectioning.mjs, this
// evaluates the REAL code sliced out of workflows/peer-review.js by its
// markers — never a copy.
//
// Usage: node eval/replay-fused-availability-parse.mjs

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

const { api, violations } = loadRegion(WORKFLOW, "FUSED-AVAILABILITY-PARSE", [
  "parseFusedAvailabilityText",
  "parseCreatedWorktreeMarker",
]);
const { parseFusedAvailabilityText, parseCreatedWorktreeMarker } = api;

console.log(`\nFUSED-AVAILABILITY-PARSE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const probeLines = (codexYes, geminiYes, voteDir) =>
  `codex:${codexYes ? "yes" : "no"}\ngemini:${geminiYes ? "yes" : "no"}\nvotedir:${voteDir}`;

console.log(`\n--- availability + votedir, independent of worktree ---`);
{
  const text = probeLines(true, false, "/tmp/peer-review-votes-1970-abc");
  const r = parseFusedAvailabilityText(text, false, true, false);
  check("codex:yes -> codexAvailable true", r.codexAvailable === true);
  check("gemini:no -> geminiAvailable false", r.geminiAvailable === false);
  check(
    "votedir parsed",
    r.resolvedVoteDir === "/tmp/peer-review-votes-1970-abc",
  );
  check("no worktree wanted -> externalCwd stays null", r.externalCwd === null);
}

console.log(`\n--- probeAnswered separates "not installed" from "never answered" (2026-09-04) ---`);
{
  // The defect this closes: an unreadable return produced codexAvailable
  // false, byte-identical to a machine with no codex, and invariant 1 is gated
  // on that same false — so the run that LOST its decorrelation passed the
  // self-audit exactly like one that never had any.
  const installed = parseFusedAvailabilityText(
    probeLines(true, true, "/tmp/votes-1"),
    false,
    false,
    false,
  );
  const notInstalled = parseFusedAvailabilityText(
    probeLines(false, false, "/tmp/votes-1"),
    false,
    false,
    false,
  );
  const empty = parseFusedAvailabilityText("", false, false, false);
  const prose = parseFusedAvailabilityText(
    "I ran the commands you asked for and everything looks fine.",
    false,
    false,
    false,
  );
  const partial = parseFusedAvailabilityText(
    "codex:yes\nvotedir:/tmp/votes-1",
    false,
    false,
    false,
  );

  check(
    "markers present -> probeAnswered true",
    installed.probeAnswered === true,
  );
  check(
    "an honest codex:no/gemini:no is still an ANSWER",
    notInstalled.probeAnswered === true && notInstalled.codexAvailable === false,
  );
  check("empty return -> probeAnswered false", empty.probeAnswered === false);
  check("prose reply -> probeAnswered false", prose.probeAnswered === false);
  check("half the markers is not an answer", partial.probeAnswered === false);
  check(
    "the two states that used to be identical now differ",
    notInstalled.codexAvailable === empty.codexAvailable &&
      notInstalled.probeAnswered !== empty.probeAnswered,
  );
  check(
    "availability still fails CLOSED on an unreadable return",
    empty.codexAvailable === false && empty.geminiAvailable === false,
  );
}

console.log(`\n--- noCodex/noGemini flags override a "yes" probe result ---`);
{
  const text = probeLines(true, true, "/tmp/votes-1");
  const r = parseFusedAvailabilityText(text, true, true, false);
  check(
    "noCodex=true zeroes codexAvailable despite codex:yes",
    r.codexAvailable === false,
  );
  check(
    "noGemini=true zeroes geminiAvailable despite gemini:yes",
    r.geminiAvailable === false,
  );
}

console.log(`\n--- the 4 codex/gemini-availability x worktree-success combinations ---`);
{
  const cases = [
    { codex: true, gemini: false, wt: "/tmp/peer-review-wt-1970-xyz", label: "codex-only, worktree ok" },
    { codex: false, gemini: true, wt: "/tmp/peer-review-wt-1970-xyz", label: "gemini-only, worktree ok" },
    { codex: true, gemini: true, wt: "WORKTREE_FAILED", label: "both available, worktree failed" },
    { codex: false, gemini: false, wt: null, label: "neither available, no worktree line at all" },
  ];
  for (const c of cases) {
    const text =
      probeLines(c.codex, c.gemini, "/tmp/votes-x") +
      (c.wt ? `\n${c.wt}` : "");
    const r = parseFusedAvailabilityText(text, false, false, true);
    if (c.wt === null) {
      check(`${c.label}: externalCwd is null (no line emitted)`, r.externalCwd === null);
    } else if (c.wt === "WORKTREE_FAILED") {
      check(`${c.label}: externalCwd is null (WORKTREE_FAILED)`, r.externalCwd === null);
    } else {
      check(`${c.label}: externalCwd resolves to the path`, r.externalCwd === c.wt);
    }
  }
}

console.log(`\n--- worktree line survives typecheck output appended after it ---`);
{
  // This is the fused case: typecheck's own sentinels follow the worktree
  // line in the SAME text. None of them start with "/" or equal
  // WORKTREE_FAILED, so the tail-scan must still find the real worktree path.
  const text = [
    probeLines(true, false, "/tmp/votes-y"),
    "/tmp/peer-review-wt-1970-real",
    "NO_DIAGNOSTICS",
    "---COVERED---",
    "src/routes/v2/auth.routes.ts",
    "src/controllers/v2/auth.controller.ts",
  ].join("\n");
  const r = parseFusedAvailabilityText(text, false, true, true);
  check(
    "externalCwd is the worktree path, not a covered-file line",
    r.externalCwd === "/tmp/peer-review-wt-1970-real",
  );
}
{
  const text = [
    probeLines(true, false, "/tmp/votes-z"),
    "WORKTREE_FAILED",
    "DEGRADED_NO_WORKTREE",
  ].join("\n");
  const r = parseFusedAvailabilityText(text, false, true, true);
  check(
    "WORKTREE_FAILED still parses to null even with a DEGRADED_ line after it",
    r.externalCwd === null,
  );
}

console.log(`\n--- CLONED: fallback (ambient cwd was not a clone of the reviewed repo, 2026-08-19) ---`);
{
  // PR #1518 shape: a review launched from a session anchored in a sibling
  // repo. `git worktree add`/`git fetch` against the ambient repo fail, the
  // bash falls back to `gh repo clone`, and marks the result with a
  // "CLONED:" prefix so teardown knows not to `git worktree remove` it.
  const text = [
    probeLines(true, false, "/tmp/votes-cloned"),
    "CLONED:/tmp/peer-review-wt-1518-fresh",
  ].join("\n");
  const r = parseFusedAvailabilityText(text, false, true, true);
  check(
    "CLONED: prefix strips to the bare path",
    r.externalCwd === "/tmp/peer-review-wt-1518-fresh",
  );
  check("CLONED: sets externalCwdIsPlainClone", r.externalCwdIsPlainClone === true);
}
{
  const text = [
    probeLines(true, false, "/tmp/votes-ambient"),
    "/tmp/peer-review-wt-1518-ambient",
  ].join("\n");
  const r = parseFusedAvailabilityText(text, false, true, true);
  check(
    "a bare worktree path leaves externalCwdIsPlainClone false",
    r.externalCwdIsPlainClone === false,
  );
}
{
  const text = [
    probeLines(true, false, "/tmp/votes-cloned2"),
    "CLONED:/tmp/peer-review-wt-1518-fresh2",
    "NO_DIAGNOSTICS",
    "---COVERED---",
    "src/foo.ts",
  ].join("\n");
  const r = parseFusedAvailabilityText(text, false, true, true);
  check(
    "CLONED: line survives typecheck output appended after it, same as a bare path does",
    r.externalCwd === "/tmp/peer-review-wt-1518-fresh2" && r.externalCwdIsPlainClone === true,
  );
}

console.log(`\n--- CREATED_WORKTREE marker (grounded-typecheck self-created its own) ---`);
{
  check(
    "marker present -> path extracted",
    parseCreatedWorktreeMarker("NO_TSCONFIG_UNRELATED\nCREATED_WORKTREE /tmp/peer-review-wt-1970-own\nNO_DIAGNOSTICS") ===
      "/tmp/peer-review-wt-1970-own",
  );
  check(
    "marker absent -> null (head worktree was reused, not self-created)",
    parseCreatedWorktreeMarker("NO_DIAGNOSTICS\n---COVERED---\nsrc/foo.ts") === null,
  );
  check("empty text -> null", parseCreatedWorktreeMarker("") === null);
  check("null text -> null", parseCreatedWorktreeMarker(null) === null);
}

console.log(`\n--- Fusion C: teardown removal is best-effort, never gates the read ---`);
{
  const fs = await import("node:fs");
  const src = fs.readFileSync(WORKFLOW, "utf8");
  const m = src.match(
    /git worktree remove --force "\$\{p\}" 2>\/dev\/null \|\| true/,
  );
  check(
    "worktree removal line in the fused teardown+verify dispatch ends in '|| true'",
    !!m,
    m ? "" : "removal line not found or missing the best-effort guard",
  );
  // Anchored on the phrase the classifier actually keys on
  // (`agent-cost.mjs`: /^Return the last line of/), not on the path that
  // follows it. Pinning the whole path made this fail when the metrics file
  // moved out of the skill directory — a relocation the classifier neither
  // sees nor cares about, so the test was guarding something it did not claim.
  const head = src.match(/Return the last line of \S+metrics\.jsonl as structured data/);
  check(
    "teardown-and-verify-metrics-row's literal head still starts with the classifier-recognised phrase",
    !!head,
  );
  // A CLONED: fallback path was never `git worktree add`ed against anything —
  // `git worktree remove` finds nothing to remove and no-ops via its own
  // `|| true`, leaking the clone. This branch must exist alongside the one
  // above, not replace it: a real worktree still needs `git worktree remove`
  // so the ambient repo's own worktree registry (orphaned-worktrees.mjs's
  // data source) gets cleaned up, not just the directory.
  const rmrf = src.match(/rm -rf "\$\{p\}" 2>\/dev\/null \|\| true/);
  check(
    "plain-clone teardown falls back to rm -rf, alongside git worktree remove",
    !!rmrf,
    rmrf ? "" : "rm -rf branch not found — a CLONED: externalCwd would leak on cleanup",
  );
}

console.log(`\n--- AMBIENT_MATCH gate present at all three worktree call sites (2026-08-19) ---`);
{
  const fs = await import("node:fs");
  const src = fs.readFileSync(WORKFLOW, "utf8");
  const ambientChecks = src.match(
    /git remote get-url origin 2>\/dev\/null \| grep -qi/g,
  );
  // Was 2 until 2026-09-11, when the pinned head checkout got its own early
  // dispatch (HEAD-WORKTREE region) so lenses could read source at the right
  // commit — the fused probe runs after the lens fan-out, too late for them.
  // That site builds a worktree in the ambient clone exactly like the other
  // two, so it needs the same guard: PR #1518 is the case where an unqualified
  // `worktree add` ran against a sibling repo's remote and the resulting null
  // path silently dropped `-C` from the external CLI call.
  check(
    "AMBIENT_MATCH computed via `git remote get-url origin` at every site that builds a worktree in the ambient clone",
    (ambientChecks || []).length === 3,
    `found ${(ambientChecks || []).length}, expected 3 (early setup-head-worktree + fused check-external-heads + standalone grounded-typecheck)`,
  );
  check(
    "the standalone grounded-typecheck dispatch degrades explicitly when ambient does not match",
    /if \[ "\$AMBIENT_MATCH" != yes \]; then echo "DEGRADED_WRONG_REPO"; exit 0; fi/.test(src),
  );
  check(
    "the gh-repo-clone fallback exists for the external-head worktree",
    /gh repo clone \$\{repo\} "\$WT" -- --quiet --depth 1 --no-single-branch/.test(src),
  );
}

// --- where the external CLIs are pinned, when the probe echoes nothing -----
//
// Added 2026-09-11 after the first live run of v67 found the defect this file
// was already describing in its own header: "`externalCwd` staying null when a
// worktree really was created just means external heads run in the base
// checkout, degraded, with a log line easy to miss in a long run." That is
// exactly what happened, and none of the checks above could see it, because
// the parser was right. The probe's worktree section only ECHOES a path when it
// CREATES one, and since v66 it usually creates nothing: the run materialises a
// pinned checkout earlier, so `WT` starts non-empty and the whole `if` body is
// skipped. The path existed and reached nobody.
//
// Measured on three live runs (PRs #2101, #2100, #2094, all 2026-09-11): every
// metrics row carries `headPin: {pinned: true, kind: "worktree"}` beside
// `cleanSweep.skippedReason: "no-pinned-head"`. The two fields contradict each
// other, and the contradiction had been shipping since v66.
console.log(`\n--- external cwd resolves from the probe OR the pinned checkout ---`);
{
  const { api: api2 } = loadRegion(WORKFLOW, "FUSED-AVAILABILITY-PARSE", [
    "resolveExternalCwd",
  ]);
  const { resolveExternalCwd } = api2;
  const PIN = { path: "/tmp/peer-review-wt-2101-DwbTF3", kind: "worktree", ok: true };

  check(
    "the probe's own path wins — it built that tree for this purpose",
    resolveExternalCwd(
      { externalCwd: "/tmp/probe-tree", externalCwdIsPlainClone: false },
      PIN,
    ).externalCwd === "/tmp/probe-tree",
  );
  // THE REGRESSION. Without the fallback this returns null and every external
  // CLI loses `-C` while a pinned tree sits on disk unused.
  check(
    "a null probe path falls back to the run's pinned checkout",
    resolveExternalCwd({ externalCwd: null, externalCwdIsPlainClone: false }, PIN)
      .externalCwd === PIN.path,
  );
  check(
    "…and the fallback carries the teardown distinction, so a clone is not left to `git worktree remove`",
    resolveExternalCwd(
      { externalCwd: null },
      { path: "/tmp/c", kind: "clone", ok: true },
    ).externalCwdIsPlainClone === true &&
      resolveExternalCwd({ externalCwd: null }, PIN).externalCwdIsPlainClone ===
        false,
  );
  check(
    "a CLONED path from the probe keeps its own plain-clone flag",
    resolveExternalCwd(
      { externalCwd: "/tmp/cloned", externalCwdIsPlainClone: true },
      PIN,
    ).externalCwdIsPlainClone === true,
  );
  // The controls. Without these, a resolver that simply returns headPin.path
  // unconditionally, or any non-null string, satisfies everything above.
  check(
    "no probe path and a FAILED pin resolves to null, not to the pin's empty path",
    resolveExternalCwd({ externalCwd: null }, { path: null, kind: "none", ok: false })
      .externalCwd === null,
  );
  check(
    "no probe path and no pin at all resolves to null",
    resolveExternalCwd({ externalCwd: null }, null).externalCwd === null &&
      resolveExternalCwd({}, undefined).externalCwd === null,
  );
}

// A pure function nobody calls passes every check above. This is the half that
// says the workflow actually uses it, and it is the half that was missing: the
// old code read `externalCwd = parsed.externalCwd` and stopped there.
console.log(`\n--- …and the workflow actually resolves it that way ---`);
{
  const fs = await import("node:fs");
  const src = fs.readFileSync(WORKFLOW, "utf8");
  check(
    "the run resolves externalCwd through resolveExternalCwd, not straight off the probe",
    /=\s*resolveExternalCwd\(/.test(src),
    "no call site — the resolver is dead code and the probe's null still wins",
  );
  check(
    "the pinned checkout is what it falls back to",
    /resolveExternalCwd\(\s*\{[^}]*\},\s*headPin,?\s*\)/.test(
      src.replace(/\n\s*/g, " "),
    ),
    "resolveExternalCwd is called with something other than headPin",
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall fused-availability-parse expectations hold");
