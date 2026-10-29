#!/usr/bin/env node
// replay-prior-reviews.mjs — which prior reviews still count as open, and
// whether posting would file a duplicate.
//
// Why this needs its own replay. The failure it guards was silent in both
// directions. On the 2026-07-24 PR #1935 run an open CHANGES_REQUESTED review
// by the same operator, at the same SHA, matched none of the four decision
// categories the thread harvest recognises (DEFERRED / RESOLVED / BY-DESIGN /
// SPLIT-OUT), reached no prompt, and two lenses re-raised it as a fresh
// blocker — one confirmation away from posting a second, weaker copy of a
// review that already existed. The opposite mistake is worse: route an open
// blocker into the "do NOT re-litigate" block and a refuter kills it as
// PRE_DECIDED, so the report calls the PR clean while the bug stands.
//
// So the gate has to be exact about a small number of things — state equality,
// one supersede comparison per author, and who "we" are — and none of those is
// a judgement call. They live in code, and this runs that code: the region is
// sliced out of workflows/peer-review.js by its markers, never copied.
//
// Usage: node eval/replay-prior-reviews.mjs

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

const { api, violations } = loadRegion(WORKFLOW, "PRIOR-REVIEWS", [
  "openChangeRequests",
  "ownStandingReview",
  "postGate",
]);
const { openChangeRequests, ownStandingReview, postGate } = api;

