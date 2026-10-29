#!/usr/bin/env node
// replay-embedded-bash.mjs — the shell and Python that peer-review.js embeds in
// its prompts, checked without paying for a run.
//
// WHY THIS EXISTS, precisely. On 2026-07-27 the incremental re-review's baseline
// probe shipped with `python3 -c "…"` and the head sha interpolated inline as
// `${JSON.stringify(...)}` — which emits its own double quotes, closing the bash
// string and cutting the Python in half. Every offline check was green: the
// scope function had a golden, the mirror invariants agreed, 21/21 passed. None
// of them could see it, because the bug was in a STRING that only a shell ever
// parses. It failed open to a full review, exactly as designed, so the only
// symptom was a 16-file run where a 4-file one was expected — 5 lenses instead
// of 2, noticed by a human watching the agent count.
//
// The ladder in SKILL.md says plumbing like this is Tier 1's job. That is true
// of prompts and dispatch shape; it is NOT true of a shell script, which is
// mechanically checkable for zero. This file moves that class down a rung.
//
// Two parts:
//   1. Every quoted heredoc body is parsed as Python. Catches the syntax half.
//   2. The baseline probe's actual bash is RUN, hermetically — `gh` is stubbed
//      on PATH so no network is touched — against the real metrics.jsonl and the
//      real fixture patches, and its JSON output is asserted. Catches the
//      quoting half, which a syntax check alone would not: the broken version
//      was valid bash, it just ran the wrong program.
//
// Usage: node eval/replay-embedded-bash.mjs

import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";
import { FIXTURES_DIR, lib, LIB_DIR, PLUGIN_ROOT, RECORDED_METRICS_PATH, SKILL_DIR, WORKFLOW } from "./paths.mjs";
import { clientProfileByReviewer } from "./client-profiles.mjs";

// The recorded ledger and fixture patches come from the backend repo, and the
// probe matches ledger rows by repo, so the repo is read from its frozen profile.
const BE_REPO = clientProfileByReviewer("backend").profile.repo;

// `bin/` is on the Bash tool's PATH whenever the plugin is enabled, which is how
// the emitted bash resolves the plugin root. The sandbox below has to model
// that or it tests a shell the harness never actually runs in.
const PLUGIN_BIN = `${PLUGIN_ROOT}/bin`;

const HERE = dirname(fileURLToPath(import.meta.url));
const FIX = join(FIXTURES_DIR, "incremental-scope");
const src = readFileSync(WORKFLOW, "utf8");
// The persist step's Python left peer-review.js on 2026-07-28. Part 1 below
// parses every quoted heredoc it can find as Python, and it found them by
// reading the workflow alone — so the move would have silently dropped four
// python bodies (reconcile, audit, persist, report) out of the only check that
// ever compiles them. Concatenated, not switched: both files carry heredocs.
const PERSIST_SH = lib("persist-run.sh");
const srcWithScripts = `${src}\n${readFileSync(PERSIST_SH, "utf8")}`;

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// Every script below runs through a FILE, never through `bash -c`. On Windows
// the command line handed to a spawned process is capped at ~8191 characters,
// and the excess is not an error — it is silently DROPPED, so bash parses a
// script that just stops. Measured cutoff on this platform: 8187 bytes actually
// delivered, binary-searched, and identical at environment blocks of 3.3 KB,
// 6.7 KB and 14.7 KB — the cap is the command line, not environment pressure.
//
// The probe crossed it in v40, when import-adjacency's `PYADJACENCY` heredoc
// grew `src.slice(start, end)` — the RAW block, before the fixture
// substitutions below — from 6203 to 10631 bytes. Name the quantity, because
// the substituted scripts each land somewhere else again (site 1 is the
// largest) and a byte count nobody can re-derive from the sentence around it
// decays into folklore. The symptom is a parse error that names the wrong
// culprit — `here-document delimited by end-of-file (wanted 'PYADJACENCY')` —
// because the terminator is simply not in the bytes bash received. v40 and v41
// both froze with this file red.
//
// `bash <file>` has no such cap and delivers the same bytes. It is also the
// spelling the rest of the plugin already settled on
// (eval/replay-verdicts.mjs, lib/reconcile-votes.mjs), for a different Windows
// defect in the same spawn. The differences that could reach a script are `$0`
// and `BASH_SOURCE` (`bash` vs the temp path), `BASH_ARGV0`, positional
// parameters, and inherited shell options; none appears in the sliced blocks —
// grepped for all of those plus `set -e`/`-u`/`pipefail` and `trap`, zero hits.
// That is a claim about THESE blocks, not about the two forms in general.
const SCRIPT_DIR = mkdtempSync(join(tmpdir(), "pr-script-"));

// A fake HOME holding the recorded ledger, so a probe that looks for an earlier
// review reads the same runs on every machine and never reads, or writes, this
// machine's own ledger.
function recordedHomeEnv() {
  const home = realpathSync(mkdtempSync(join(tmpdir(), "pr-home-probe-")));
  mkdirSync(join(home, ".claude", "tribunal-data"), { recursive: true });
  writeFileSync(join(home, ".claude", "tribunal-data", "metrics.jsonl"), readFileSync(RECORDED_METRICS_PATH));
  return { HOME: home, USERPROFILE: home, TRIBUNAL_DATA_DIR: join(home, ".claude", "tribunal-data") };
}
let scriptSeq = 0;
const runBash = (script, opts) => {
  const file = join(SCRIPT_DIR, `s${++scriptSeq}.sh`);
  writeFileSync(file, script);
  return execFileSync("bash", [file], opts);
};

console.log(`\nembedded shell/Python — ${WORKFLOW}`);

// --- part 1: every quoted heredoc body must be valid Python ------------------
console.log(`\n--- embedded Python parses ---`);
const heredocs = [...srcWithScripts.matchAll(/<<'([A-Z_]+)'\n([\s\S]*?)\n\1\n/g)];
check("at least one quoted heredoc is present", heredocs.length > 0, `${heredocs.length}`);
for (const [, tag, body] of heredocs) {
  // Only Python bodies — a JSON heredoc (METRICSJSON, VOTESJSON) is data.
  if (!/^\s*(import|from)\s/m.test(body)) continue;
  const f = join(mkdtempSync(join(tmpdir(), "pr-heredoc-")), `${tag}.py`);
  writeFileSync(f, body);
  try {
    execFileSync("python3", ["-c", `import ast,sys; ast.parse(open(sys.argv[1]).read())`, f], {
      stdio: ["ignore", "ignore", "pipe"],
    });
    check(`${tag} parses as Python`, true);
  } catch (err) {
    check(`${tag} parses as Python`, false, String(err.stderr || err).slice(0, 200));
  }
}

