#!/usr/bin/env node
// check-mirror.mjs — the peer-review harness exists twice: as executable code in
// ~/.claude/workflows/peer-review.js (Workflow mode) and as prose the
// orchestrator follows verbatim in SKILL.md + references/external-heads.md
// (in-context mode, which SKILL.md's own routing sends the MAJORITY of PRs to).
// Nothing kept the two in agreement, and they have drifted repeatedly — most
// recently on 2026-07-22, when the fix for the discarded-votes bug landed in the
// workflow and left `grep -q "^VERDICT:"` standing in external-heads.md, i.e.
// the bug stayed live on the common path while being reported as fixed.
// external-heads.md's own header warns "if you edit one side, edit the other";
// a warning is not a mechanism. This is the mechanism.
//
// Rules are deliberately anchored to VALUES (numbers, enum members, script
// paths), never to wording — a checker that fails on rephrasing gets ignored,
// and an ignored checker is worse than none.
//
// A rule whose value cannot be EXTRACTED is a failure, not a skip: silent
// non-coverage is the thing being prevented.
//
// v101 removed the in-context path (SKILL.md's prose procedure, its references
// and external-heads.md). What stays is the other half of this file: values the
// Workflow shares with lib/, scripts/, agents and the eval scripts, each a copy
// something has to keep equal. Every rule now reads only the sources that
// still exist. A rule left comparing a single source, or nothing, is skipped and
// counted, never passed silently; one with `expect` still checks what is left.
//
// Usage: node eval/check-mirror.mjs [--verbose]

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WORKFLOW, PLUGIN_ROOT, agent, lib } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SKILL_DIR = resolve(HERE, "..");
const FILES = {
  workflow: WORKFLOW,
  persistSh: lib("persist-run.sh"),
  // fetch-pr's gh calls moved here from prompt text in tribunal.js (v72), so the
  // Workflow path spans this file too, the same way it spans persist-run.sh.
  fetchPr: join(SKILL_DIR, "scripts/fetch-pr-context.mjs"),
  skill: join(SKILL_DIR, "SKILL.md"),
  metrics: join(SKILL_DIR, "references/metrics-schema.md"),
  persist: join(SKILL_DIR, "eval/replay-persist.mjs"),
  latch: join(SKILL_DIR, "eval/replay-latch.mjs"),
  // The third site that spawns the verdict classifier. Added 2026-08-12
  // (the EFTYPE fix): the interpreter-form rule below has to see all three, and this
  // file was the one not already reachable from any extractor.
  verdicts: join(SKILL_DIR, "eval/replay-verdicts.mjs"),
  intake: join(SKILL_DIR, "eval/replay-intake.mjs"),
  reconcile: lib("reconcile-votes.mjs"),
  verifier: agent("finding-verifier.md"),
  refuterAgent: agent("tribunal-refuter.md"),
  // The scorer's own copy of the operatorOverride vocabulary. Added 2026-07-29:
  // it forked from every other surface (it said reinstated|rejected while the
  // workflow, both docs and the corpus row said `dropped`), score.mjs refused
  // the one row that used it, and the corpus ran on two ground-truth labels
  // while appearing to have three — all inside this checker's blind spot,
  // because the operatorOverride rule below anchored on SKILL.md alone.
  overrideMjs: join(SKILL_DIR, "eval/operator-override.mjs"),
  // The verifiedBy evidence gate's real, importable copy (in-context mode runs
  // this for real via Bash). The workflow's VERIFIED-GATE region is a mirrored
  // inline copy (Workflow scripts have no filesystem/import access) — the rule
  // below pins the one concrete value both copies must share.
  verified: lib("verified.mjs"),
  // The routing decision's real, importable copy (SKILL.md's Dispatch decision
  // pipes `gh pr view` straight into it) and this plugin's own README, which
  // restates the two thresholds for a human reading the top-level docs.
  routeScript: join(SKILL_DIR, "scripts/route-review.mjs"),
  readme: resolve(PLUGIN_ROOT, "README.md"),
  // The incremental-re-review baseline predicate's real, importable copy
  // (2026-09-10) — it used to be Python embedded in WORKFLOW's PYPRIOR
  // heredoc, which is what the operatorOverride rule below anchored on. Moved
  // here so that anchor still points at the code that actually runs.
  resolveBaseline: join(SKILL_DIR, "scripts/resolve-baseline.mjs"),
};
const verbose = process.argv.includes("--verbose");
const src = Object.fromEntries(
  Object.entries(FILES).map(([k, p]) => [k, readFileSync(p, "utf8")]),
);
// Same treatment for the Workflow side, and for the same reason. The persist
// step's bash used to be prompt text inside peer-review.js, where a haiku had to
// retype all 9.5 KB of it on every run; it now lives in lib/persist-run.sh. The
// invariants below are claims about what the WORKFLOW PATH does, and that path
// spans both files — three rules (truncation guard, wall clock, harnessVersion)
// went red the moment the text moved, which is the mirror correctly noticing and
// not a reason to weaken them.
src.workflow = `${src.workflow}\n${src.persistSh}\n${src.fetchPr}`;

// --- extractors -------------------------------------------------------------
const num = (re) => (text) => {
  const m = text.match(re);
  return m ? Number(m[1]) : null;
};
// The HIGHEST number across every occurrence — for a doc that accumulates one
// section per schema version rather than rewriting a single one. `num` takes
// the first match, which silently reports the OLDEST version the moment a
// second section is correctly added, and would do so on every future bump.
const maxNum = (re) => (text) => {
  const found = [...text.matchAll(re)].map((m) => Number(m[1]));
  return found.length ? Math.max(...found) : null;
};
// The external-head failure classes, in ONE place — this list was previously
// hardcoded three times inside the failureReason invariant, so adding a class
// to peer-review.js reported drift against docs that already documented it.
// Adding a class means adding it here, and nowhere else in this file.
const FAILURE_CLASSES = [
  "timeout",
  "error",
  "quota",
  "auth",
  "refused",
  "unparseable",
  "confined",
];
// A set of enum members, gathered from every occurrence of a pattern.
const members = (re, allowed) => (text) => {
  const found = new Set();
  for (const m of text.matchAll(re)) {
    const token = (m[1] || "").toLowerCase();
    if (allowed.includes(token)) found.add(token);
  }
  return found.size ? [...found].sort() : null;
};
const present = (needle) => (text) => (text.includes(needle) ? true : false);
// Every match of a global regex must agree on the same value, or the extractor
// itself reports drift (null, which the runner already treats as failure) —
// a stale restatement left behind after only one of several mentions was
// updated is exactly the case this is for.
const uniform = (re) => (text) => {
  const found = [...text.matchAll(re)].map((m) => Number(m[1]));
  if (!found.length) return null;
  return found.every((n) => n === found[0]) ? found[0] : null;
};
// Only the fenced blocks — the in-context path executes those verbatim, while
// the surrounding prose is free to discuss superseded behaviour.
const codeBlocks = (text) =>
  [...text.matchAll(/```[\s\S]*?```/g)].map((m) => m[0]);
// The workflow-side equivalent of codeBlocks: peer-review.js earns the same
// right its markdown siblings already have — to DISCUSS a superseded mechanism
// without the checker reading the description as a reintroduction. Drops
// whole-line `//` comments only, never a trailing comment and never anything
// inside a template literal, so an executable copy living in one of the bash
// heredocs is still caught. Added 2026-07-24 after a comment explaining WHY
// classify-verdict.sh exists tripped the "raw ^VERDICT: grep" invariant — the
// invariant's own note already said prose is allowed to mention it, but only
// the `heads` extractor implemented that.
const codeOnly = (text) =>
  text
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");

const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
// Workflow-only structural check (no `skill` key — the in-context path has
// no per-call agent() dispatch to reorder, so there is nothing to mirror
// against; omitting `skill` from `extract` means only `workflow` is checked,
// per the runner loop below). "===== YOUR CASE =====" is not globally
// unique — it appears once per refuter-family site (2026-08-18: claudeRefuter,
// claudeRefuterBatchPrompt, strengthenPromptFor, defense, ruling) — so each
// call anchors on `opener`, a substring unique to its own function, then
// searches forward from there for that occurrence's own marker and the
// static/dynamic anchors that must sit before/after it, the same technique
// the lens rule above uses via `text.indexOf(..., fromIndex)`.
const refuterFamilyOrderingRule = (
  name,
  opener,
  staticAnchors,
  dynamicAnchors,
) => ({
  name,
  extract: {
    workflow: (text) => {
      const openerIdx = text.indexOf(opener);
      if (openerIdx === -1) return null;
      const markerIdx = text.indexOf("===== YOUR CASE =====", openerIdx);
      if (markerIdx === -1) return null;
      for (const a of staticAnchors) {
        const idx = text.indexOf(a, openerIdx);
        if (idx === -1 || idx > markerIdx) return false;
      }
      for (const a of dynamicAnchors) {
        const idx = text.indexOf(a, markerIdx);
        if (idx === -1 || idx < markerIdx) return false;
      }
      return true;
    },
  },
  expect: true,
});