console.log(`\nPRIOR-REVIEWS region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const rv = (author, state, at, summary = "") => ({ author, state, at, summary });

console.log(`\n--- which reviews are open ---`);
{
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "Zod schema missing"),
    rv("someone", "APPROVED", "2026-07-22T10:00:00Z"),
    rv("coderabbitai", "CHANGES_REQUESTED", "2026-07-21T11:00:00Z", "nit"),
  ]);
  check("a human CHANGES_REQUESTED is open", open.length === 1 && open[0].author === "alice");
  check("an APPROVED review is not open", !open.some((r) => r.author === "someone"));
  check("a bot's CHANGES_REQUESTED is ignored", !open.some((r) => r.author === "coderabbitai"));
  check("summary text survives verbatim", open[0].summary === "Zod schema missing");
}
{
  for (const bot of ["github-actions", "dependabot", "renovate", "some-app[bot]", "CodeRabbitAI"]) {
    check(
      `bot login rejected: ${bot}`,
      openChangeRequests([rv(bot, "CHANGES_REQUESTED", "2026-07-21T10:00:00Z")]).length === 0,
    );
  }
  check(
    "a human whose name merely contains 'bot' is NOT rejected",
    openChangeRequests([rv("robotnik", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z")]).length === 1,
  );
}
{
  check(
    "state matching is case-insensitive",
    openChangeRequests([rv("a", "changes_requested", "2026-07-21T10:00:00Z")]).length === 1,
  );
  check("an empty author is dropped", openChangeRequests([rv("  ", "CHANGES_REQUESTED", "x")]).length === 0);
  check("a null list does not throw", openChangeRequests(null).length === 0);
  check("undefined does not throw", openChangeRequests(undefined).length === 0);
  check("a null entry does not throw", openChangeRequests([null, undefined]).length === 0);
  check(
    "a missing timestamp becomes null, not an empty string",
    openChangeRequests([rv("a", "CHANGES_REQUESTED", "")])[0].at === null,
  );
}

console.log(`\n--- supersede: a later verdict by the same author closes it ---`);
{
  // The case that matters most: the reviewer came back and approved. Treating
  // their old CHANGES_REQUESTED as live would block every subsequent post and
  // annotate every finding as a repeat of an objection that was withdrawn.
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
    rv("alice", "APPROVED", "2026-07-23T10:00:00Z"),
  ]);
  check("a later APPROVED by the same author supersedes it", open.length === 0);
}
{
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
    rv("alice", "COMMENTED", "2026-07-23T10:00:00Z"),
  ]);
  check("a later COMMENTED by the same author supersedes it too", open.length === 0);
}
{
  const open = openChangeRequests([
    rv("alice", "APPROVED", "2026-07-19T10:00:00Z"),
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
  ]);
  check("an EARLIER approval does not supersede a later block", open.length === 1);
}
{
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
    rv("otro", "APPROVED", "2026-07-23T10:00:00Z"),
  ]);
  check("another author's approval does not supersede mine", open.length === 1);
}
{
  // Author casing differs between GitHub surfaces; a supersede that misses on
  // case would leave a withdrawn objection looking live.
  const open = openChangeRequests([
    rv("Alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
    rv("alice", "APPROVED", "2026-07-23T10:00:00Z"),
  ]);
  check("supersede matches the author case-insensitively", open.length === 0);
}
{
  // An unknown time is not a later time. Superseding on a blank timestamp would
  // silently retire a live blocker — the direction that lets a real bug through.
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x"),
    rv("alice", "APPROVED", ""),
  ]);
  check("an undated approval cannot supersede a dated block", open.length === 1);
  const open2 = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "", "x"),
    rv("alice", "APPROVED", "2026-07-23T10:00:00Z"),
  ]);
  check("an undated block is not superseded by a dated approval either", open2.length === 1);
}
{
  const open = openChangeRequests([
    rv("a", "CHANGES_REQUESTED", "2026-07-20T10:00:00Z", "first"),
    rv("a", "CHANGES_REQUESTED", "2026-07-22T10:00:00Z", "second"),
  ]);
  check("two blocks by the same author both stay open", open.length === 2);
}

console.log(`\n--- post gate ---`);
{
  const g = postGate([], "alice");
  check("no open reviews → clear", g.status === "clear" && g.ownOpenReview === null);
}
{
  const open = openChangeRequests([rv("otro", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x")]);
  const g = postGate(open, "alice");
  check("someone else's open block → clear (not my duplicate)", g.status === "clear");
}
{
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "Zod schema"),
  ]);
  const g = postGate(open, "alice");
  check("my own open block → own-open-review", g.status === "own-open-review");
  check("the gate hands back the review it matched", g.ownOpenReview?.at === "2026-07-21T10:00:00Z");
}
{
  const open = openChangeRequests([rv("Alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x")]);
  check("viewer matching is case-insensitive", postGate(open, "alice").status === "own-open-review");
}
{
  // `gh api user` failing must not read as "clear". Fail-open here is exactly
  // the duplicate the gate exists to stop, so an unknown viewer degrades to
  // asking — and ONLY when something is actually open.
  const open = openChangeRequests([rv("otro", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z", "x")]);
  check("unknown viewer with open reviews → unknown-viewer", postGate(open, "").status === "unknown-viewer");
  check("unknown viewer, nothing open → still clear", postGate([], "").status === "clear");
  check("null viewer behaves like an empty one", postGate(open, null).status === "unknown-viewer");
}

console.log(`\n--- the PR #1935 shape that motivated this ---`);
{
  // alice filed CHANGES_REQUESTED on 2026-07-21; the run on 2026-07-24 hit
  // the same SHA, re-raised the same finding twice, and offered to post.
  const open = openChangeRequests([
    rv("alice", "CHANGES_REQUESTED", "2026-07-21T16:04:11Z", "partner serializer: Zod schema not applied"),
  ]);
  const g = postGate(open, "alice");
  check("the run is recognised as a re-review of my own open block", g.status === "own-open-review");
  check("the date reaches the report annotation", open[0].at.slice(0, 10) === "2026-07-21");
  check("the prior review's own wording is preserved for matching", /Zod schema/.test(open[0].summary));
}

console.log(`\n--- own standing review: a duplicate that is not an objection ---`);
{
  // openChangeRequests answers "what objections are live", so an APPROVED
  // review is correctly absent from it. The gate needs the wider question,
  // because a second review stacked on our own approval is still a duplicate.
  const all = [
    rv("alice", "APPROVED", "2026-08-19T19:20:15Z"),
    rv("alice", "APPROVED", "2026-08-19T19:20:33Z"),
    rv("bob", "APPROVED", "2026-08-19T16:47:37Z"),
  ];
  check("no objections are open", openChangeRequests(all).length === 0);
  const g = postGate(openChangeRequests(all), "alice", all);
  check("my own standing approval → own-prior-review", g.status === "own-prior-review");
  check("the gate hands back the LATEST of mine", g.ownStandingReview?.at === "2026-08-19T19:20:33Z");
  check("ownOpenReview stays null (nothing is blocked)", g.ownOpenReview === null);
  check("someone else's approval alone → clear", postGate([], "nadie", all).status === "clear");
}
{
  const all = [rv("alice", "COMMENTED", "2026-08-19T10:00:00Z")];
  check("a standing COMMENTED of mine also counts", postGate([], "alice", all).status === "own-prior-review");
}
{
  const all = [rv("alice", "DISMISSED", "2026-08-19T10:00:00Z")];
  check("a DISMISSED review of mine does not stand", postGate([], "alice", all).status === "clear");
}
{
  const all = [rv("alice", "APPROVED", "")];
  check("an undated review of mine cannot be shown to stand", postGate([], "alice", all).status === "clear");
}
{
  const all = [rv("Alice", "APPROVED", "2026-08-19T10:00:00Z")];
  check("viewer matching stays case-insensitive here too", postGate([], "alice", all).status === "own-prior-review");
}
{
  // An open block outranks a standing approval: when both apply, the more
  // urgent thing to name is the objection.
  const all = [
    rv("alice", "APPROVED", "2026-08-18T10:00:00Z"),
    rv("alice", "CHANGES_REQUESTED", "2026-08-19T10:00:00Z", "x"),
  ];
  const g = postGate(openChangeRequests(all), "alice", all);
  check("an open block wins over a standing approval", g.status === "own-open-review");
}
{
  const all = [rv("otro", "APPROVED", "2026-08-19T10:00:00Z")];
  check("reviews exist but no viewer → unknown-viewer", postGate([], "", all).status === "unknown-viewer");
  check("no reviews at all and no viewer → still clear", postGate([], "", []).status === "clear");
}
{
  check("omitting allReviews keeps the old two-arg behaviour", postGate([], "alice").status === "clear");
  check("ownStandingReview tolerates junk", ownStandingReview(null, "x") === null && ownStandingReview([null], "x") === null);
}

console.log(`\n--- the PR #2039 shape that motivated own-prior-review ---`);
{
  // 2026-08-19: two standing APPROVED reviews by the operator at the head
  // commit. count was 0, reviewers was empty, and the gate said clear.
  const all = [
    rv("bob", "APPROVED", "2026-08-19T16:47:37Z"),
    rv("alice", "APPROVED", "2026-08-19T19:20:15Z"),
    rv("alice", "APPROVED", "2026-08-19T19:20:33Z"),
  ];
  const open = openChangeRequests(all);
  const g = postGate(open, "alice", all);
  check("count is still 0 (these are not objections)", open.length === 0);
  check("but the gate no longer says clear", g.status !== "clear");
  check("it names the review a post would duplicate", g.ownStandingReview?.state === "APPROVED");
}

console.log(`
What this does NOT establish:
  · Whether the synthesizer actually MATCHES a finding to a prior review item.
    That match is prose-to-prose and deliberately model-side; this file only
    guarantees the prior items reach it with author, date and wording intact.
  · That Step 4 obeys the gate. The gate value is computed here and recorded in
    metrics.priorOpenReviews.gate; honouring it is SKILL.md Step 4, i.e. Tier 1
    at best — read the gate field on a run against a PR that already has an
    open CHANGES_REQUESTED.
  · Anything about panels. This path annotates and gates posting; it never
    changes membership, votes or verdicts, by construction.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