// --- part 1b: no backticks inside a heredoc ---------------------------------
// Every one of these heredocs lives inside a JS template literal, so a single
// backtick in a comment TERMINATES the template. `node --check` does not catch
// it — the result is still syntactically valid JavaScript, it just means
// something else — and the Workflow parser rejects the file outright.
//
// This shipped: a comment reading "`direction` decides whether the finding still
// blocks" made the workflow unparseable, and it was tagged into v10, v11 and v12
// before anything tried to run it. Three frozen releases that could not start.
console.log(`\n--- no backticks inside the heredocs ---`);
for (const [, tag, body] of heredocs) {
  const n = (body.match(/`/g) || []).length;
  check(`${tag} carries no backtick`, n === 0, `${n} found — this makes the workflow unparseable`);
}

// --- part 2: run the baseline probe's real bash, hermetically ---------------
console.log(`\n--- the baseline probe actually runs ---`);
// Fixture integrity FIRST. Everything below feeds these two patches to the real
// bash, so an empty one produces "scope unusable" — indistinguishable from the
// bug this file exists to catch, and it points the reader at the workflow.
//
// Not hypothetical: on 2026-07-28 an INJECTION run truncated d047fa9f.patch to
// zero. The injected variant pointed `CUR` at the fixture, the next line
// redirects a `gh pr diff` into `$CUR`, and the stub for that arm makes the
// fetch fail — so the redirect emptied the file. Several minutes went into
// diagnosing the workflow before anyone looked at the fixture. An injection that
// runs shell can reach outside its sandbox; this check makes that loud.
for (const f of ["d047fa9f.patch", "0eedf726.patch"]) {
  let size = 0;
  try {
    size = readFileSync(join(FIX, f)).length;
  } catch {
    /* asserted below */
  }
  check(
    `fixture ${f} is intact`,
    size > 10000,
    `${size} bytes — restore it with git checkout before trusting anything below`,
  );
}
// Slice the shell out of the prompt by its own anchors, then substitute the
// template holes. An UNSUBSTITUTED `${` is a hard failure: it means the block
// moved and this test is now checking something other than what ships.
// Anchor on the probe's own unique line and walk BACK to its BASE= — anchoring
// on `BASE=$(gh pr view` alone finds the `--head-sha` block instead, which is
// what the first version of this test did. The "every hole substituted" check
// below is what caught it: a wrong slice shows up as unresolved `${...}`.
// `PRIOR=$(` and not `PRIOR=$(PR_HEAD_SHA=` on purpose: the anchor must survive
// a rewrite of the probe into ANY shell form, so that reverting to the broken
// `python3 -c "…"` version makes this test fail on the JSON assertion — the real
// symptom — instead of on "block not found", which a mere rename would also
// trigger. Verified both ways by reproducing the shipped bug.
const probeStart = src.indexOf("PRIOR=$(");
const start = probeStart === -1 ? -1 : src.lastIndexOf("BASE=$(gh pr view", probeStart);
const end = src.indexOf("Return that final JSON line", probeStart);
check("probe bash found in the workflow", start !== -1 && end > start);

if (start !== -1 && end > start) {
  const HEAD = "d047fa9fffffffffffffffffffffffffffffffff";
  let bash = src
    .slice(start, end)
    .replaceAll("${prNumber}", "1968")
    .replaceAll("${repo}", BE_REPO)
    .replaceAll('"$HEAD_SHA"', JSON.stringify(HEAD))
    .replaceAll("${JSON.stringify(sinceOverride)}", '""')
    .replaceAll("${currentPatchPath}", join(FIX, "d047fa9f.patch"))
    .replace(/\\\$\{/g, "${")
    .trim();
  // Anchor on the JS identifiers, not on `${` — the script legitimately contains
  // shell expansions (`${TMPDIR:-/tmp}`, `${CUR:-none}`) that must survive
  // substitution untouched. A blanket `${` check flags those as holes, which is
  // how the first version of this assertion failed on correct code.
  const holes = ["${prNumber}", "${repo}", "${currentPatchPath}", "${sinceOverride}", "${JSON."].filter(
    (h) => bash.includes(h),
  );
  check("every template hole was substituted", holes.length === 0, holes.join(","));

  // A `gh` stub on PATH: the base branch for `pr view`, and the fixture patch
  // for the compare API. No network, and the assertion below therefore measures
  // the SCRIPT, not GitHub's availability.
  const bin = mkdtempSync(join(tmpdir(), "pr-bin-"));
  writeFileSync(
    join(bin, "gh"),
    `#!/bin/sh
# The PR's base is fetched as baseRefOid — a COMMIT — because branch names move:
# once #1968 merged, compare/development...<sha> returned 0 bytes and the baseline
# fetch produced an empty file. The stub answers with a commit so this test
# exercises the shape that ships, not the one that broke.
# Answers ONLY for --json baseRefOid. A stub that replies to any \`pr view\`
# cannot tell whether the script asked for the base COMMIT or the base BRANCH,
# and that is exactly how this test stayed green while the shipped probe still
# used baseRefName: the fix had been applied to the wrong occurrence.
case "$1 $2" in
  "pr diff") cat ${JSON.stringify(join(FIX, "d047fa9f.patch"))} ;;
  "pr view")
    case "$*" in
      *baseRefOid*) echo 4b7bad7200000000000000000000000000000000 ;;
      *headRefOid*) echo d047fa9fffffffffffffffffffffffffffffffff ;;
      *baseRefName*) echo development ;;
      *) echo "stub gh: pr view asked for something unexpected: $*" >&2; exit 1 ;;
    esac ;;
  "api repos/${BE_REPO}/compare/4b7bad72"*) cat ${JSON.stringify(join(FIX, "0eedf726.patch"))} ;;
  *) echo "stub gh: unexpected $*" >&2; exit 1 ;;
esac
`,
  );
  chmodSync(join(bin, "gh"), 0o755);

  // The probe reads metrics.jsonl for a POSTED row of PR 1968 at a sha that is
  // not HEAD. The recorded ledger holds exactly one: 0eedf726, the review from
  // 2026-07-24 with 3 findings. That row is the fixture.
  let out = "";
  try {
    out = runBash(bash, {
      env: { ...process.env, ...recordedHomeEnv(), PATH: `${bin}:${PLUGIN_BIN}:${process.env.PATH}` },
      encoding: "utf8",
    });
  } catch (err) {
    out = `${err.stdout || ""}${err.stderr || ""}`;
  }
  const last = out.trim().split("\n").filter(Boolean).pop() || "";
  let parsed = null;
  try {
    parsed = JSON.parse(last);
  } catch {
    /* left null — asserted below */
  }
  check("the probe emits one line of parseable JSON", parsed !== null, last.slice(0, 200));
  if (parsed) {
    check(
      "it FINDS the posted baseline (this is the bug that shipped: found=false)",
      parsed.prior?.found === true,
      parsed.prior?.reason,
    );
    check("the baseline is the 0eedf726 review", (parsed.prior?.sha || "").startsWith("0eedf726"));
    check("it carries that review's findings", (parsed.prior?.findings || []).length === 3, `${(parsed.prior?.findings || []).length}`);
    check("the scope is usable", parsed.scope?.usable === true, parsed.scope?.reason);
    check("4 files in scope, not 16", (parsed.scope?.inScope || []).length === 4, JSON.stringify(parsed.scope?.inScope));
    check("12 files carried forward", (parsed.scope?.unchanged || []).length === 12, `${(parsed.scope?.unchanged || []).length}`);
  }
}

// --- part 2b: the human-review baseline source, through the real bash -------
// Part 2 only exercises source 1 (a tribunal-row baseline already existed for
// PR 1968). This proves the OTHER source reaches all the way through: a `gh`
// stub answers `--json reviews` with a real APPROVED review and no matching
// metrics row, and the probe's own JSON must come back with
// source: "human-review" and the review's commit as sha — not just the pure
// function in isolation (that is replay-resolve-baseline.mjs's job), but the
// bash that fetches reviews and pipes them through resolve-baseline.mjs.
console.log(`\n--- the human-review baseline source, through the real bash ---`);
if (start !== -1 && end > start) {
  const NOROW_PR = "88888888"; // no metrics.jsonl row will ever exist for this
  const NOROW_HEAD = "cafef00dcafef00dcafef00dcafef00dcafef00d";
  const bin4 = mkdtempSync(join(tmpdir(), "pr-bin4-"));
  writeFileSync(
    join(bin4, "gh"),
    `#!/bin/sh
case "$1 $2" in
  "pr diff") echo "" ;;
  "pr view")
    case "$*" in
      *baseRefOid*) echo 4b7bad7200000000000000000000000000000000 ;;
      *headRefOid*) echo ${NOROW_HEAD} ;;
      *baseRefName*) echo development ;;
      *reviews*) echo '[{"author":"alice","state":"APPROVED","at":"2026-09-09T13:44:09Z","commit":"b548bfc335ca3d8e16f83b49ba04b1a02c6f3601"},{"author":"alice","state":"COMMENTED","at":"2026-09-09T17:52:43Z","commit":"6f1a852f00000000000000000000000000000000"},{"author":"coderabbitai","state":"APPROVED","at":"2026-09-09T18:00:00Z","commit":"deadbeef00000000000000000000000000000000"}]' ;;
      *) echo "stub gh: pr view asked for something unexpected: $*" >&2; exit 1 ;;
    esac ;;
  "api repos/acme/api/compare/4b7bad72"*) echo "" ;;
  *) echo "stub gh: unexpected $*" >&2; exit 1 ;;
