#!/usr/bin/env node
// replay-head-worktree.mjs — the pinned checkout every source-reading agent is
// pointed at, and the parser that decides whether one exists.
//
// Why this needs its own replay. The rule "read at the reviewed commit, not the
// local clone" was prose on two surfaces and got measurably better but never
// reached zero: 19% of one run's refuter reads were unpinned before the
// 2026-09-08 fix, 3% of lens reads and 5% of refuter reads after it. On
// 2026-09-11 a refuter with only the sentence refuted three REAL findings at
// confidence 0.95, citing four symbols that exist in the local checkout and not
// at the reviewed sha; the same prompt against a pinned worktree got all three
// right. A wrongly killed finding leaves no trace in the report, so nothing
// downstream can notice this — the guarantee has to be established here.
//
// The one failure that would be worse than no pin at all is a path-shaped empty
// string reaching a prompt ("read files at " with nothing after it), so that
// case gets its own checks in both directions.
//
// Like eval/replay-diff-sectioning.mjs, this evaluates the REAL code sliced out
// of the workflow by its markers — never a copy.
//
// Usage: node eval/replay-head-worktree.mjs

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

const { api, violations } = loadRegion(WORKFLOW, "HEAD-WORKTREE", [
  "headWorktreeBash",
  "parseHeadWorktree",
  "pinnedCheckoutBlock",
]);
const { headWorktreeBash, parseHeadWorktree, pinnedCheckoutBlock } = api;

const SHA = "991e04734f7ce7410d1c0234951d1b71a4845c4a";

console.log(`\nHEAD-WORKTREE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- parsing the probe's answer ---`);
{
  const w = parseHeadWorktree("CREATED_WORKTREE /tmp/peer-review-wt-1911-abc123");
  check("a created worktree yields its path", w.path === "/tmp/peer-review-wt-1911-abc123");
  check("and is labelled a worktree, which teardown needs", w.kind === "worktree");
  check("and reports ok", w.ok === true);
}
{
  const c = parseHeadWorktree("CLONED:/tmp/peer-review-wt-1911-xyz");
  check("a clone fallback yields its path", c.path === "/tmp/peer-review-wt-1911-xyz");
  // git worktree remove would fail on a plain clone, and rm -rf on a real
  // worktree leaves the repo registering a directory that no longer exists.
  check("and is labelled a clone, because teardown differs", c.kind === "clone");
}
check("an explicit failure is not ok", parseHeadWorktree("WORKTREE_FAILED").ok === false);
check("and carries no path", parseHeadWorktree("WORKTREE_FAILED").path === null);
check("an empty answer is not ok", parseHeadWorktree("").ok === false);
check("a dead dispatch (null) is not ok", parseHeadWorktree(null).ok === false);
check(
  "unrelated chatter is not ok",
  parseHeadWorktree("Preparing worktree\nsomething else entirely").ok === false,
);

// The trap this whole file exists for: a marker with nothing after it must not
// become a path. `"CREATED_WORKTREE "` + "" is exactly what a redirect failure
// or a truncated line produces.
check(
  "a marker with no path after it is NOT a pin",
  parseHeadWorktree("CREATED_WORKTREE ").ok === false,
  JSON.stringify(parseHeadWorktree("CREATED_WORKTREE ")),
);
check(
  "and neither is a clone marker with no path",
  parseHeadWorktree("CLONED:").ok === false,
);

console.log(`\n--- reading the answer out of real command noise ---`);
check(
  "the marker is found among leading git chatter",
  parseHeadWorktree(
    "Preparing worktree (detached HEAD 7495b91f)\nHEAD is now at 7495b91f\nCREATED_WORKTREE /tmp/wt",
  ).path === "/tmp/wt",
);
check(
  "a real worktree wins over a stale clone line",
  parseHeadWorktree("CLONED:/tmp/old\nCREATED_WORKTREE /tmp/new").kind === "worktree",
);

