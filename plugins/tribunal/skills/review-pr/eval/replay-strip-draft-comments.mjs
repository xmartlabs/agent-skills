#!/usr/bin/env node
// replay-strip-draft-comments.mjs — regression test for
// scripts/strip-draft-comments.mjs (see PR #1970: the section leaked into a
// posted review because a model retyped the report by hand instead of the
// script doing the removal). Imports the real module directly — same
// reasoning as replay-evidence-resolver.mjs, since it's a plain script with
// no injected-runtime globals to mirror.
//
// Usage: node eval/replay-strip-draft-comments.mjs

import { stripDraftComments } from "../scripts/strip-draft-comments.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// Section at the end of the report (the common case — it's always last).
{
  const input = [
    "## Peer review: PR #1 — title",
    "",
    "### Verdict suggestion: COMMENT",
    "",
    "### Non-blocking / nits",
    "- some finding",
    "",
    "### Draft comments (ready to paste)",
    "1. comment one",
    "2. comment two",
  ].join("\n");
  const out = stripDraftComments(input);
  check("drops section at end of doc", !out.includes("Draft comments"));
  check("keeps preceding sections", out.includes("### Non-blocking / nits") && out.includes("some finding"));
}

// Section in the middle, followed by another heading — must not eat it.
{
  const input = [
    "## Peer review: PR #2 — title",
    "",
    "### Draft comments (ready to paste)",
    "1. comment one",
    "",
    "### Bot comments vetted",
    "- [VALID] something",
  ].join("\n");
  const out = stripDraftComments(input);
  check("drops section mid-doc", !out.includes("Draft comments") && !out.includes("comment one"));
  check("keeps the section after it", out.includes("### Bot comments vetted") && out.includes("[VALID] something"));
}

// No section present — must be a no-op (idempotent).
{
  const input = [
    "## Peer review: PR #3 — title",
    "",
    "### Verdict suggestion: APPROVE",
    "",
    "### Coverage",
    "all good",
  ].join("\n");
  check("idempotent when section absent", stripDraftComments(input) === input);
}

console.log(`\n${failures.length === 0 ? "PASS" : "FAIL"}: ${failures.length} failure(s)`);
process.exit(failures.length === 0 ? 0 : 1);