esac
`,
  );
  chmodSync(join(bin4, "gh"), 0o755);

  // A throwaway HOME whose metrics.jsonl exists but holds no row for
  // NOROW_PR, so source 1 finds nothing and the probe must fall to source 2.
  const fakeHome4 = realpathSync(mkdtempSync(join(tmpdir(), "pr-home4-")));
  const dataDir4 = join(fakeHome4, ".claude", "tribunal-data");
  mkdirSync(dataDir4, { recursive: true });
  writeFileSync(join(dataDir4, "metrics.jsonl"), "");

  const bash4 = src
    .slice(start, end)
    .replaceAll("${prNumber}", NOROW_PR)
    .replaceAll("${repo}", "acme/api")
    .replaceAll('"$HEAD_SHA"', JSON.stringify(NOROW_HEAD))
    .replaceAll("${JSON.stringify(sinceOverride)}", '""')
    .replace(/\\\$\{/g, "${")
    .trim();

  let out4 = "";
  try {
    out4 = runBash(bash4, {
      env: {
        ...process.env,
        HOME: fakeHome4,
        USERPROFILE: fakeHome4,
        TRIBUNAL_DATA_DIR: join(fakeHome4, ".claude", "tribunal-data"),
        PATH: `${bin4}:${PLUGIN_BIN}:${process.env.PATH}`,
      },
      encoding: "utf8",
    });
  } catch (err) {
    out4 = `${err.stdout || ""}${err.stderr || ""}`;
  }
  const last4 = out4.trim().split("\n").filter(Boolean).pop() || "";
  let parsed4 = null;
  try {
    parsed4 = JSON.parse(last4);
  } catch {
    /* asserted below */
  }
  check("the probe emits one line of parseable JSON (human-review case)", parsed4 !== null, last4.slice(0, 200));
  if (parsed4) {
    check(
      "no tribunal row for this PR -> source is human-review",
      parsed4.prior?.source === "human-review",
      JSON.stringify(parsed4.prior),
    );
    // The stub's three reviews are shaped so that this one assertion is
    // load-bearing for BOTH filters, and the expected winner is the OLDEST of
    // the three. Break the COMMENTED exclusion and alice's later COMMENTED
    // wins with 6f1a852f. Break the bot exclusion and coderabbitai's later
    // APPROVED wins with deadbeef. Two earlier versions of this fixture caught
    // neither: the first gave both reviews the same commit, so any pick
    // passed, and the second made the only COMMENTED review a bot's, so the
    // bot filter masked the state filter and injecting COMMENTED changed
    // nothing.
    check(
      "the baseline sha is the APPROVED human review's commit, not the later COMMENTED or bot one",
      parsed4.prior?.sha === "b548bfc335ca3d8e16f83b49ba04b1a02c6f3601",
      parsed4.prior?.sha,
    );
    check("a human-review baseline carries no findings to verify", (parsed4.prior?.findings || []).length === 0, JSON.stringify(parsed4.prior?.findings));
  }
}

// --- part 3: the probe depends on NO relayed path, and still fails open -----
// History: `prContext.diffPath` was transcribed by the fetch agent, and on
// 2026-07-27 it came back with one character altered (`_` for `-` in the
// repo name). The file existed; the name did not, and the scope silently
// went unusable. The probe first learned to fall back to a well-known path; as
// of 2026-07-28 it takes no relayed path at all and fetches its own copy, which
// is also what makes it safe to dispatch CONCURRENTLY with fetch-pr.
//
// So this part now asserts the stronger property — the relayed path is GONE from
// the shipped bash — and then that the fail-open behaviour survives when the
// probe's own fetch is the thing that fails.
console.log(`\n--- the probe fetches its own patch and still fails open ---`);
if (start !== -1 && end > start) {
  const tmp = mkdtempSync(join(tmpdir(), "pr-tmp-"));
  writeFileSync(join(tmp, "peer-review-diff-1968.patch"), readFileSync(join(FIX, "d047fa9f.patch")));
  const bin2 = mkdtempSync(join(tmpdir(), "pr-bin2-"));
  writeFileSync(
    join(bin2, "gh"),
    `#!/bin/sh