// --- rules ------------------------------------------------------------------
// Each rule extracts the same value from 2+ files and requires agreement.
const RULES = [
  {
    name: "T_KILL threshold",
    extract: {
      workflow: num(/const T_KILL = ([0-9.]+)/),
      skill: num(/killed iff `refuteFraction ≥ ([0-9.]+)`/),
    },
  },
  {
    name: "DELTA (borderline band)",
    extract: {
      workflow: num(/const DELTA = ([0-9.]+)/),
      skill: num(
        /borderline band\*\* `\|refuteFraction − [0-9.]+\| ≤ ([0-9.]+)`/,
      ),
    },
  },
  {
    name: "GEMINI_CONFIDENCE_CAP",
    extract: {
      workflow: num(/const GEMINI_CONFIDENCE_CAP = ([0-9.]+)/),
      skill: num(/Gemini slot's confidence is capped at ([0-9.]+)\*\*/),
      heads: num(/declared confidence is capped at ([0-9.]+) in the tally/),
    },
  },
  {
    // Three copies of the same threshold: the Workflow's inlined VERIFIED-GATE
    // region (no import access — see that region's header), the real,
    // importable lib/verified.mjs the in-context path runs via Bash, and the
    // in-context prose describing the rule to a human/orchestrator reader.
    // Anchored on the number itself, not the word "quote" — see file header.
    name: "verifiedBy MIN_QUOTE_CHARS",
    extract: {
      workflow: num(/const VERIFIED_GATE_MIN_QUOTE_CHARS = ([0-9]+)/),
      verified: num(/const MIN_QUOTE_CHARS = ([0-9]+)/),
      skill: num(/a quote under ([0-9]+) characters/),
    },
  },
  {
    // The field name itself must appear on every path that can produce or
    // consume it — the schema (workflow), the mirrored inline gate (workflow),
    // the real gate module (verified), and the in-context instructions telling
    // a human orchestrator to require and check it.
    name: "verifiedBy field present on every path",
    extract: {
      workflow: present("verifiedBy"),
      verified: present("verifiedBy"),
      skill: present("verifiedBy"),
    },
    expect: true,
  },
  {
    name: "failureReason classes",
    extract: {
      workflow: members(
        /failureReason[\s\S]{0,200}?enum: \[([^\]]+)\]/g,
        FAILURE_CLASSES,
      ),
      // fall back to listing the enum members individually
      skill: (text) => {
        const found = FAILURE_CLASSES.filter((c) =>
          new RegExp("`" + c + "`").test(text),
        );
        return found.length ? found.sort() : null;
      },
      heads: (text) => {
        const found = FAILURE_CLASSES.filter((c) =>
          new RegExp("`" + c + "`").test(text),
        );
        return found.length ? found.sort() : null;
      },
    },
    // The workflow extractor returns the enum contents as one blob; normalise.
    normalise: {
      workflow: (v, text) => {
        const m = text.match(
          /failureReason: \{\s*type: "string",\s*enum: \[([^\]]+)\]/,
        );
        if (!m) return null;
        return m[1]
          .split(",")
          .map((s) => s.trim().replace(/"/g, ""))
          .filter(Boolean)
          .sort();
      },
    },
  },
  {
    name: "sticky-routing trigger set",
    extract: {
      workflow: (text) => {
        const m = text.match(
          /if \(!\[([^\]]+)\]\.includes\(failureReason\)\) return;/,
        );
        if (!m) return null;
        return m[1]
          .split(",")
          .map((s) => s.trim().replace(/"/g, ""))
          .filter(Boolean)
          .sort();
      },
      skill: (text) => {
        const m = text.match(
          /\*\*Sticky routing — for ([^.]+?), never `refused`\.\*\*/,
        );
        if (!m) return null;
        return [...m[1].matchAll(/`([a-z]+)`/g)].map((x) => x[1]).sort();
      },
    },
  },
  {
    // gap 1 (specs/2026-07-29-external-review-concepts.md): the verdict is a
    // code-health rule, not a severity tally — a surviving `important` finding
    // that is behaviour-changing or on a high-risk surface must escalate to
    // REQUEST CHANGES rather than grade the same as a style nit.
    name: "verdict rule: important + isBehaviorChanging/touchesHighRiskSurface escalates",
    extract: {
      // Regex, not present()'s exact substring: prettier is free to wrap this
      // boolean expression across lines at the 80-col width, and did — a
      // literal-substring check broke on reformatting alone, with the rule's
      // actual logic untouched. \s* tolerates any wrapping, one line or many.
      workflow: (text) =>
        /f\.severity === "important" &&\s*\(f\.isBehaviorChanging \|\| f\.touchesHighRiskSurface\)/.test(
          text,
        ),
      skill: present("isBehaviorChanging` or `touchesHighRiskSurface"),
    },
    expect: true,
  },
  {
    name: "verdict rule: nits alone never withhold approval",
    extract: {
      workflow: present("nits alone must never withhold approval"),
      skill: present("nits alone must never withhold approval"),
    },
    expect: true,
  },
  {
    // specs/2026-07-29-external-review-concepts.md, Part 3, "verified defect":
    // in-context-review.md explicitly names `code-review-expert` for both the
    // defense and the adjudicator (judge) dispatch, but the Workflow ran both
    // on the generic default agent with no `agentType` at all — a real
    // divergence check-mirror.mjs never caught because it never checked
    // agentType. Anchored to the dispatch's own `label:` value so a rename of
    // either role's label fails this loudly instead of silently widening the
    // match to something unrelated.
    // Amended 2026-08-10: the reviewer became per-repo (`LENS_AGENT` in the
    // Workflow, `<LENS_AGENT>` in the skill), so the invariant is no longer
    // "both name code-review-expert" but "both name the SAME agent the review
    // lenses use". The workflow side is unchanged — it already anchored on the
    // constant rather than its value, which is why only the prose side moved.
    name: "debate dispatches (defense, judge) use the repo's lens agent on both paths",
    extract: {
      workflow: (text) => {
        const defenseOpts = text.match(
          /label: `defense:[^`]*`,[\s\S]{0,700}?\n\s*\},\n\s*\);/,
        );
        const judgeOpts = text.match(
          /label: `judge:[^`]*`,[\s\S]{0,700}?\n\s*\},\n\s*\);/,
        );
        if (!defenseOpts || !judgeOpts) return null;
        return (
          defenseOpts[0].includes("agentType: LENS_AGENT") &&
          judgeOpts[0].includes("agentType: LENS_AGENT")
        );
      },
      skill: (text) =>
        /\*\*Defense\*\* — dispatch one `<LENS_AGENT>`/.test(text) &&
        /\*\*Adjudicator \(judge lens\)\*\* — dispatch one `<LENS_AGENT>`/.test(
          text,
        ),
    },
    expect: true,
  },
  {
    // specs/2026-07-29-external-review-concepts.md, Part 3, "standing question":
    // the precondition for shipping peer-review-refuter.md at all is that
    // check-mirror.mjs actually diffs its contract against the inline prompt —
    // otherwise the two are related only by whoever last remembered to sync
    // them, which is the exact failure class this whole file exists to close.
    // v91: the verdict is factual only, so the two kill reasons are the whole
    // list on both sides; the judgement categories moved to their own fields.
    name: "refuter agent definition: killReason enum matches the inline REFUTE_TAIL",
    extract: {
      workflow: (text) => {
        const m = text.match(
          /THE FACT \(verdict\)[\s\S]{0,600}?A REFUTED vote without that quote does not count\./,
        );
        if (!m) return null;
        return [...m[0].matchAll(/"([a-z-]+)"/g)].map((x) => x[1]).sort();
      },
      refuterAgent: (text) => {
        const m = text.match(
          /exactly two ways that can be true:[\s\S]{0,400}?Put one of them in `killReason`/,
        );
        if (!m) return null;
        return [...m[0].matchAll(/`([a-z-]+)`/g)].map((x) => x[1]).sort();
      },
    },
  },
  {
    // members() alone returns the whole bracketed enum as ONE ungrouped token
    // (see "failureReason classes" above, which needs a `normalise.workflow`
    // step for exactly that reason) — this extractor splits it directly
    // instead, so it doesn't depend on remembering that gotcha.
    name: "refuter agent definition: refutationType enum matches",
    extract: {
      workflow: (text) => {
        const m = text.match(/refutationType[\s\S]{0,80}?enum: \[([^\]]+)\]/);
        if (!m) return null;
        return [...m[1].matchAll(/"([A-Z_]+)"/g)]
          .map((x) => x[1].toLowerCase())
          .sort();
      },
      refuterAgent: (text) => {
        const found = [
          "PRE_DECIDED",
          "TECHNICALLY_WRONG",
          "NO_REFUTATION",
        ].filter((c) => new RegExp("`" + c + "`").test(text));
        return found.length ? found.map((c) => c.toLowerCase()).sort() : null;
      },
    },
  },
  {
    name: "refuter agent definition: core disposition matches the inline prompt",
    extract: {
      workflow: present(
        "Does the mechanism the finding describes actually happen at the pinned commit?",
      ),
      refuterAgent: present(
        "Does the mechanism the finding describes actually happen at the pinned commit?",
      ),
    },
    expect: true,
  },
  {
    name: "refuter agent definition: batch anti-drift instruction matches",
    extract: {
      workflow: present("Similarity between two cases is coincidence"),
      refuterAgent: present("Similarity between two cases is coincidence"),
    },
    expect: true,
  },
  {
    // Found 2026-08-03 cross-referencing an Anthropic hallucination-reduction
    // guide against this harness: externalPromptText's own comment says "One
    // prompt body, shared by both families... a per-family prompt is a second
    // variable in an experiment that already has one" — but nothing enforced
    // that between the Workflow copy (externalPromptText) and the in-context
    // copy (external-heads.md's Codex bash block), and they HAD drifted
    // (external-heads.md carried "than the rest of the panel", tribunal.js
    // didn't). Anchored on the full disposition sentence, not just a
    // substring, so a future edit to either copy that drops or changes the
    // clause fails loudly instead of silently.
    name: "external-head disposition instruction matches between Workflow and in-context paths",
    extract: {
      // "than the rest of the" is the exact clause that had drifted (present
      // in external-heads.md, missing from tribunal.js) — anchored short and
      // on a single line deliberately, since external-heads.md hard-wraps its
      // bash heredoc at ~80 cols and a longer needle would cross a line break
      // present() can't see through.
      workflow: present("than the rest of the panel"),
      heads: present("than the rest of the"),
    },
    expect: true,
  },
  {
    // This rule was GREEN BY VACUITY until 2026-08-12 (the EFTYPE fix). Its needle was
    // `~/.claude/lib/classify-verdict.sh` — the pre-plugin layout — and neither
    // side has contained that string since the harness was packaged: the
    // workflow spells it `"$(tribunal-root)"/lib/…` and external-heads.md spells
    // it `${CLAUDE_PLUGIN_ROOT}/lib/…`. With no `expect: true` the rule passed on
    // AGREEMENT, and two falses agree, so a rule named "both paths use the shared
    // verdict classifier" was asserting nothing at all. That is plausibly why
    // nobody noticed lib/reconcile-votes.mjs had kept the dead homedir path.
    //
    // Anchored on the suffix common to both spellings, pinned with an explicit
    // `expect: true` so absence can never again be read as agreement, and scoped
    // to EXECUTABLE text via codeOnly/codeBlocks — the same helpers the
    // neighbouring `^VERDICT:` rule uses, for the same reason.
    //
    // That scoping is PARTIAL, and the limit belongs here rather than in
    // whoever rediscovers it: `codeOnly` strips whole-line `//` comments only —
    // deliberately, so an executable copy inside a bash heredoc is still seen —
    // so a TRAILING comment carrying the needle still satisfies the rule with the
    // real invocation gone. Measured 2026-08-12, both here and on the interpreter
    // rule below: replace the call with a no-op plus a trailing comment holding
    // the old text and both stay green at 66/66. The rule below is NOT immune
    // to it either; anchoring on call syntax instead of a path changes what gets
    // caught, not that limitation.
    //
    // Kept anyway, because the two shapes are not equally likely. What these
    // rules exist to catch is an edit that CHANGES a live call — reverting
    // `execFileSync("bash", [X, …])` to `execFileSync(X, …)`, or repointing a
    // path — and all four of those mutations are caught (measured). What escapes
    // is deleting a call and leaving its text behind in a trailing comment, which
    // is not something an edit does by accident. Closing it needs a `codeOnly`
    // that understands trailing comments without breaking on `https://` inside a
    // string, and that is shared by every rule in this file — its own ticket.
    name: "both paths use the shared verdict classifier",
    extract: {
      workflow: (text) => codeOnly(text).includes("/lib/classify-verdict.sh"),
      heads: (text) =>
        codeBlocks(text).some((b) => b.includes("/lib/classify-verdict.sh")),
    },
    expect: true,
  },
  {
    // The EFTYPE fix, 2026-08-12. Every Node caller must spawn the classifier THROUGH bash, never
    // as a directly-executable file. Windows cannot exec a `.sh` whatever its
    // shebang or exec bit, so `execFileSync(CLASSIFIER, [...])` dies with EFTYPE
    // — and in lib/reconcile-votes.mjs, which runs on every real review, the
    // error was swallowed and reported as `vote-without-verdict` on every slot.
    //
    // This is a SOURCE-level pin because no behavioural test can replace it. On
    // POSIX the direct-exec spelling works fine (the file is mode 0755 with a
    // `#!/usr/bin/env bash` shebang), so a regression here is invisible to the
    // battery on macOS and Linux and only turns red on Windows. A guard that
    // fires on one platform is not a guard; this one reads the text and fires
    // everywhere.
    //
    // Written as a negative plus a positive: the negative is what actually
    // catches the regression (the three files spell their variable differently —
    // CLASSIFIER, CLASSIFIER, classify — and quote 'bash' differently, so no
    // single positive needle spans them), and the positive stops the rule going
    // vacuous if a call site is deleted outright.
    // The EFTYPE fix. `classifierFailed` is a cross-path field, and every other recent
    // one — operatorOverride, agentCost, lensAgent, persistVerified — earns a
    // rule here. Two of those were added only after PR #44's review pointed out
    // that a new cross-path contract without a mirror rule "looks like an
    // oversight rather than a decision"; this is the same shape one commit later,
    // so it gets its rule at the same time as the field.
    //
    // Five surfaces, because the field is worthless if any single one drops it:
    // the reconciler emits it, persist-run.sh carries it into the row, the
    // workflow branches on it when deciding what to tell the operator, the schema
    // documents it for whoever reads metrics.jsonl a year from now, and the
    // in-context path is instructed to record it — that last one matters most,
    // since it is the path whose instruction is prose rather than code.
    // Two anchoring decisions here, both forced by mutation-testing this rule
    // rather than reasoned up front — the first draft passed while four of the
    // five surfaces were broken.
    //
    // 1. Word-anchored, not `present()`. A substring match is satisfied by any
    //    longer identifier containing the name, so renaming the field to
    //    `classifierFailedCount` on ONE surface left the rule green with the
    //    surfaces out of sync.
    // 2. The `workflow` slot anchors on the property READ, not the bare name.
    //    `src.workflow` is tribunal.js CONCATENATED with lib/persist-run.sh (see
    //    the note above the concatenation), so a bare-name test there is really
    //    "either file has it" — it can never fail while persist-run.sh still
    //    carries the field, which makes the slot redundant with `persistSh` and
    //    blind to the workflow dropping its read. The concatenation note argues a
    //    wider haystack cannot make a rule wrongly pass; that holds while only
    //    one of the two files is expected to carry the string, and this field is
    //    the case where both do.
    name: "classifierFailed: emitted, persisted, read, documented, and recorded in-context",
    extract: {
      reconcile: (t) => /\bclassifierFailed\b/.test(t),
      persistSh: (t) => /\bclassifierFailed\b/.test(t),
      workflow: (t) => /voteReconciliation\.classifierFailed\b/.test(t),
      metrics: (t) => /\bclassifierFailed\b/.test(t),
      inContext: (t) => /\bclassifierFailed\b/.test(t),
    },
    expect: true,
  },
  {
    name: "verdict classifier is spawned through an interpreter, never exec'd directly",
    extract: {
      reconcile: (text) =>
        !/execFileSync\(\s*CLASSIFIER\s*,/.test(codeOnly(text)) &&
        /execFileSync\(\s*["']bash["']\s*,\s*\[\s*CLASSIFIER\b/.test(
          codeOnly(text),
        ),
      verdicts: (text) =>
        !/execFileSync\(\s*CLASSIFIER\s*,/.test(codeOnly(text)) &&
        /execFileSync\(\s*["']bash["']\s*,\s*\[\s*CLASSIFIER\b/.test(
          codeOnly(text),
        ),
      latch: (text) =>
        !/execFileSync\(\s*classify\s*,/.test(codeOnly(text)) &&
        /execFileSync\(\s*["']bash["']\s*,\s*\[\s*classify\b/.test(
          codeOnly(text),
        ),
    },
    expect: true,
  },
  {
    // Scoped to executable text on purpose — fenced blocks in the markdown,
    // non-comment lines in the workflow. All three files legitimately DISCUSS
    // the old classifier (that history is why the helper exists); what must not
    // survive is an executable copy of it, since the in-context path runs these
    // blocks verbatim.
    name: "no executable block still uses the raw ^VERDICT: grep",
    extract: {
      workflow: (text) => !codeOnly(text).includes('grep -q "^VERDICT:"'),
      heads: (text) =>
        !codeBlocks(text).some((b) => b.includes('grep -q "^VERDICT:"')),
    },
    expect: true,
  },
  {
    name: "dedup: cross-file merges are forbidden",
    extract: {
      workflow: present("crossFileClustersRejected"),
      skill: present("Never merge across files"),
    },
  },
  {
    name: "dedup: within-file proximity window",
    extract: {
      workflow: num(/const MERGE_LINE_WINDOW = (\d+)/),
      skill: num(/within ~(\d+) lines of each other/),
    },
  },
  {
    name: "dedup: title-overlap threshold",
    extract: {
      workflow: (text) => {
        const m = text.match(/const TITLE_OVERLAP_MIN = ([0-9.]+)/);
        return m ? Math.round(Number(m[1]) * 100) : null;
      },
      skill: num(/titles share ≥(\d+)% of their content words/),
    },
  },
  {
    name: "panel membership: vetted bot findings",
    extract: {
      workflow: present('finding.lens === "bot-comments"'),
      skill: present("**vetted bot comment** classified VALID"),
    },
  },
  {
    name: "panel membership: RETEST-named claims",
    extract: {
      workflow: present("isRetestTarget(finding)"),
      skill: present("named in `--context` with `RETEST:`"),
    },
  },
  {
    // The count guard is duplicated between the workflow's persist-metrics bash
    // and eval/replay-persist.mjs, which is what proves the guard works. If the
    // refusal message drifts, the replay is testing text the workflow no longer
    // emits — green, and guarding nothing.
    name: "metrics row: truncation guard refuses partial writes",
    extract: {
      workflow: present("REFUSED: record carries %d findings, expected"),
      persist: present("REFUSED: record carries %d findings, expected"),
    },
    expect: true,
  },
  {
    // The auditor has to build the SAME filename the run wrote, and the payload
    // has to tell it which round to build. Both halves are required and neither
    // is sufficient: while the reconciler hardcoded `<slot>-<source>.out` and
    // `externalVotes` dropped `round`, every refined slot was audited against
    // its superseded round-0 answer — 17 spurious mismatches and a failed
    // self-audit on a run that was 20 of 20 clean (2026-07-25 #1911). Anchored
    // on the round suffix itself, which is the value that must agree.
    name: "reconciliation is round-aware on both sides",
    extract: {
      workflow: (s) =>
        // externalVotes — the payload handed to the reconciler — carries round.
        /slotStatus: vote\.slotStatus,[\s\S]{0,1600}?round: v\.refined \? 1 : 0/.test(
          s,
        ),
      reconcile: (s) =>
        present('`${slotId}${round ? `r${round}` : ""}-${source}`')(s) &&
        present("vote.round")(s),
    },
    expect: true,
  },
  {
    // A superseded round is not a dropped answer, and the row's reader has to
    // be able to tell them apart — the kinds list is the contract.
    name: "reconciliation kinds: superseded-round is documented, not an orphan",
    extract: {
      reconcile: members(/"(orphan-artifact|superseded-round)"/g, [
        "orphan-artifact",
        "superseded-round",
      ]),
      metrics: members(/`(orphan-artifact|superseded-round)`/g, [
        "orphan-artifact",
        "superseded-round",
      ]),
    },
  },
  {
    // A bash fragment like `${TMPDIR:-/tmp}/...` is safe when it is expanded and
    // consumed inside ONE shell (the worktree setup does exactly that and echoes
    // the resolved path back). It is a defect when the STRING is shared across
    // shells, because each agent resolves it differently — which is how the
    // artifacts split in two on 2026-07-23 and every external vote was lost.
    // The artifact directory is the shared-across-shells case, so neither path
    // may name it with an unexpanded fragment.
    name: "artifact dir is an absolute path, not a per-shell fragment",
    extract: {
      workflow: (text) =>
        !/const VOTE_DIR = `\\\$\{TMPDIR/.test(text) &&
        text.includes("resolvedVoteDir ||"),
      heads: (text) =>
        !codeBlocks(text).some((b) => /VOTES="\$\{TMPDIR/.test(b)),
    },
    expect: true,
  },
  {
    // Both paths must refuse to call an unexamined finding blocking. The two
    // sides say it in their own words — anchor on the enforceable clause.
    name: "nothing unadjudicated is reported as Blocking",
    extract: {
      workflow: present('MUST NOT appear under "Blocking"'),
      skill: present("Nothing unadjudicated may be reported as Blocking"),
    },
    expect: true,
  },
  {
    // Until this was added, selfAudit — the self-audit that once caught a
    // run with 0 external voters over 26 panels while every offline replay
    // stayed green — ran ONLY in Workflow mode. SKILL.md's own routing sends
    // the majority of PRs through in-context instead, so the majority of
    // runs carried no self-audit at all and nothing said so. Anchored on the
    // script name because that is the thing that must not silently stop
    // being called on either path.
    name: "self-audit (audit-run.mjs) runs on both paths",
    extract: {
      workflow: present("audit-run.mjs"),
      skill: present("audit-run.mjs"),
    },
    expect: true,
  },
  {
    // The Workflow computes findings[].mergedLocationsMissingFromReport
    // inline against the same MERGED-LOCATIONS-CHECK region this script
    // names; the in-context path has no in-process `report` variable to
    // compare against, so it runs the same check as a script instead —
    // lib/check-report-fidelity.mjs, sliced from that exact region. This
    // anchors that both paths actually reference it: a doc that stopped
    // calling the script would silently drop selfAudit's
    // merged-locations-dropped coverage for the majority of PRs (in-context
    // is where SKILL.md routes most of them) while the Workflow kept
    // catching it — the asymmetry this file exists to prevent.
    name: "merged-locations report check runs on both paths",
    extract: {
      workflow: present("mergedLocationsMissingFromReportText"),
      skill: present("check-report-fidelity.mjs"),
    },
    expect: true,
  },
  {
    // Same asymmetry risk as the merged-locations rule above, for the three
    // other report-fidelity checks (killReason citation, mergedFrom's
    // "(found by: ...)" note, grounded's typecheck citation) added alongside
    // it — all four are patched by the SAME lib/check-report-fidelity.mjs
    // script in-context, so one rule per function name confirms the doc
    // still calls out to all of them, not just the one that happened to ship
    // first.
    name: "report-fidelity checks (killReason/mergedFrom/grounded/unexamined wording) run on both paths",
    extract: {
      workflow: (text) =>
        text.includes("killReasonMissingFromReportText") &&
        text.includes("mergedFromAnnotationMissingFromReportText") &&
        text.includes("groundedDiagnosticMissingFromReportText") &&
        text.includes("unexaminedWordingInReportText"),
      skill: present("check-report-fidelity.mjs"),
    },
    expect: true,
  },
  {
    // The report the critic judged has to outlive the run on both paths too,
    // and for the same reason v63 added it in Workflow mode: a violation
    // whose evidence lived only in one agent turn's context named a drift
    // nobody could go back and read.
    name: "the report the critic judged is persisted on both paths",
    extract: {
      persistSh: present("report.md"),
      skill: present("report.md"),
    },
    expect: true,
  },
  {
    // The bypass was `important` + no risk flags falling through to `return
    // false`. If that branch comes back, the invariant above degrades a real
    // blocker to a nit line instead of failing loudly — so intake is pinned too.
    // The <dependents> block was Workflow-only, and the in-context path is the
    // one small PRs take. Both paths must render it from the same region.
    name: "dependents block: rendered on both paths from DEPENDENTS-BLOCK",
    extract: {
      workflow: (text) => text.includes("// ===== BEGIN DEPENDENTS-BLOCK =====") && /const dependents = dependentsBlock\(blastRadius\);/.test(text),
      skill: (t) => t.includes("scripts/render-dependents.mjs <diffPath> <repo checkout> --rev <HEAD_SHA>") && t.includes("above the `===== YOUR PASS =====` marker"),
    },
    expect: true,
  },
  {
    name: "panel intake: every non-nit earns a panel",
    extract: {
      workflow: (text) => {
        const at = text.indexOf("function needsPanel(finding) {");
        if (at === -1) return null;
        const body = text.slice(at, text.indexOf("\n}", at));
        return (
          /if \(finding\.severity === "nit"\) return false;/.test(body) &&
          /\n  return true;$/.test(body)
        );
      },
      skill: present("**1 refuter — never zero.**"),
    },
    expect: true,
  },
  {
    // The single-shot latch has to exist on BOTH execution paths or the leak it
    // closes stays open on the one that reviews most PRs. Anchored to the
    // `mkdir` guard itself, not to the surrounding prose. Three latched blocks
    // since 2026-08-14: codex refuter, gemini refuter, and the clean-verdict
    // sweep — a new CLI block without the latch must fail this, and a removed
    // block must too.
    name: "external heads: single-shot latch on both paths",
    extract: {
      workflow: (text) =>
        text.includes('`if ! mkdir "${lockPath}" 2>/dev/null; then`'),
      heads: (text) =>
        codeBlocks(text).filter((b) => b.includes('mkdir "$LOCK" 2>/dev/null'))
          .length === 3,
    },
    expect: true,
  },
  {
    // The latch is only useful if the refusal it records is still reported. If
    // the marker drifts on either side, blocked repeats become invisible —
    // which is exactly the state this work was undoing.
    name: "latch: blocked-repeat marker",
    extract: {
      workflow: present('echo "BLOCKED: 1"'),
      heads: present('echo "BLOCKED: 1"'),
      reconcile: present("BLOCKED:"),
      latch: present("BLOCKED: 1"),
    },
    expect: true,
  },
  {
    // Which Codex model actually voted has to be captured on both paths or
    // codexModelUsed is silently null on whichever one drifts — the same class
    // of gap harnessVersion was added to close, just one field later. Anchored
    // to the sidecar line itself (workflow/heads), the parser that reads it
    // back (reconcile), and the schema doc that names the derived field.
    name: "external heads: MODEL: sidecar line, read back and documented",
    extract: {
      workflow: present('echo "MODEL: $MODEL" >> "${metaPath}"'),
      heads: present('echo "MODEL: $MODEL" >> "$META"'),
      reconcile: present("codexModelsUsed"),
      metrics: present("codexModelUsed"),
    },
    expect: true,
  },
  {
    // An open CHANGES_REQUESTED must reach the report as a REPEAT, never as a
    // pre-decided item. Both paths have to keep those two channels apart: fold
    // the open blocker into the "do NOT re-litigate" block on either side and a
    // refuter kills it as PRE_DECIDED, so the report calls the PR clean while
    // the bug stands. Anchor on the section header each path emits.
    name: "open blockers are repeats, not pre-decided",
    extract: {
      workflow: present("### Repeat (already open in the thread)"),
      skill: present("### Repeat (already open in the thread)"),
    },
    expect: true,
  },
  {
    // The post gate is the enforceable half. `--post` authorises posting a
    // review, not stacking a second one on your own still-open block, and
    // "we could not tell who we are" must degrade to asking rather than to
    // posting. The workflow computes the value; SKILL.md Step 4 obeys it — so
    // the three values have to exist on both sides or the gate is decorative.
    name: "duplicate-review gate: three values on both paths",
    extract: {
      workflow: (text) =>
        ["clear", "own-open-review", "unknown-viewer"].every((v) =>
          text.includes(`"${v}"`),
        ),
      skill: (text) =>
        ["clear", "own-open-review", "unknown-viewer"].every((v) =>
          text.includes(`\`${v}\``),
        ),
    },
    expect: true,
  },
  {
    // Both paths must emit the field and neither may fill it. The whole value
    // of an operator override is that a human, not a run, put it there; a path
    // that omits the key makes "not overridden" indistinguishable from "this
    // path never records overrides", and a path that fills it in turns the
    // corpus's only non-model ground truth back into model output.
    name: "operatorOverride: emitted null by both paths, filled by neither",
    extract: {
      workflow: present("operatorOverride: null"),
      skill: present("`operatorOverride: null`"),
    },
    expect: true,
  },
  {
    // The row has to carry the input side or it cannot see a cost change at all:
    // output is ~13% of the dollars. Both paths must emit the key, and the
    // in-context path must say so explicitly rather than omitting it — an absent
    // field and an uncaptured one are indistinguishable, and here the source data
    // (per-agent transcripts) is deleted, so "capture it later" does not exist.
    name: "agentCost: emitted by both paths",
    extract: {
      workflow: present("agentCost: null"),
      skill: present("`agentCost`"),
    },
    expect: true,
  },
  {
    // Reviewer provenance, added 2026-08-10 when the lens agent became per-repo.
    // Both paths must write it, and the in-context one has the weaker guarantee
    // of the two: the Workflow stamps a constant it computed itself, while this
    // path depends on an orchestrator reading a table and remembering. Without
    // this rule that obligation is prose with no mechanism behind it, which is
    // exactly how `agentsActual` stayed null on every batched row. A row that
    // cannot say which reviewer produced it is uncomparable, and the harness's
    // corpus, cost bands and thresholds were all measured on the BE one — so a
    // missing `lensAgent` does not read as "unknown", it reads as BE.
    name: "lensAgent: emitted by both paths",
    extract: {
      workflow: present("lensAgent: LENS_AGENT"),
      skill: present('`"lensAgent"`'),
    },
    expect: true,
  },
  {
    // The read-back's answer has to REACH the caller, on both paths. Detection
    // was never the broken half — `persistOk` was computed correctly and was
    // false — but its only consequence was a `log()`, which goes to the
    // Workflow's progress narration and not into the return value a background
    // invocation receives. So the orchestrator saw `{report, metrics}` and a
    // passing self-audit over a run that had written no row (FE #1516). This
    // pins both halves of the contract: the Workflow returns the pair, and the
    // skill reads it before anything else in Step 4.
    name: "persistVerified/persistDetail: returned by the Workflow, read by the skill",
    extract: {
      workflow: present(
        "return { report, metrics, persistVerified: persistOk, persistDetail };",
      ),
      skill: (s) =>
        present("persistVerified: <bool>, persistDetail: <string|null>")(s) &&
        present(
          "**Read `persistVerified` on that third shape, before anything else in Step 4.**",
        )(s),
    },
    expect: true,
  },
  {
    // The completeness critic was the ONLY dispatch with neither a HEAD_SHA pin
    // nor an evidence requirement, and it is the one whose output can get a real
    // defect dismissed. It fabricated a code quote on 2026-07-25 and contradicted
    // a verified blocking finding with it. Both paths must impose both rules.
    name: "completeness critic: pinned to HEAD and must quote to contradict",
    extract: {
      workflow: (s) =>
        present("READ AT ${prContext.headRefOid} AND NOWHERE ELSE")(s) &&
        present(
          "IF YOU CONTRADICT A FINDING BELOW, QUOTE THE CODE THAT REFUTES IT",
        )(s),
      skill: (s) =>
        present("Pin the critic to `HEAD_SHA` and make it quote")(s),
    },
    expect: true,
  },
  {
    // The shared cache prefix (2026-07-25). Everything invariant across the
    // lenses of a run must sit ABOVE the "YOUR PASS" marker and the per-lens
    // focus BELOW it, or the 8 lenses of a run each write their own cache —
    // 10% of input volume at 57% of input cost. This is checked by POSITION
    // rather than by presence, because the failure mode is a block drifting back
    // above the marker where nothing would notice: the review still works, it
    // just silently costs more.
    name: "lens prompt: invariant blocks precede the per-lens focus",
    extract: {
      workflow: (text) => {
        const marker = text.indexOf("===== YOUR PASS =====");
        if (marker === -1) return null;
        const ctx = text.indexOf("${p.contextBlock}");
        const budget = text.indexOf("${LENS_BUDGET_INSTRUCTION}");
        const focus = text.indexOf("Restrict this pass to ONLY: ${p.focus}");
        if (ctx === -1 || budget === -1 || focus === -1) return null;
        return ctx < marker && budget < marker && focus > marker;
      },
      skill: present("invariant blocks first, the lens's own focus LAST"),
    },
    expect: true,
  },
  // The refuter family (2026-08-18) — same "===== YOUR CASE =====" marker,
  // same load-bearing reason as the lens rule above: this is the
  // highest-fan-out dispatch class in the harness (one call per candidate
  // finding, times every escalation/refine/debate round), so an unshared
  // prefix here costs more than anywhere else. The marker string is not
  // unique per site (it appears once per site, 5 times total), so each rule
  // below anchors on a static substring that IS unique to its own function to
  // locate that occurrence, then searches forward from there for the marker
  // and the dynamic content that must follow it — never a bare
  // `indexOf("===== YOUR CASE =====")`, which would only ever find the first
  // of the five and silently stop covering the other four.
  refuterFamilyOrderingRule(
    "claudeRefuter prompt: invariant blocks precede the per-finding case",
    "REFUTE — do not confirm — this code review finding on PR #${prNumber} in repo ${repo}.",
    ["${contextInstruction}${REFUTE_TAIL}"],
    ["Finding: ${finding.title}"],
  ),
  refuterFamilyOrderingRule(
    "claudeRefuterBatchPrompt: invariant blocks precede the batch's findings",
    "You are refuting ${findings.length} INDEPENDENT code-review findings",
    ["For EVERY case below, return one entry"],
    ["${cases}"],
  ),
  refuterFamilyOrderingRule(
    "strengthenPromptFor: invariant blocks precede the per-finding case",
    "A code-review finding on PR #${prNumber} (repo ${repo}) was nearly refuted",
    ['If you honestly cannot make it concrete, set'],
    ["Original finding: ${finding.title}"],
  ),
  refuterFamilyOrderingRule(
    "defense prompt: per-finding case follows the diff (already correct, pinned for uniformity)",
    "DEFEND this code-review finding on PR #${prNumber} (repo ${repo}).",
    [],
    ["Finding: ${workingFinding.title}"],
  ),
  refuterFamilyOrderingRule(
    "ruling prompt: the return-format instruction precedes the per-finding case",
    "You are the ADJUDICATOR (judge lens) on a peer-review panel.",
    [
      "Return decision SURVIVES or REFUTED, your confidence, a one-line rationale, and changedFromPanel = true iff your decision differs from the panel's mechanical outcome below.",
    ],
    ["Finding: ${workingFinding.title}"],
  ),
  // Import-adjacency heuristic (2026-08-18) — classifyPriorFindings escalates a
  // carried-forward finding into real verification when its unchanged file has
  // an import/require relationship with a file this PR DID change (PR #2012:
  // auth.controller.ts / accounts.ts). Its own ceiling, separate from
  // PRIOR_VERIFY_CEILING, must agree between both paths the same way T_KILL/
  // DELTA do below.
  {
    name: "IMPORT_ESCALATION_CEILING (import-heuristic verification cap)",
    extract: {
      workflow: num(/const IMPORT_ESCALATION_CEILING = (\d+);/),
      skill: num(/import-escalated population at (\d+) per run/),
    },
  },
  {
    name: "both paths state the import/require escalation rule",
    extract: {
      workflow: present("import/require relationship"),
      skill: present("import/require relationship"),
    },
    expect: true,
  },
  {
    // A run can be bounded three ways: the panel ceiling, the agent ceiling, and
    // running out of money mid-flight. The third is Workflow-only (the prose path
    // has no budget.spent()), so what both paths must agree on is that the key is
    // always WRITTEN — "no breaker on this path" and "the breaker never fired"
    // are different facts, and an omitted key conflates them.
    name: "cost breaker: the key is written by both paths",
    extract: {
      workflow: present("costBreaker: {"),
      skill: present("`costBreaker: null`"),
    },
    expect: true,
  },
  {
    // The two flags FINDINGS_SCHEMA already demands from every lens, and that
    // the row mapper silently dropped: `undefined` in 257 of 258 recorded
    // findings. Four mechanisms key on touchesHighRiskSurface (shouldEscalate,
    // the Codex slot's reasoning effort, whether a Gemini slot opens, the panel
    // budget's priority order), so an unaudited value has been driving real
    // decisions. Both paths must emit them, as booleans.
    name: "risk flags: persisted by both paths",
    extract: {
      workflow: (s) =>
        present("touchesHighRiskSurface: !!v.finding.touchesHighRiskSurface")(
          s,
        ) && present("isBehaviorChanging: !!v.finding.isBehaviorChanging")(s),
      skill: (s) =>
        present(
          "`touchesHighRiskSurface` and `isBehaviorChanging` as booleans",
        )(s),
    },
    expect: true,
  },
  {
    // A refine rewrites severity, so the final value cannot answer what the
    // panel was asked to adjudicate. Both ends or neither.
    name: "severityAtIntake: emitted by both paths",
    extract: {
      workflow: present(
        "severityAtIntake: v.severityAtIntake ?? v.finding.severity",
      ),
      skill: present("`severityAtIntake`"),
    },
    expect: true,
  },
  {
    // Both paths must narrow a re-review the same way, and both must carry the
    // prior findings forward. If only one side carried them, a PR reviewed
    // through the other path would report fewer problems than its first review
    // with no error anywhere — and the in-context path is where most PRs route.
    name: "incremental re-review: same scope script, and carry-forward on both paths",
    extract: {
      workflow: (s) =>
        present("scripts/incremental-scope.mjs")(s) &&
        present("Carried forward (not re-reviewed)")(s) &&
        present('reviewMode: incrementalScope ? "incremental" : "full"')(s),
      skill: (s) =>
        present("scripts/incremental-scope.mjs")(s) &&
        present("Carried forward (not re-reviewed)")(s),
      metrics: (s) =>
        present("`reviewMode`")(s) && present("`findingsCarriedForward`")(s),
    },
    expect: true,
  },
  {
    // The cheap rung. Both paths must know the flag exists and that its return
    // is a third valid shape — an orchestrator that does not recognise
    // `{preflight}` would fall into the "malformed return" branch and report a
    // successful dry run as a broken workflow.
    name: "--preflight: known to both paths, returns before any lens",
    extract: {
      workflow: (s) =>
        present("const preflight = parsedArgs.preflight === true;")(s) &&
        present("return { preflight: true, summary };")(s),
      skill: (s) =>
        present("`--preflight`")(s) &&
        present("{ preflight: true, summary:")(s),
    },
    expect: true,
  },
  {
    // A degraded run must be loud on both paths, not just recorded. Silence is
    // what made two plumbing bugs cost a launched review each.
    name: "incremental fallback is announced, not just logged",
    extract: {
      workflow: (s) =>
        present("INCREMENTAL RE-REVIEW EXPECTED BUT NOT APPLIED")(s) &&
        present("incrementalFallback,")(s),
      metrics: present("`incrementalFallback`"),
    },
    expect: true,
  },
  {
    // The Workflow is launched by its namespaced name, and the skill still has to
    // carry the one exception. Two facts, both measured, and the rule needs both:
    // a `name:` launch runs the installed snapshot, which is wrong when validating
    // an edit (2026-07-27, a line-1 marker absent from the persisted copy), and a
    // `scriptPath` into the plugin's own directory is refused outright by the tool
    // (2026-09-11, CLI 2.1.268). An instruction naming a path inside the plugin
    // would be dead on every machine, so assert the name form is present and that
    // no plugin-rooted scriptPath invocation came back.
    name: "the Workflow is launched by its namespaced name, with the edit-validation exception intact",
    extract: {
      skill: (s) =>
        present("Workflow({ name: 'tribunal:review-pr-workflow'")(s) &&
        !/scriptPath:\s*'\$\{CLAUDE_PLUGIN_ROOT\}/.test(s) &&
        present("tribunal-root")(s),
    },
    expect: true,
  },
  {
    // The Draft-comments strip is structural or it is nothing. The script's own
    // header says the prose instruction had been there since the skill's first
    // version and still leaked the section into a posted review on PR #1970 — so
    // a rule that merely checks the script EXISTS would repeat the mistake at one
    // remove: an unused script on disk is an unenforced instruction with extra
    // steps. This anchors on Step 4 actually piping the body through it, and on
    // the heredoc no longer asking a model to omit the section by hand.
    name: "the posted body is built by strip-draft-comments.mjs, not by hand",
    extract: {
      skill: (s) =>
        present("strip-draft-comments.mjs")(s) &&
        // As of the Step 4 consolidation, SKILL.md invokes post-review.mjs,
        // which imports stripDraftComments internally rather than SKILL.md
        // piping the report through the strip script directly — the
        // invariant (structural strip, no hand-typed omission) is unchanged,
        // only which script performs it.
        present(
          'node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --report-file "$REPORT_FILE"',
        )(s) &&
        // The old hand-omission heredoc must be gone, not merely supplemented.
        !/<the full Step 3 report, MINUS/.test(s),
    },
    expect: true,
  },
  {
    // A lens that found nothing must still appear in the row. The whole value of
    // the field is the zero-yield entry, so the rule anchors on the grouping and
    // on the in-context path declaring the key rather than omitting it.
    name: "lensYield reports every planned lens, including the ones that found nothing",
    extract: {
      workflow: (s) =>
        present("// ===== BEGIN LENS-YIELD =====")(s) &&
        present("lensYield: buildLensYield(")(s) &&
        present("const lensLabels = lensPlan.map(")(s),
      skill: present('"lensYield": null'),
      metrics: (s) =>
        present("`lensYield`")(s) && present("one entry per planned lens")(s),
    },
    expect: true,
  },
  {
    // The wall clock reads its start epoch from the trail, not from a prose
    // return, and an over-bound value is FLAGGED rather than nulled. Both halves
    // are load-bearing: the first is the 2026-07-27 defect, the second is the
    // correction to its first fix, which would have deleted the one correct 13h
    // measurement in the log. The in-context path must declare all three fields.
    name: "wall clock resolves from the inflight trail and keeps over-bound values",
    extract: {
      workflow: (s) =>
        present("# ===== BEGIN PY-WALLCLOCK =====")(s) &&
        present("'inflight-exact'")(s) &&
        present("elapsed, not active")(s) &&
        !/ACTIVE_RUN_BOUND:\s*\n?\s*seconds = None/.test(s),
      skill: (s) =>
        present('"wallClockNote": null')(s) &&
        present('"wallClockSource": null')(s),
      metrics: (s) =>
        present("`wallClockSource`")(s) && present("kept, never nulled")(s),
    },
    expect: true,
  },
  {
    // The verifier gets its own role, on both paths, and that role states the
    // cost asymmetry in the direction its job requires. A reviewer persona here
    // says "when unsure, call it fixed" — retiring a real defect on a model's
    // say-so, the most expensive output this harness can produce.
    name: "the prior-finding verifier has its own role, not the reviewer's",
    extract: {
      // Assert the PAIRING, not the presence. `present('agentType:
      // "finding-verifier"')` only proves the string exists somewhere in the
      // file, which stays green if a SECOND prior-finding dispatch is added
      // later carrying `code-review-expert` — the reviewer persona back on the
      // one job whose expensive error is "when unsure, call it fixed". So every
      // verify-prior dispatch is checked individually, and the absence of any
      // such dispatch is a failure too: a rule that passes when the thing it
      // guards is gone guards nothing.
      workflow: (s) => {
        const starts = [...s.matchAll(/label: [`"]verify-prior/g)].map(
          (m) => m.index,
        );
        if (!starts.length) return false;
        return starts.every((i) => {
          const next = s.slice(i + 8).search(/label: [`"]/);
          const win = s.slice(i, next === -1 ? i + 2000 : i + 8 + next);
          return (
            // The assignment, not the mention: this dispatch's own comment
            // explains why it is NOT the reviewer agent, and a checker that
            // fires on correct prose gets ignored.
            //
            // Matched through the constants (`VERIFIER_AGENT` / `LENS_AGENT`)
            // since the agent names gained the plugin's `tribunal:` namespace —
            // dispatching the bare name resolves to a personal ~/.claude/agents
            // copy where one exists, and fails outright where it does not.
            win.includes("agentType: VERIFIER_AGENT") &&
            !win.includes("agentType: LENS_AGENT")
          );
        });
      },
      skill: present("`tribunal:finding-verifier`"),
      verifier: (s) =>
        present("Your cost asymmetry is the opposite of a reviewer's")(s) &&
        present("tools: Read, Bash")(s),
    },
    expect: true,
  },
  {
    // The human's correction of the panel must outrank the panel on both paths.
    // Anchored on the two direction values, which are the load-bearing contract.
    name: "carry-forward honours operatorOverride over the panel's flags",
    extract: {
      // The projection that carries `operatorOverride` through moved out of
      // WORKFLOW's own text on 2026-09-10 (the PYPRIOR heredoc -> a real,
      // imported script) — checked against BOTH files, since the workflow
      // still owns the reinstated/withdrawn direction check and the script
      // now owns the projection.
      workflow: (s) =>
        present('if (direction === "reinstated") return true;')(s) &&
        present('direction === "withdrawn"')(s) &&
        present("operatorOverride: f.operatorOverride,")(src.resolveBaseline),
      skill: (s) =>
        present('`direction: "reinstated"`')(s) &&
        present('`direction: "withdrawn"`')(s),
      metrics: present("carry-forward reads `operatorOverride`"),
    },
    expect: true,
  },
  {
    // THE fork this rule exists to prevent, and it already happened once: the
    // scorer accepted `reinstated|rejected` while peer-review.js, both docs and
    // the PR #1968 row all used `dropped`. score.mjs reported that row INVALID
    // and never scored it, so the corpus silently ran on two operator labels
    // instead of three — the single scarcest input the harness has, since it is
    // the only ground truth that is not a model grading a model. Anchored on the
    // scorer's own list, which is what actually decides whether a label counts.
    name: "operatorOverride direction vocabulary agrees across code and both docs",
    extract: {
      overrideMjs: (text) => {
        const m = text.match(/OVERRIDE_DIRECTIONS = \[([^\]]+)\]/);
        if (!m) return null;
        return [...m[1].matchAll(/"([a-z]+)"/g)].map((x) => x[1]).sort();
      },
      // Every direction the scorer accepts must be honoured by the shipping
      // blocker rule — a direction the scorer scores but countsAsBlocker has
      // never heard of would silently keep a retracted finding blocking.
      workflow: (text) => {
        const m = text.match(/function countsAsBlocker\(f\)[\s\S]*?\n\}/);
        if (!m) return null;
        return ["reinstated", "rejected", "withdrawn"]
          .filter((d) => m[0].includes(`"${d}"`))
          .sort();
      },
      metrics: (text) =>
        ["reinstated", "rejected", "withdrawn"]
          .filter((d) => new RegExp('`"' + d + '"`').test(text))
          .sort(),
      skill: (text) =>
        ["reinstated", "rejected", "withdrawn"]
          .filter((d) => new RegExp('`direction: "' + d + '"`').test(text))
          .sort(),
    },
  },
  {
    // The legacy spelling must stay refused by the scorer. Accepting it as an
    // alias would leave two names for one event, which is the fork itself.
    name: "the legacy `dropped` spelling is not resurrected as a valid direction",
    extract: {
      overrideMjs: (text) =>
        !/OVERRIDE_DIRECTIONS = \[[^\]]*"dropped"/.test(text),
      metrics: present('**`\"dropped\"` is not valid**'),
    },
    expect: true,
  },
  {
    // fetch-pr-context.mjs splits bot comments from human ones, and the
    // PRIOR-REVIEWS region decides which open reviews are bots with its own
    // copy of the same rule. Two copies of a regex drift silently: a login one
    // side calls a bot would reach inThreadDecisions while the other side
    // ignored its review.
    name: "fetch-pr-context.mjs classifies bots with the workflow's BOT_LOGIN",
    extract: {
      workflow: (text) => {
        const m = text.match(/\nconst BOT_LOGIN =\s*(\/[^\n]+\/[a-z]*);/);
        return m ? m[1] : null;
      },
      fetchPr: (text) => {
        const m = text.match(/\nexport const BOT_LOGIN =\s*(\/[^\n]+\/[a-z]*);/);
        return m ? m[1] : null;
      },
    },
  },
  {
    // Both paths must fetch the PR's TOP-LEVEL comment thread, not only the
    // inline review comments. This is where an author answers a review, and the
    // Workflow was missing it: on 2026-07-27 the author posted a specific,
    // correct rebuttal to a blocking finding four hours before a run that never
    // fetched it, so the run re-reported the blocker and it was posted to GitHub
    // as still open. SKILL.md had the call all along — the drift was one-sided,
    // which is exactly what this checker exists to catch and did not, because no
    // invariant covered the fetch commands.
    //
    // Since v73 both paths fetch it through scripts/fetch-pr-context.mjs, so the
    // skill side anchors on the script invocation. The literal it used to anchor
    // on, `/issues/<n>/comments`, still appears in Step 4's continuity fetch,
    // which would have kept this rule green with Step 1's fetch deleted.
    name: "both paths fetch the PR comment thread, not just inline comments",
    extract: {
      workflow: present("/issues/${prNumber}/comments"),
      skill: present("skills/review-pr/scripts/fetch-pr-context.mjs --pr <n> --repo <REPO>"),
    },
    expect: true,
  },
  {
    // GitHub's comment endpoints return 30 items per page. An unpaginated fetch
    // drops the rest silently, and the newest comment, usually the author's
    // reply, is the one that falls off: BE #1911 has 59 inline comments and the
    // unpaginated call returned 30. Every comment fetch on both paths must
    // paginate.
    name: "every comment fetch paginates",
    extract: {
      workflow: (text) =>
        /"--paginate", "--slurp", `repos\/\$\{repo\}\/issues\/\$\{prNumber\}\/comments`/.test(text) &&
        /"--paginate", "--slurp", `repos\/\$\{repo\}\/pulls\/\$\{prNumber\}\/comments`/.test(text),
      skill: (text) => {
        const fetches = [...text.matchAll(/gh api ([^\n`]*?)repos\/<REPO>\/(?:issues|pulls)\/<n>\/comments/g)];
        return fetches.length > 0 && fetches.every((m) => m[1].includes("--paginate"));
      },
    },
    expect: true,
  },
  {
    // The question a re-review exists to answer must actually be ASKED on both
    // paths, and a fix claim must carry code. Reporting "not verified" instead
    // of verifying is what shipped on 2026-07-27.
    name: "prior findings are verified, and a fix claim needs a quote",
    extract: {
      workflow: (s) =>
        present("is it fixed at")(s) &&
        present("gradePriorVerification")(s) &&
        present(
          "without a word-for-word quote is discarded and recorded as CANNOT_TELL",
        )(s),
      skill: (s) =>
        present(
          "FIXED or STILL_OPEN verdict without a quote is discarded and recorded as CANNOT_TELL",
        )(s) && present("CANNOT_TELL")(s),
      metrics: present("`priorVerification`"),
    },
    expect: true,
  },
  {
    // A prior finding on a file this run RE-READ has to be accounted for on both
    // paths, and neither may call its absence a fix. The word list is the point:
    // "not verified as fixed" is the claim the report is allowed to make, and
    // the failure being prevented is a reader inferring a fix from silence.
    name: "re-read prior findings are accounted for, and never called fixed",
    extract: {
      workflow: (s) =>
        present("RE-CHECKED")(s) &&
        present("Prior findings on files this run DID re-read")(s) &&
        // CANNOT_TELL must never read as a fix, on either path. Anchored on the
        // word both sides use for it rather than on a sentence, so a rewording
        // does not trip the checker while a semantic change does.
        present("UNRESOLVED — could not be verified")(s) &&
        present("findingsNotResurfaced")(s),
      skill: (s) =>
        present("Prior findings on files this run DID re-read")(s) &&
        present("UNRESOLVED — could not be verified")(s),
      metrics: present("`findingsNotResurfaced`"),
    },
    expect: true,
  },
  {
    // Narrowing must never happen on a guess. Both paths state the same
    // fail-open rule, because the failure it prevents is silent on both.
    name: "incremental re-review: fails open to a full review",
    extract: {
      workflow: (s) =>
        present("FAIL OPEN, ALWAYS")(s) &&
        present("!forceFullReview && !pinnedSha")(s),
      skill: (s) => present("Fail open at every step")(s),
    },
    expect: true,
  },
  {
    // The start marker and the incremental probe ship as ONE dispatch. Both are
    // bash that runs right after fetch-pr and depends on nothing but headRefOid,
    // and a Workflow script has no shell — so each separate piece of bookkeeping
    // bash costs a full agent round-trip.
    //
    // What this pins is the part a future edit would silently undo: the marker's
    // epoch must reach the script as a SCHEMA'D field, not as prose to be
    // regexed. The regex is what shipped `wallClockSeconds: null` on three runs
    // whose `started` line had been written all along. Splitting the dispatch
    // again is allowed; going back to reading the epoch out of free text is not.
    name: "the run's start epoch is a schema'd field, not parsed prose",
    extract: {
      workflow: (s) =>
        present("const MARKER_BASH =")(s) &&
        present('startEpoch: { type: ["number", "null"] }')(s) &&
        !present("/epoch=(\\d+)/")(s),
      metrics: present("`wallClockSeconds`"),
    },
    expect: true,
  },
  {
    // Step 4 has no code-side counterpart — it runs the same way regardless of
    // which path Step 2 took ("posting always happens here, never inside the
    // Workflow script"), so this is a single-document invariant, not a
    // cross-implementation one, following the same `workflow: () => true`
    // precedent as "SKILL.md points the in-context path at its steps" above.
    // What it guards: the footer must be built by the script, for BOTH
    // invocation shapes, and appended by post-review.mjs itself — never pasted
    // into the report heredoc, where it would ride through the homosapiens pass
    // and the Draft-comments strip un-audited.
    name: "posted-comment footer: built by script from the row, appended after stripping",
    extract: {
      workflow: () => true,
      skill: (s) =>
        present("build-post-footer.mjs")(s) &&
        present("--mode workflow")(s) &&
        !present("--mode in-context")(s) &&
        present("--footer-file")(s),
    },
    expect: true,
  },
  {
    // The row has to say WHICH harness wrote it, on both paths and from the same
    // script. A version id only one path stamps splits the log into identifiable
    // and unidentifiable halves — the state that makes the pre-2026-07-27 rows
    // useless for any before/after question.
    name: "harnessVersion: stamped by both paths from the same script",
    extract: {
      workflow: (s) =>
        present("scripts/harness-version.mjs --short")(s) &&
        present("rec['harnessVersion']")(s),
      skill: present("scripts/harness-version.mjs --short"),
      metrics: present("`harnessVersion`"),
    },
    expect: true,
  },
  {
    // Which round cast a vote, and which vote the escalation gate added.
    // Position cannot substitute: tally.valid drops abstaining slots.
    name: "vote provenance: round and fromEscalation on both paths",
    extract: {
      workflow: (s) =>
        present("round: v.refined ? 1 : 0")(s) &&
        present("fromEscalation: !!vote.fromEscalation")(s),
      skill: (s) => present("`round` and `fromEscalation`")(s),
    },
    expect: true,
  },
  {
    // Both paths must carry the SAME budget number. A ceiling that differs
    // between the Workflow and the prose path would make lens cost — the single
    // largest term in a run — depend on which path a PR happened to route down.
    name: "lens tool-call budget agrees on both paths",
    extract: {
      workflow: num(/const LENS_TOOL_CALL_BUDGET = (\d+);/),
      skill: num(/tool-call ceiling of ~(\d+)/),
    },
  },
  {
    // The ceiling without the anti-floor clause is the dangerous half: lenses
    // under 30 calls returned 0.20 findings, so "aim under 40" read alone
    // produces a broken review, not a cheaper one.
    name: "the budget is stated as a ceiling, never a floor",
    extract: {
      workflow: present("not a reason to stop early"),
      skill: present('never "stop at 40"'),
    },
    expect: true,
  },
  {
    // Clean-verdict sweep (2026-08-14): both paths must carry the zero-panel
    // gate, and both must state the rule that makes it safe — an unreadable
    // sweep answer is a FAILED check, never a second-family confirmation.
    // Recording garbage as agreement would let infrastructure failure argue
    // for merging, the exact inversion the abstention rule prevents on panels.
    name: "clean-verdict sweep: gate exists and unreadable-is-not-agreement on both paths",
    extract: {
      workflow: (s) =>
        present("function shouldRunCleanSweep")(s) &&
        present("never as agreement")(s),
      skill: (s) =>
        present("Clean-verdict sweep")(s) && present("never as agreement")(s),
      heads: (s) =>
        present("Clean-verdict sweep")(s) && present("never as agreement")(s),
    },
    expect: true,
  },
  {
    name: "metrics schemaVersion",
    extract: {
      workflow: num(/schemaVersion: (\d+)/),
      metrics: maxNum(/## Fields added in schemaVersion (\d+)/g),
    },
  },
  {
    // The routing decision's size threshold: route-review.mjs's real,
    // importable constant, SKILL.md's two restatements (Dispatch decision and
    // the Tier-1 vehicle note), and the plugin README's own copy.
    name: "route-review WORKFLOW_MIN_FILES",
    extract: {
      routeScript: num(/const WORKFLOW_MIN_FILES = (\d+)/),
      skill: uniform(/≥ (\d+) files/g),
    },
  },
  {
    // The routing decision's high-risk scope threshold, same three surfaces.
    name: "route-review HIGH_RISK_SCOPE_MAX",
    extract: {
      routeScript: num(/const HIGH_RISK_SCOPE_MAX = (\d+)/),
      skill: uniform(/scope > (\d+)/g),
    },
  },
  {
    // Wiring: the script must actually be reachable from SKILL.md's dispatch
    // pipeline, and the exclusion list it depends on must be named on both
    // sides — a stale prose description of routing that never got wired to
    // this script is exactly the failure ROUTE-REVIEW replaces.
    name: "route-review script and PRODUCT_FILE_EXCLUSIONS wired into SKILL.md",
    extract: {
      skill: (s) =>
        present("scripts/route-review.mjs")(s) &&
        present("PRODUCT_FILE_EXCLUSIONS")(s),
      routeScript: present("PRODUCT_FILE_EXCLUSIONS"),
    },
    expect: true,
  },
  {
    // The content scan degrades to nothing if SKILL.md stops passing the diff:
    // contentRisk is null on every run forever and routing goes back to what
    // it was before the scan existed, with nothing anywhere to notice. Same
    // instructed-not-enforced shape the scan was added to close, so it gets
    // the same treatment as the two thresholds above.
    name: "route-review content scan is actually wired: SKILL.md passes --diff",
    extract: {
      skill: (s) =>
        /route-review\.mjs[\s\S]{0,200}--diff\s+\S+/.test(s) &&
        present("contentRisk")(s),
      routeScript: (s) => present("MASS_OP_RE")(s) && present("--diff")(s),
    },
    expect: true,
  },
  {
    // The ops the regex looks for, named identically in the code and in the
    // prose that tells an operator what the scan covers. A prose list that
    // drifts from the regex is a promise the script does not keep.
    name: "route-review MASS_OP_RE and SKILL.md name the same operations",
    extract: {
      skill: (s) =>
        ["deleteMany", "updateMany", "DELETE FROM", "TRUNCATE", "DROP TABLE", "$executeRaw"]
          .every((op) => s.includes(op)),
      routeScript: (s) =>
        ["deleteMany", "updateMany", "DELETE", "TRUNCATE", "DROP", "executeRaw"]
          .every((op) => s.includes(op)),
    },
    expect: true,
  },
  {
    // The Dispatch fetch and in-context Step 2 have to name the SAME file, or
    // "the same fetch moved earlier" is false and Step 2 quietly fetches the
    // diff a second time. `${TMPDIR:-/tmp}` is load-bearing: on macOS TMPDIR
    // is not /tmp, so a bare /tmp here and a ${TMPDIR:-/tmp} there are two
    // different paths that look identical while reading.
    name: "the routing patch and Step 2's patch are the same path",
    extract: {
      skill: (s) => {
        const uses = s.match(/\S*peer-review-diff-<n>\.patch/g) || [];
        return (
          uses.length >= 2 &&
          uses.every((u) => u.includes("${TMPDIR:-/tmp}/peer-review-diff"))
        );
      },
    },
    expect: true,
  },
  {
    // The whole reason contentRisk is a tri-state. If the prose stops saying
    // it, a reader treats null as false and reports an unread diff as a clean
    // PR, which is what a truncated fetch cost a full review on PR #1911.
    name: "contentRisk null means nobody looked, said in SKILL.md and enforced in the script",
    extract: {
      skill: (s) => /contentRisk: null[\s\S]{0,120}nobody looked/.test(s),
      routeScript: (s) => present("contentRisk = looked ?")(s),
    },
    expect: true,
  },
  {
    // A pinned compare needs the PR's base COMMIT. The branch name works until
    // the PR merges; after that compare/<branch>...<sha> is empty, because the
    // branch already contains the sha (BE #2167, 2026-10-19, routed on a 0-byte
    // patch). The Workflow and fetch-pr-context.mjs already used baseRefOid.
    name: "a pinned compare is built on the PR's baseRefOid, in the prose and in the Workflow",
    extract: {
      skill: (s) =>
        /--json baseRefOid --jq \.baseRefOid\)[\s\S]{0,200}compare\/\$BASE\.\.\.\$PIN/.test(s) && !s.includes("compare/<BASE>..."),
      workflow: (s) => present("compare/${view.baseRefOid}...${headSha}")(s) && present("--json baseRefOid --jq .baseRefOid")(s),
    },
    expect: true,
  },
  {
    // The reviewer agent reads its methodology from the first line of its
    // prompt. A path that differs between the two paths, or a dispatch that
    // drops the line, sends a reviewer to the plugin's generic methodology with
    // none of the repo's own rules, and nothing else would notice.
    name: "every reviewer dispatch names the assembled methodology, at the same path, in the prose and in the Workflow",
    extract: {
      skill: (s) =>
        s.includes("assemble-methodology.mjs --kind <profile.reviewer>") &&
        s.includes("--out /tmp/tribunal-methodology-<owner>-<name>-<n>.md") &&
        s.includes("**Every `<LENS_AGENT>` dispatch starts with one line, then a blank line:** `METHODOLOGY FILE: /tmp/tribunal-methodology-<owner>-<name>-<n>.md`"),
      workflow: (s) =>
        present("const METHODOLOGY_PATH = `/tmp/tribunal-methodology-${repo.replace(/[^A-Za-z0-9]+/g, \"-\")}-${prNumber}.md`;")(s) &&
        present("const METHODOLOGY_LINE = `METHODOLOGY FILE: ${METHODOLOGY_PATH}`;")(s) &&
        reviewerDispatchesAllCarryTheLine(s),
    },
    expect: true,
  },
  {
    // The repo's own CLAUDE.md and scoped rules reach a subagent only through
    // this line: a session started outside the repo loads neither, and a
    // scoped rule never fires for `git show`. A refuter dispatch without it
    // judges a finding against policy it cannot see.
    name: "every reviewer and Claude refuter dispatch carries the conventions line, in the prose and in the Workflow",
    extract: {
      skill: (s) =>
        s.includes("load-conventions.mjs --profile") &&
        s.includes("--out /tmp/tribunal-conventions-<owner>-<name>-<n>.md") &&
        s.includes("conventionsPath: <load-conventions' out, only when it ran>") &&
        s.includes("**When SKILL.md loaded the repo's conventions, one more line follows it:** `CONVENTIONS FILE: /tmp/tribunal-conventions-<owner>-<name>-<n>.md`") &&
        s.includes("Every Claude refuter dispatch (`tribunal:tribunal-refuter`, single or batched) starts with the same line"),
      workflow: (s) =>
        present("const reviewerAgent = (prompt, opts) => agent(CONVENTIONS.reviewerPrompt(METHODOLOGY_LINE, prompt), { ...opts, agentType: LENS_AGENT });")(s) &&
        present("const FETCH_PR_BASH = `${ASSEMBLE_BASH}${CONVENTIONS.checkBash}")(s) &&
        refuterDispatchesAllCarryConventions(s),
    },
    expect: true,
  },
];

// Every Claude refuter dispatch (`agentType: refuterAgentType`) must hand
// agent() a prompt wrapped in withConventions().
function refuterDispatchesAllCarryConventions(src) {
  const lines = src.split("\n");
  let sites = 0;
  for (let i = 0; i < lines.length; i++) {
    if (!lines[i].includes("agentType: refuterAgentType")) continue;
    let j = i;
    while (j >= 0 && !/(?<![A-Za-z])agent\(/.test(lines[j])) j--;
    if (j < 0) return false;
    const after = `${lines[j].slice(lines[j].search(/(?<![A-Za-z])agent\(/) + 6)}\n${lines[j + 1] || ""}`.trimStart();
    if (!after.startsWith("withConventions(")) return false;
    sites++;
  }
  return sites >= 2;
}

// Every `agentType: LENS_AGENT` outside the helper itself must sit in a
// `reviewerAgent(` call, the only dispatch that prepends METHODOLOGY_LINE.
function reviewerDispatchesAllCarryTheLine(src) {
  const lines = src.split("\n");
  const helper = lines.findIndex((l) => l.startsWith("const reviewerAgent ="));
  if (helper < 0 || !lines[helper].includes("METHODOLOGY_LINE")) return false;
  let sites = 0;
  for (let i = 0; i < lines.length; i++) {
    if (i === helper || !lines[i].includes("agentType: LENS_AGENT")) continue;
    let j = i;
    while (j >= 0 && !/(?<![A-Za-z])(agent|reviewerAgent|relayAgent)\(/.test(lines[j])) j--;
    if (j < 0 || !/(?<![A-Za-z])reviewerAgent\(/.test(lines[j])) return false;
    sites++;
  }
  // 5 since v101, when the Workflow stopped dispatching the fidelity critic.
  return sites >= 5;
}

// --- run --------------------------------------------------------------------
const failures = [];
let checked = 0;

// Rules whose `skill` side described the in-context path (SKILL.md and
// references/in-context-review.md read as one text). That path is gone since
// v101, so for these the `skill` side is retired; every other rule still reads
// SKILL.md. A frozen list, not a skip-on-failure: a rule outside it whose
// SKILL.md side stops matching still fails.
const IN_CONTEXT_SIDE = new Set([
  "T_KILL threshold",
  "DELTA (borderline band)",
  "GEMINI_CONFIDENCE_CAP",
  "verifiedBy MIN_QUOTE_CHARS",
  "verifiedBy field present on every path",
  "failureReason classes",
  "sticky-routing trigger set",
  "verdict rule: nits alone never withhold approval",
  "debate dispatches (defense, judge) use the repo's lens agent on both paths",
  "dedup: cross-file merges are forbidden",
  "dedup: within-file proximity window",
  "dedup: title-overlap threshold",
  "panel membership: vetted bot findings",
  "panel membership: RETEST-named claims",
  "nothing unadjudicated is reported as Blocking",
  "self-audit (audit-run.mjs) runs on both paths",
  "the report the critic judged is persisted on both paths",
  "dependents block: rendered on both paths from DEPENDENTS-BLOCK",
  "panel intake: every non-nit earns a panel",
  "open blockers are repeats, not pre-decided",
  "lensAgent: emitted by both paths",
  "completeness critic: pinned to HEAD and must quote to contradict",
  "lens prompt: invariant blocks precede the per-lens focus",
  "IMPORT_ESCALATION_CEILING (import-heuristic verification cap)",
  "both paths state the import/require escalation rule",
  "cost breaker: the key is written by both paths",
  "risk flags: persisted by both paths",
  "severityAtIntake: emitted by both paths",
  "incremental re-review: same scope script, and carry-forward on both paths",
  "lensYield reports every planned lens, including the ones that found nothing",
  "wall clock resolves from the inflight trail and keeps over-bound values",
  "the prior-finding verifier has its own role, not the reviewer's",
  "carry-forward honours operatorOverride over the panel's flags",
  "operatorOverride direction vocabulary agrees across code and both docs",
  "both paths fetch the PR comment thread, not just inline comments",
  "prior findings are verified, and a fix claim needs a quote",
  "re-read prior findings are accounted for, and never called fixed",
  "incremental re-review: fails open to a full review",
  "harnessVersion: stamped by both paths from the same script",
  "vote provenance: round and fromEscalation on both paths",
  "lens tool-call budget agrees on both paths",
  "the budget is stated as a ceiling, never a floor",
  "clean-verdict sweep: gate exists and unreadable-is-not-agreement on both paths",
  "route-review WORKFLOW_MIN_FILES",
  "route-review HIGH_RISK_SCOPE_MAX",
  "route-review script and PRODUCT_FILE_EXCLUSIONS wired into SKILL.md",
  "route-review content scan is actually wired: SKILL.md passes --diff",
  "route-review MASS_OP_RE and SKILL.md name the same operations",
  "the routing patch and Step 2's patch are the same path",
  "contentRisk null means nobody looked, said in SKILL.md and enforced in the script",
  "every reviewer dispatch names the assembled methodology, at the same path, in the prose and in the Workflow",
  "every reviewer and Claude refuter dispatch carries the conventions line, in the prose and in the Workflow",
]);
// The prose sources of the in-context path, gone in v101.
const RETIRED = new Set(["inContext", "heads"]);
const skipped = [];
for (const rule of RULES) {
  const live = Object.entries(rule.extract).filter(
    ([file]) => !RETIRED.has(file) && !(file === "skill" && IN_CONTEXT_SIDE.has(rule.name)),
  );
  // `workflow: () => true` stood in for "nothing to check on this side" while
  // the prose side carried the claim; alone it would pass on nothing.
  const placeholder = ([, fn]) => /^\(\)\s*=>\s*true$/.test(String(fn).trim());
  if (!live.length || live.every(placeholder) || (!("expect" in rule) && live.length < 2)) {
    skipped.push(rule.name);
    continue;
  }
  const values = {};
  for (const [file, fn] of live) {
    let v = fn(src[file]);
    if (rule.normalise && rule.normalise[file])
      v = rule.normalise[file](v, src[file]);
    values[file] = v;
  }
  checked++;

  const missing = Object.entries(values).filter(
    ([, v]) => v === null || v === undefined,
  );
  if (missing.length) {
    failures.push(
      `${rule.name}: could not extract from ${missing.map(([f]) => f).join(", ")} — the rule no longer matches the file, which is itself drift`,
    );
    continue;
  }

  if ("expect" in rule) {
    const wrong = Object.entries(values).filter(([, v]) => !eq(v, rule.expect));
    if (wrong.length)
      failures.push(
        `${rule.name}: ${wrong.map(([f, v]) => `${f}=${JSON.stringify(v)}`).join(", ")} (expected ${JSON.stringify(rule.expect)})`,
      );
  } else {
    const entries = Object.entries(values);
    const [, first] = entries[0];
    const disagree = entries.filter(([, v]) => !eq(v, first));
    if (disagree.length)
      failures.push(
        `${rule.name}: ${entries.map(([f, v]) => `${f}=${JSON.stringify(v)}`).join("  ")}`,
      );
  }

  if (verbose)
    console.log(
      `  ${failures.length && failures[failures.length - 1].startsWith(rule.name) ? "FAIL" : "ok  "} ${rule.name.padEnd(44)} ${JSON.stringify(Object.values(values)[0])}`,
    );
}

console.log(
  `\nmirror invariants: ${checked - failures.length}/${checked} agree; ${skipped.length} rule(s) skipped, their other side was the in-context prose removed in v101`,
);
if (verbose) for (const n of skipped) console.log(`  skip ${n}`);
if (failures.length) {
  console.log("\ndrift:");
  for (const f of failures) console.log(`  ${f}`);
  console.log(
    "\nFix both copies: the Workflow cannot import, so each value above lives in it and in the file named beside it.",
  );
  process.exit(1);
}
console.log(
  "the Workflow and its copies agree on every checked value",
);