console.log(`\n--- what the agent is actually told ---`);
{
  const pinned = pinnedCheckoutBlock({ path: "/tmp/wt", kind: "worktree", ok: true }, SHA);
  check("the pinned block names the directory", pinned.includes("/tmp/wt"));
  check("and the commit", pinned.includes(SHA));
  check(
    "and forbids the other clone by name, not by implication",
    /not in any other clone/i.test(pinned),
    pinned.slice(0, 120),
  );
  // The conventions, the schema and the sibling modules are IN there at the
  // right version. Today agents fetch CLAUDE.md themselves, from whatever tree
  // they land in — measured at roughly one read per refuter dispatch.
  check(
    "and points conventions and siblings at the same tree",
    /CLAUDE\.md/.test(pinned) && /conventions/i.test(pinned),
  );
}
{
  const degraded = pinnedCheckoutBlock(null, SHA);
  check("the degraded block still demands a pinned read", /git show <sha>:<path>/.test(degraded));
  check("and says plainly that no checkout exists", /NO PINNED CHECKOUT/.test(degraded));
  check(
    "and explains the concrete damage, not just the rule",
    /confirm code the PR deleted/.test(degraded),
  );
  // A degraded block that still says "prepared at" would send the agent to a
  // path that is not there.
  check("and never claims a prepared directory", !/prepared at/.test(degraded));
  check(
    "and never leaves a dangling 'at <nothing>'",
    !/\bat\s*$/m.test(degraded) && !degraded.includes("at  "),
  );
}
{
  // Same guarantee through the shape the parser actually returns on failure,
  // rather than through a hand-written null: these must agree, or the guard
  // holds only for inputs nobody produces.
  const fromParser = pinnedCheckoutBlock(parseHeadWorktree("WORKTREE_FAILED"), SHA);
  check(
    "a parsed failure degrades exactly like an absent pin",
    fromParser === pinnedCheckoutBlock(null, SHA),
  );
  const fromEmptyMarker = pinnedCheckoutBlock(parseHeadWorktree("CREATED_WORKTREE "), SHA);
  check(
    "and so does a marker with an empty path",
    fromEmptyMarker === pinnedCheckoutBlock(null, SHA),
  );
}

console.log(`\n--- the bash that materialises it ---`);
{
  const bash = headWorktreeBash(SHA, "acme/api", 1911);
  check("it pins to the sha it was given", bash.includes(SHA));
  check("it names the repo", bash.includes("acme/api"));
  check("the temp dir carries the PR number, so strays are attributable", bash.includes("wt-1911-"));
  // PR #1518: an unqualified worktree add ran against the wrong repo's remote
  // when the session was anchored in a sibling checkout, and the resulting null
  // path silently dropped -C from the external CLI invocation.
  check("it refuses to build in an unrelated checkout", bash.includes("AMBIENT_MATCH"));
  check("it fetches then retries before giving up", /git fetch origin/.test(bash));
  check("it falls back to a shallow clone", /gh repo clone/.test(bash));
  check("it verifies the result before claiming success", /rev-parse HEAD/.test(bash));
  check("it emits an explicit failure marker", bash.includes("WORKTREE_FAILED"));
  check(
    "every use of the temp path is quoted",
    !/[^"]\$WT[^"]/.test(bash.replace(/WT=\$\(mktemp[^\n]*/, "")),
  );
}

// The two consumers below live outside the pure region because they read run
// state, so they are asserted at the source level — the same way
// replay-fused-availability-parse pins its AMBIENT_MATCH call sites. A guard
// that only exists where it was decided is the failure rule 9 names: a run that
// degraded has to be readable as degraded by everyone downstream, not just by
// whoever watched the log.
console.log(`\n--- the degradation is visible everywhere state is read ---`);
{
  const fs = await import("node:fs");
  const src = fs.readFileSync(WORKFLOW, "utf8");
  check(
    "the metrics row records whether the run was structurally pinned",
    /headPin:\s*\{[\s\S]{0,200}pinned:\s*headPin\.ok/.test(src),
  );
  check(
    "and which kind of checkout it was, since teardown and trust differ",
    /headPin:\s*\{[\s\S]{0,200}kind:\s*headPin\.kind/.test(src),
  );
  check(
    "the posted report carries a banner when there is no pinned checkout",
    // Since 2026-10-23 the banners go into `report` itself, which is what the
    // Workflow returns and Step 4 posts, and reportAsPosted is derived from it.
    /report = withBanners\(\s*report,[\s\S]{0,80}headPin\.ok\s*\?\s*""\s*:\s*HEAD_PIN_DEGRADED_BANNER/.test(src) &&
      /const reportAsPosted = stripDraftCommentsFromBody\(report\);/.test(src),
    "the banner must be appended mechanically, not requested from the synthesis prompt",
  );
  check(
    "and the banner says what it costs the reader, not just that it happened",
    /leaves no trace here/.test(src),
  );
}

console.log("");
if (failures.length) {
  console.log(`replay-head-worktree: ${failures.length} FAILURE(S)`);
  for (const f of failures) console.log(`  - ${f}`);
  process.exit(1);
}
console.log("replay-head-worktree: all checks passed");