# The PR's base is fetched as baseRefOid — a COMMIT — because branch names move:
# once #1968 merged, compare/development...<sha> returned 0 bytes and the baseline
# fetch produced an empty file. The stub answers with a commit so this test
# exercises the shape that ships, not the one that broke.
# Answers ONLY for --json baseRefOid. A stub that replies to any \`pr view\`
# cannot tell whether the script asked for the base COMMIT or the base BRANCH,
# and that is exactly how this test stayed green while the shipped probe still
# used baseRefName: the fix had been applied to the wrong occurrence.
case "$1 $2" in
  "pr diff") cat ${JSON.stringify(join(FIX, "d047fa9f.patch"))} ;;
  "pr view")
    case "$*" in
      *baseRefOid*) echo 4b7bad7200000000000000000000000000000000 ;;
      *headRefOid*) echo d047fa9fffffffffffffffffffffffffffffffff ;;
      *baseRefName*) echo development ;;
      *) echo "stub gh: pr view asked for something unexpected: $*" >&2; exit 1 ;;
    esac ;;
  "api repos/${BE_REPO}/compare/4b7bad72"*) cat ${JSON.stringify(join(FIX, "0eedf726.patch"))} ;;
  *) echo "stub gh: unexpected $*" >&2; exit 1 ;;
esac
`,
  );
  chmodSync(join(bin2, "gh"), 0o755);
  const bash2 = src
    .slice(start, end)
    .replaceAll("${prNumber}", "1968")
    .replaceAll("${repo}", BE_REPO)
    .replaceAll('${JSON.stringify(prContext.headRefOid || "")}', JSON.stringify("d047fa9f" + "f".repeat(32)))
    .replaceAll("${JSON.stringify(sinceOverride)}", '""')
    .replace(/\\\$\{/g, "${")
    .trim();
  let out2 = "";
  try {
    out2 = runBash(bash2, {
      env: { ...process.env, ...recordedHomeEnv(), PATH: `${bin2}:${PLUGIN_BIN}:${process.env.PATH}`, TMPDIR: tmp },
      encoding: "utf8",
    });
  } catch (err) {
    out2 = `${err.stdout || ""}${err.stderr || ""}`;
  }
  let p2 = null;
  try {
    p2 = JSON.parse(out2.trim().split("\n").filter(Boolean).pop() || "");
  } catch {
    /* asserted below */
  }
  check(
    "the shipped probe takes no relayed patch path",
    !bash2.includes("currentPatchPath") && !src.slice(start, end).includes("${currentPatchPath}"),
    "a relayed path is back — the concurrency with fetch-pr is now a race",
  );
  check("still emits parseable JSON", p2 !== null, out2.slice(-160));
  check(
    "it fetches its own patch and scopes 4 files",
    p2?.scope?.usable === true && (p2?.scope?.inScope || []).length === 4,
    JSON.stringify(p2?.scope?.reason || p2?.scope?.inScope),
  );
  // And when the probe's OWN fetch fails, it must fail open rather than narrow.
  // A stub that refuses `pr diff` is the only way to reach that branch now that
  // nothing is relayed to it — pointing TMPDIR somewhere empty no longer does,
  // because the probe simply fetches the patch again.
  const bin3 = mkdtempSync(join(tmpdir(), "pr-bin3-"));
  writeFileSync(
    join(bin3, "gh"),
    `#!/bin/sh
case "$1 $2" in
  "pr diff") echo "stub gh: diff unavailable" >&2; exit 1 ;;
  "pr view")
    case "$*" in
      *baseRefOid*) echo 4b7bad7200000000000000000000000000000000 ;;
      *headRefOid*) echo d047fa9fffffffffffffffffffffffffffffffff ;;
      *baseRefName*) echo development ;;
      *) exit 1 ;;
    esac ;;
  "api repos/${BE_REPO}/compare/4b7bad72"*) cat ${JSON.stringify(join(FIX, "0eedf726.patch"))} ;;
  *) exit 1 ;;
