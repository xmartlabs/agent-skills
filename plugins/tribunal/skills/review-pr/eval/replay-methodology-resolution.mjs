#!/usr/bin/env node
// replay-methodology-resolution.mjs — the two lens agent wrappers
// (agents/fe-code-review-expert.md, agents/code-review-expert.md) resolve
// their methodology file deterministically, and treat reading it as a hard
// precondition rather than optional context-gathering.
//
// The defect. Both wrappers read the file at runtime rather than embedding
// it, on purpose (so an edit takes effect without needing the cached agent
// definition to change). Their fallback path, when `$CLAUDE_PLUGIN_ROOT` is
// empty, used to Glob `**/plugins/tribunal/methodology/...` from the
// repository root with no way to choose between several coexisting local
// checkouts. Live-verifying tribunal v57 on FE PR #1553 (2026-09-09), all
// five dispatched lens agents got an empty `$CLAUDE_PLUGIN_ROOT` (not
// populated for agents dispatched inside a Workflow run) and all five landed
// on the SAME stale checkout: a `documentation` clone sitting on an unrelated
// branch, frozen at plugin version 48.0.0 — nine releases behind what was
// actually installed. The fix (v58) replaces the glob with a deterministic
// scan of the installed plugin cache, sorted by version.
//
// The SECOND thing that same live run showed: even after the v58 fix, two of
// the five lens agents never called Read on the methodology file at all —
// they answered from generic familiarity with "a frontend code reviewer"
// instead. That is a model instruction-following gap this file cannot make
// airtight (no code here executes at all if the model just skips reading its
// own instructions), but the wrapper text can at least make skipping it
// costly rather than free — this replay pins that the stronger wording is
// present and has not quietly regressed back to something softer.
//
// This is prose, not code inside a marked pure region, so unlike most of this
// battery it cannot execute the real resolution logic — Tier 0 cannot reach
// what a model does with a paragraph. What it CAN do, and does: assert the
// deterministic-resolution shell command and the hard-precondition paragraph
// are actually present, word for word, in both wrapper files, so a future
// edit that softens or drops either one fails loudly here instead of being
// discovered the next time a live run reproduces PR #1553.
//
// Usage: node eval/replay-methodology-resolution.mjs

import { readFileSync } from "node:fs";
import { agent } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const FE_WRAPPER = agent("fe-code-review-expert.md");
const BE_WRAPPER = agent("code-review-expert.md");
const fe = readFileSync(FE_WRAPPER, "utf8");
const be = readFileSync(BE_WRAPPER, "utf8");

console.log(`\nmethodology resolution — ${FE_WRAPPER}, ${BE_WRAPPER}`);

console.log(`\n--- deterministic resolution against the installed plugin cache (v58) ---`);
{
  const scanCmd =
    'for d in ~/.claude/plugins/cache/*/tribunal/*/; do [ -d "$d" ] && echo "$(basename "$d") $d"; done | sort -V | tail -1 | cut -d\' \' -f2-';
  for (const [label, src] of [["FE", fe], ["BE", be]]) {
    check(`${label} wrapper still tries \$CLAUDE_PLUGIN_ROOT first`, /echo "\$CLAUDE_PLUGIN_ROOT"/.test(src));
    check(`${label} wrapper's fallback scans the installed cache, not a bare repo-root glob`, src.includes(scanCmd));
    check(`${label} wrapper sorts by version rather than trusting glob/mtime order`, /sort -V \| tail -1/.test(src));
    check(
      `${label} wrapper only falls back to a repository-root glob as a last resort, and says so if it does`,
      /Only if step 2 finds nothing at all[\s\S]{0,400}say so plainly in your Summary/.test(src),
    );
  }
}

console.log(`\n--- reading the methodology is a hard precondition, not optional context (PR #1553) ---`);
{
  const hardPrecondition =
    "**This Read call is a hard precondition, not optional context-gathering.**";
  for (const [label, src] of [["FE", fe], ["BE", be]]) {
    check(`${label} wrapper states the Read is a hard precondition`, src.includes(hardPrecondition));
    check(
      `${label} wrapper forbids acting before the Read completes`,
      /Do not read the diff, do not form an opinion about the code, and do not call `StructuredOutput` until it has actually completed/.test(
        src,
      ),
    );
    check(
      `${label} wrapper names the actual failure mode — reviewing from familiarity instead of this run's real procedure`,
      /is not a substitute/.test(src),
    );
  }
}

console.log(`\n--- what this replay cannot check ---`);
{
  // Documented rather than silently absent: a green run here says the WORDING
  // held, never that a model actually obeyed it. Live PR #1553 verification
  // showed 3 of 5 lens agents reading the file correctly and 2 of 5 skipping
  // the Read call entirely despite the (then-unstrengthened) instruction —
  // this file has no way to reproduce or gate on that model behavior offline.
  check(
    "this file makes no claim about live compliance — only that a regression to weaker wording would be caught",
    true,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall methodology-resolution wrapper expectations hold");