esac
`,
  );
  chmodSync(join(bin3, "gh"), 0o755);
  let out3 = "";
  try {
    out3 = runBash(bash2, {
      env: { ...process.env, ...recordedHomeEnv(), PATH: `${bin3}:${PLUGIN_BIN}:${process.env.PATH}`, TMPDIR: mkdtempSync(join(tmpdir(), "pr-empty-")) },
      encoding: "utf8",
    });
  } catch (err) {
    out3 = `${err.stdout || ""}${err.stderr || ""}`;
  }
  let p3 = null;
  try {
    p3 = JSON.parse(out3.trim().split("\n").filter(Boolean).pop() || "");
  } catch {
    /* asserted below */
  }
  check(
    "its own fetch failing -> unusable, and it names why",
    p3?.scope?.usable === false && /patch not found or empty/.test(p3?.scope?.reason || ""),
    JSON.stringify(p3?.scope?.reason),
  );
}

// --- part 4b: NO top-level statement may use a const declared later ---------
// Part 4 below checks one block — the preflight early-return — for temporal dead
// zone errors. The CLASS is "any top-level use before declaration", and on
// 2026-07-28 the class bit somewhere part 4 does not look: `parallel([
// prContextThunk, bookkeepingThunk ])` was written 50 lines above
// `const bookkeepingThunk`. Every offline check was green, the file parsed, and
// the Workflow died on dispatch with "Cannot access 'bookkeepingThunk' before
// initialization" — zero agents, but a launched run to find it.
//
// Same shape as the v10 backtick gate that covered heredocs only, and the
// region-coverage check that read one file only. A check scoped to one location
// of a class gets recorded as covering the class.
//
// Scope: top-level `const NAME = ` declarations, and their first use at column 0
// (i.e. in a top-level statement, not nested inside a function body that may run
// later). Deliberately narrow — a false alarm here would be worse than the gap,
// because it would be argued with rather than fixed.
console.log(`\n--- parallel() thunks are declared before they are used ---`);
{
  // NARROW ON PURPOSE. The class is "top-level use before declaration", and the
  // instance that killed a run on 2026-07-28 was a thunk list:
  //
  //     const [prContext, contextBookkeeping] = await parallel([
  //       prContextThunk,
  //       bookkeepingThunk,          <- declared 8 lines BELOW this line
  //     ]);
  //
  // Two general line-based scans were attempted first and both came back green
  // on that exact injection; a third reported prompt prose as code, because a
  // template literal's interior lines sit at column 0 and are indistinguishable
  // from statements without real parsing. Rather than ship a scanner whose
  // failures have to be argued with, this checks the one construct where the
  // fault actually occurs and where the text is unambiguous — a `parallel([…])`
  // argument list holds identifiers, never prose.
  //
  // What it does NOT cover is stated plainly: any other top-level forward
  // reference. That gap is real and recorded in RELEASES.md rather than papered
  // over by a check that cannot see it either.
  const decl = new Map();
  for (const m of src.matchAll(/^const ([A-Za-z_$][\w$]*)\s*=/gm))
    if (!decl.has(m[1])) decl.set(m[1], m.index);

  const calls = [...src.matchAll(/^(?:const .*?=\s*)?await parallel\(\[/gm)];
  check("parallel() call sites were found", calls.length > 0, `${calls.length}`);
  const early = [];
  for (const c of calls) {
    const open = c.index + c[0].length;
    const close = src.indexOf("]);", open);
    if (close === -1) continue;
    const argsText = src.slice(open, close);
    // Identifiers passed by NAME. An inline `() => …` thunk is code in place and
    // cannot be a forward reference.
    for (const m of argsText.matchAll(/(^|[,\s])([A-Za-z_$][\w$]*)\s*(?=,|$)/gm)) {
      const name = m[2];
      const at = decl.get(name);
      if (at !== undefined && at > c.index)
        early.push(`${name} passed to parallel() at offset ${c.index}, declared at ${at}`);
    }
  }
  check(
    "every named thunk is declared above its parallel() call",
    early.length === 0,
    early.slice(0, 4).join("; "),
  );
}

// --- part 4: the early return must not reference anything declared later -----
// `--preflight` returns from the middle of the script, and a `const` used above
// its own declaration is a TEMPORAL DEAD ZONE error — which `node --check` does
// not see, because it is a runtime failure in syntactically valid code. The
// first preflight did exactly this (it read `diffMode`, declared 200 lines
// below) and died on a live PR after two agents. Cheap to check statically,
// impossible to notice by reading.
console.log(`\n--- the preflight early-return references nothing declared later ---`);
const blockStart = src.indexOf("if (preflight) {");
check("preflight block found", blockStart !== -1);
if (blockStart !== -1) {
  // Brace-match to find the block's end.
  let depth = 0;
  let blockEnd = blockStart;
  for (let i = src.indexOf("{", blockStart); i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}") {
      depth--;
      if (depth === 0) {
        blockEnd = i;
        break;
      }
    }
  }
  const block = src.slice(blockStart, blockEnd);
  // Identifiers the block reads, minus property accesses and its own locals.
  const localNames = new Set(
    [...block.matchAll(/\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g)].map((m) => m[1]),
  );
  const referenced = new Set(
    [...block.matchAll(/(^|[^.\w$])([A-Za-z_$][\w$]*)\s*(?![\w$]*\s*:)/g)]
      .map((m) => m[2])
      .filter((n) => !localNames.has(n)),
  );
  const late = [];
  for (const name of referenced) {
    const decl = new RegExp(`^(?:const|let|function)\\s+${name}\\b`, "m").exec(src);
    if (decl && decl.index > blockStart) late.push(`${name} (declared ${decl.index - blockStart} chars later)`);
  }
  check(
    "no module-level binding is used before its declaration",
    late.length === 0,
    late.slice(0, 5).join("; "),
  );
}

// --- part 5: the FUSED context-bookkeeping dispatch --------------------------
// `mark-run-started` and `incremental-scope` used to be two agents. They are one
// now, because a Workflow script has no shell and every separate piece of
// bookkeeping bash therefore costs a full round-trip — 10 of 17 agents and 55%
// of the output tokens on the 3-file PR #1970.
//
// A fusion is only safe if the second half still does its job while the first
// half runs in front of it, and that is a claim about SHELL, which is what this
// file exists to settle for zero. So the marker's real bash is sliced out and
// prepended to the probe's real bash, exactly as the prompt assembles them, and
// run against a throwaway HOME: the appended inflight line must be there, and
// the emitted JSON must carry both the epoch and the unchanged scope.
console.log(`\n--- the fused marker + probe runs as one script ---`);
const markerStart = src.indexOf("const MARKER_BASH = `");
const markerOpen = markerStart === -1 ? -1 : src.indexOf("`", markerStart) + 1;
const markerEnd = markerOpen === -1 ? -1 : src.indexOf("`", markerOpen);
check("marker bash found in the workflow", markerStart !== -1 && markerEnd > markerOpen);

if (markerStart !== -1 && markerEnd > markerOpen && start !== -1 && end > start) {
  const HEAD = "d047fa9fffffffffffffffffffffffffffffffff";
  const markerBash = src
    .slice(markerOpen, markerEnd)
    .replaceAll("${prNumber}", "1968")
    .replaceAll('${JSON.stringify(prContext.headRefOid || null)}', JSON.stringify(HEAD))
    .replaceAll("${forceBatchPanels}", "false")
    .replaceAll("${repo}", BE_REPO)
    .replaceAll("${deployBranchCasePattern(PROFILE)}", "staging|uat|production")
    .replace(/\\\$\{/g, "${");
  check(
    "every marker template hole was substituted",
    !["${prNumber}", "${JSON.", "${forceBatchPanels}", "${repo}", "${deployBranch"].some((h) => markerBash.includes(h)),
    markerBash.slice(0, 120),
  );

  const probeBash = src
    .slice(start, end)
    .replaceAll("${prNumber}", "1968")
    .replaceAll("${repo}", BE_REPO)
    .replaceAll('"$HEAD_SHA"', JSON.stringify(HEAD))
    .replaceAll("${JSON.stringify(sinceOverride)}", '""')
    .replaceAll("${currentPatchPath}", join(FIX, "d047fa9f.patch"))
    .replace(/\\\$\{/g, "${")
    .trim();

  const bin = mkdtempSync(join(tmpdir(), "pr-bin5-"));
  writeFileSync(
    join(bin, "gh"),
    `#!/bin/sh
case "$1 $2" in
  "pr diff") cat ${JSON.stringify(join(FIX, "d047fa9f.patch"))} ;;
  "pr view")
    case "$*" in
      *headRefOid*) echo d047fa9fffffffffffffffffffffffffffffffff ;;
      *baseRefName*) echo development ;;
      *) echo 4b7bad7200000000000000000000000000000000 ;;
    esac ;;
  "api repos/${BE_REPO}/compare/4b7bad72"*) cat ${JSON.stringify(join(FIX, "0eedf726.patch"))} ;;
  *) echo "stub gh: unexpected $*" >&2; exit 1 ;;
esac
`,
  );
  chmodSync(join(bin, "gh"), 0o755);

  // A throwaway HOME so the marker's `~/.claude/...` append lands somewhere
  // disposable. metrics.jsonl is the probe's real input, so the fake HOME gets a
  // copy of the real one — otherwise this would prove the fusion runs and
  // nothing about whether the probe still finds its baseline underneath it.
  //
  // `HOME` ALONE DOES NOT ISOLATE THIS ON WINDOWS, and the failure is silent and
  // destructive. Bash expands `~` from $HOME, but Python's os.path.expanduser
  // prefers %USERPROFILE% there, so the shell half of persist-run.sh created the
  // throwaway tree while its python half appended to the developer's REAL
  // ~/.claude/tribunal-data/metrics.jsonl. Measured 2026-08-10: four synthetic
  // `pr 4242` rows landed in live measurement data, which is the one file this
  // harness calls irreproducible — and the Tier 0 replays re-decide recorded
  // panels out of it, so the pollution would have fed straight back into the
  // next "green" battery. Every spawn below sets BOTH.
  // realpath, not the mkdtemp path: on macOS /var is a symlink to /private/var,
  // and incremental-scope.mjs guards its main block with
  // `import.meta.url === file://${process.argv[1]}`. Handed the unresolved path
  // that comparison is false, the script prints nothing, SCOPE comes back empty
  // and the probe looks broken when it is not. Cost an hour once; noted here so
  // it costs nobody else one.
  // Only the DATA the probe reads is faked. The scripts it runs are resolved by
  // the emitted bash through `tribunal-root`, which points at the real plugin —
  // so copying them into the fake HOME would test a copy instead of what ships.
  const fakeHome = realpathSync(mkdtempSync(join(tmpdir(), "pr-home5-")));
  const dataDir = join(fakeHome, ".claude", "tribunal-data");
  mkdirSync(dataDir, { recursive: true });
  writeFileSync(join(dataDir, "metrics.jsonl"), readFileSync(RECORDED_METRICS_PATH));

  let out = "";
  try {
    out = runBash(`${markerBash}\n${probeBash}`, {
      env: {
        ...process.env,
        HOME: fakeHome,
        USERPROFILE: fakeHome,
        TRIBUNAL_DATA_DIR: join(fakeHome, ".claude", "tribunal-data"),
        PATH: `${bin}:${PLUGIN_BIN}:${process.env.PATH}`,
      },
      encoding: "utf8",
    });
  } catch (err) {
    out = `${err.stdout || ""}${err.stderr || ""}`;
  }
  const last = out.trim().split("\n").filter(Boolean).pop() || "";
  let parsed = null;
  try {
    parsed = JSON.parse(last);
  } catch {
    /* asserted below */
  }
  check("the fused script emits one line of parseable JSON", parsed !== null, last.slice(0, 200));

  let inflight = "";
  try {
    inflight = readFileSync(join(dataDir, "inflight-runs.jsonl"), "utf8");
  } catch {
    /* asserted below */
  }
  const marker = inflight.trim().split("\n").filter(Boolean).pop() || "";
  let markerRow = null;
  try {
    markerRow = JSON.parse(marker);
  } catch {
    /* asserted below */
  }
  check("the marker half still appends its started line", markerRow?.event === "started", marker.slice(0, 120));
  check("that line carries a numeric epoch", Number.isInteger(markerRow?.epoch), String(markerRow?.epoch));

  if (parsed) {
    // The whole reason startEpoch became a schema'd field: it used to travel
    // home as `epoch=N` inside an unschema'd text return, and on 2026-07-27
    // three rows shipped wallClockSeconds: null while the marker line had been
    // written all along.
    check(
      "startEpoch reaches the return value, and matches the appended line",
      Number.isInteger(parsed.startEpoch) && parsed.startEpoch === markerRow?.epoch,
      `${parsed.startEpoch} vs ${markerRow?.epoch}`,
    );
    // The probe half must be untouched by the marker running in front of it.
    check("the fused run still FINDS the posted baseline", parsed.prior?.found === true, parsed.prior?.reason);
    check("the fused run still scopes 4 files, not 16", (parsed.scope?.inScope || []).length === 4, JSON.stringify(parsed.scope?.inScope));
  }

  // Probe WITHOUT the marker (--preflight): startEpoch must be present and null,
  // never absent and never a guess. `EPOCH` is simply unset in that shell.
  let soloOut = "";
  try {
    soloOut = runBash(probeBash, {
      env: {
        ...process.env,
        HOME: fakeHome,
        USERPROFILE: fakeHome,
        TRIBUNAL_DATA_DIR: join(fakeHome, ".claude", "tribunal-data"),
        PATH: `${bin}:${PLUGIN_BIN}:${process.env.PATH}`,
      },
      encoding: "utf8",
    });
  } catch (err) {
    soloOut = `${err.stdout || ""}${err.stderr || ""}`;
  }
  let solo = null;
  try {
    solo = JSON.parse(soloOut.trim().split("\n").filter(Boolean).pop() || "");
  } catch {
    /* asserted below */
  }
  check(
    "probe alone (preflight) emits startEpoch: null, key present",
    solo !== null && "startEpoch" in solo && solo.startEpoch === null,
    JSON.stringify(solo).slice(0, 160),
  );
}

// --- part 6: lib/persist-run.sh actually RUNS -------------------------------
// The row-writing bash moved out of the prompt and onto disk on 2026-07-28. Part
// 1 proves its Python compiles; that is not the same as proving the script
// works, and this is the step whose failure is most expensive — a broken persist
// means the run happened and no measurement of it exists. Before the move a
// break here would at least have shown up as a mangled prompt; now it is a file
// nobody looks at between runs. So it gets executed, hermetically.
console.log(`\n--- lib/persist-run.sh writes a row ---`);
{
  // realpath: incremental-scope.mjs guards its main block with
  // `import.meta.url === file://${process.argv[1]}`, and /var vs /private/var
  // makes that comparison false. Same trap as part 5.
  const home = realpathSync(mkdtempSync(join(tmpdir(), "pr-persist-home-")));
  const skillDir = join(home, ".claude", "tribunal-data");
  const libDir = join(home, ".claude", "lib");
  mkdirSync(join(skillDir, "scripts"), { recursive: true });
  mkdirSync(libDir, { recursive: true });
  const realLib = LIB_DIR;
  const realSkill = SKILL_DIR;
  // The real helpers, so this exercises the shipping path rather than a stub-
  // shaped approximation of it.
  // classify-verdict.sh joins the list: reconcile-votes.mjs now
  // resolves it as a SIBLING, so a libDir without it is silently a broken-
  // classifier environment. Copying it keeps the hermetic copy a faithful one,
  // and leaves TRIBUNAL_CLASSIFIER as the explicit way to break it below.
  for (const f of ["persist-run.sh", "reconcile-votes.mjs", "audit-run.mjs", "classify-verdict.sh"])
    writeFileSync(join(libDir, f), readFileSync(join(realLib, f)));
  writeFileSync(
    join(skillDir, "scripts", "harness-version.mjs"),
    readFileSync(join(realSkill, "scripts", "harness-version.mjs")),
  );

  const vd = join(home, "votes");
  mkdirSync(vd, { recursive: true });
  const START = Math.floor(Date.now() / 1000) - 300;
  writeFileSync(
    join(skillDir, "inflight-runs.jsonl"),
    `${JSON.stringify({ event: "started", pr: 4242, headSha: "cafebabe", epoch: START })}\n`,
  );
  const row = {
    schemaVersion: 3,
    pr: 4242,
    headSha: "cafebabe",
    mode: "workflow",
    findings: [{ file: "a.ts", title: "t", survived: true }],
  };
  const runPersist = (expected, { hasVotes = "0", env: extraEnv = {} } = {}) => {
    writeFileSync(join(vd, "metrics.json"), JSON.stringify(row));
    try {
      return execFileSync(
        "bash",
        [join(libDir, "persist-run.sh"), vd, hasVotes, String(expected), "4242", "cafebabe", "false", ""],
        {
          env: {
            ...process.env,
            HOME: home,
            USERPROFILE: home,
            TRIBUNAL_DATA_DIR: join(home, ".claude", "tribunal-data"),
            PATH: `${PLUGIN_BIN}:${process.env.PATH}`,
            ...extraEnv,
          },
          encoding: "utf8",
        },
      );
    } catch (err) {
      return `${err.stdout || ""}${err.stderr || ""}`;
    }
  };

  const out = runPersist(1);
  let reported = null;
  try {
    reported = JSON.parse(out.trim().split("\n").filter(Boolean).pop() || "");
  } catch {
    /* asserted below */
  }
  check("it prints one line of parseable JSON", reported !== null, out.slice(-200));
  check("it reports the row as persisted", reported?.persisted === "ok", String(reported?.persisted).slice(0, 120));

  const logPath = join(skillDir, "metrics.jsonl");
  let written = null;
  try {
    written = JSON.parse(readFileSync(logPath, "utf8").trim().split("\n").pop());
  } catch {
    /* asserted below */
  }
  check("the row reached metrics.jsonl", written?.pr === 4242, JSON.stringify(written)?.slice(0, 120));
  check("it stamped a ts", typeof written?.ts === "string" && written.ts.endsWith("Z"), String(written?.ts));
  check("it kept every finding", (written?.findings || []).length === 1, `${(written?.findings || []).length}`);
  // The whole reason the marker exists: the row must carry the elapsed time,
  // resolved from the inflight trail rather than from anything a model said.
  check(
    "wall clock resolved from the inflight trail",
    written?.wallClockSource === "inflight-exact" && written?.wallClockSeconds >= 300,
    `${written?.wallClockSource} / ${written?.wallClockSeconds}`,
  );
  check("selfAudit was attached", written?.selfAudit !== undefined, JSON.stringify(written?.selfAudit)?.slice(0, 80));
  check(
    "the completed marker was appended",
    readFileSync(join(skillDir, "inflight-runs.jsonl"), "utf8").includes('"event":"completed"'),
  );

  // v63. The report the fidelity critic judged has to survive to disk, or a
  // `fidelity-critic-flagged` violation names a drift nobody can go back and
  // read. The body below is deliberately hostile: fenced code, inline
  // backticks, a `$(...)` that would execute under an unquoted heredoc, and a
  // line reading REPORTJSON that would close a naive delimiter early.
  {
    const nasty = [
      "## Blocking",
      "",
      "1. `assertDevTarget` guard order \u2014 src/x.ts:77",
      "",
      "```ts",
      "const [identity] = await prisma.$queryRaw`SELECT VERSION()`;",
      "```",
      "",
      "REPORTJSON",
      "$(touch /tmp/tribunal-should-not-exist)",
    ].join("\n");
    writeFileSync(join(vd, "report.json"), JSON.stringify({ report: nasty }));
    runPersist(1);
    const mdPath = join(vd, "report.md");
    check("the report reaches report.md", existsSync(mdPath));
    check(
      "byte-identical, backticks and delimiter line and all",
      existsSync(mdPath) && readFileSync(mdPath, "utf8") === nasty,
      existsSync(mdPath) ? JSON.stringify(readFileSync(mdPath, "utf8").slice(0, 80)) : "absent",
    );
    check(
      "nothing in it was executed on the way",
      !existsSync("/tmp/tribunal-should-not-exist"),
    );
    // An older caller writes no report.json. That must cost the row nothing.
    rmSync(mdPath, { force: true });
    rmSync(join(vd, "report.json"), { force: true });
    const outNoReport = runPersist(1);
    check(
      "a run with no report.json still persists its row",
      /"persisted":\s*"ok"/.test(outNoReport) && !existsSync(mdPath),
      outNoReport.slice(-120),
    );
  }

  // The guard that matters most: a record that lost findings in transit must be
  // REFUSED, not written. This is the 2026-07-23 failure — a run landed with 17
  // of 30 findings and nothing said so.
  const before = readFileSync(logPath, "utf8").split("\n").filter(Boolean).length;
  const refusedOut = runPersist(9);
  const after = readFileSync(logPath, "utf8").split("\n").filter(Boolean).length;
  check("a truncated record is REFUSED", /REFUSED/.test(refusedOut), refusedOut.slice(-160));
  check("and nothing is appended when it is", after === before, `${before} -> ${after}`);

  // The classifier-failed count has to survive the whole pipe —
  // reconcile-votes.mjs -> recon.json (--compact) -> persist-run.sh's Python ->
  // the row -> audit-run.mjs. Every hop is a place it can be dropped silently,
  // and the row is the only artifact that outlives the temp dir, so a count that
  // dies in transit leaves a past run indistinguishable from one that was audited
  // and came back clean. Covered here rather than in replay-reconciliation.mjs
  // because that file stops at the reconciler's stdout; this is the only harness
  // that runs the shipping persist script for real.
  //
  // Mutation-checked 2026-08-12: renaming the key that persist-run.sh reads makes
  // both assertions below fail. Before this case, the same mutation left the
  // entire battery green.
  {
    writeFileSync(
      join(vd, "recorded-votes.json"),
      JSON.stringify([
        { slotId: 0, source: "codex", verdict: "REFUTED", confidence: 0.9, slotStatus: "OK" },
        { slotId: 1, source: "codex", verdict: "NOT_REFUTED", confidence: 0.8, slotStatus: "OK" },
      ]),
    );
    writeFileSync(join(vd, "0-codex.out"), "VERDICT: REFUTED\nCONFIDENCE: 0.9\n");
    writeFileSync(join(vd, "1-codex.out"), "VERDICT: NOT_REFUTED\nCONFIDENCE: 0.8\n");
    const unrunnable = join(libDir, "no-such-classifier.sh");
    runPersist(1, { hasVotes: "1", env: { TRIBUNAL_CLASSIFIER: unrunnable } });
    let r = null;
    try {
      r = JSON.parse(readFileSync(logPath, "utf8").trim().split("\n").pop());
    } catch {
      /* asserted below */
    }
    check(
      "a dead classifier lands in the row as classifierFailed, not as a clean audit",
      r?.voteReconciliation?.classifierFailed === 2,
      JSON.stringify(r?.voteReconciliation)?.slice(0, 160),
    );
    // And the self-audit must refuse to call that run clean. `checked` still
    // equals the slot count, so the pre-existing `reconciliation-incomplete`
    // invariant cannot see it — this is the arm that can.
    check(
      "and the self-audit refuses to pass on it",
      r?.selfAudit?.passed === false &&
        (r?.selfAudit?.violations || []).some((x) => x.id === "classifier-unavailable"),
      JSON.stringify(r?.selfAudit)?.slice(0, 200),
    );
  }
}

// --- part 7: the delivery path itself ---------------------------------------
//
// Everything above asserts what the scripts DO. This asserts that they arrive.
// The distinction is the whole reason this section exists: when the probe
// outgrew the Windows command-line cap in v40, every check above went red at
// once, and each one blamed the script — `here-document delimited by
// end-of-file`, `unexpected EOF` — while the script was fine and the SPAWN was
// lossy. Two freezes shipped past that, so the failure this file most needs to
// name in its own words is the one where its own harness eats the input.
//
// Fixed size, deliberately: it does not track the probe's size, because a
// check asserting "the probe exceeds the cap" would go red the day the probe
// shrank, which is not a regression.
//
// Fault-injected, not assumed — ON WINDOWS: reverting `runBash` to
// `execFileSync("bash", ["-c", script])` makes this check fail, and the
// delivered-byte boundary sits at 8187 pass / 8188 fail, both reproduced
// before this was trusted.
//
// State the limit plainly, because this plugin has already written down the
// principle it runs against. eval/check-mirror.mjs, in this same ticket's
// rule, says a guard that fires on one platform is not a guard. This one is
// exactly that shape: off Windows the equivalent limit is ARG_MAX, orders of
// magnitude above 12 KB, so the old spelling would leave the whole battery
// green there and nothing here would notice. That half is reasoned from the
// documented limit, NOT measured — every number in this file was taken on
// Windows, which is the same blind spot the rule warns about. Closing it needs a
// source-level pin in check-mirror.mjs, which does not read this file today —
// and such a rule has to survive the trailing-comment hole that file documents,
// since the comment above quotes the forbidden spelling verbatim.
console.log("\ndelivery — a script larger than the Windows command-line cap");
{
  const PAYLOAD = 12000;
  // The payload is carried in a VARIABLE and its length echoed back, not left
  // as an inert comment. A comment would only prove the last line ran, so any
  // mangling that preserved the tail — a re-encoding, a CRLF round-trip —
  // would pass. `${#V}` makes bash count what it actually received, so the
  // assertion is byte-exact delivery of the payload AND that the tail ran.
  const script = `V='${"x".repeat(PAYLOAD)}'\necho "LEN=\${#V}"\n`;
  // Measured, not asserted: the name below reports the real size of what was
  // spawned, so it cannot drift from the literal above.
  const size = Buffer.byteLength(script);
  let delivered = "";
  let err = "";
  try {
    delivered = runBash(script, { encoding: "utf8" });
  } catch (e) {
    err = `${e.stdout || ""}${e.stderr || ""}`.trim().split("\n")[0] || String(e);
  }
  check(
    `a ${size}-byte script reaches bash whole, all ${PAYLOAD} payload bytes intact`,
    delivered.trim() === `LEN=${PAYLOAD}`,
    err || `got ${JSON.stringify(delivered.slice(0, 80))}`,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall embedded shell/Python expectations hold");
