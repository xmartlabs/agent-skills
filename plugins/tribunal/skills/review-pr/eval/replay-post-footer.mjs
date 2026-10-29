#!/usr/bin/env node
// replay-post-footer.mjs — regression test for the posted-comment metadata
// footer: scripts/build-post-footer.mjs, scripts/locate-last-row.mjs and
// post-review.mjs's composeBody().
//
// Covers, in order: pure formatting functions in isolation (no subprocess);
// the Workflow-mode CLI reading a sandboxed metrics.jsonl via
// TRIBUNAL_DATA_DIR (the same env-var override eval/paths.mjs already
// supports); the in-context-mode CLI, proving it never touches
// metrics.jsonl at all; the shared pr/headSha mismatch guard exercised
// through BOTH build-post-footer.mjs and persist-review-record.mjs with the
// same bad --pr — this is the first replay coverage of
// persist-review-record.mjs's own mismatch guard (eval/replay-persist.mjs
// tests a different, older guard inside the Workflow's own persist-metrics
// step); and composeBody's footer-survives-the-Draft-comments-strip behavior.
//
// Usage: node eval/replay-post-footer.mjs [--verbose]

import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import {
  formatTokenCount,
  formatWallClock,
  formatCodexModel,
  buildFooterLine,
  engineLabel,
  ownPluginVersion,
} from "../scripts/build-post-footer.mjs";
import { composeBody } from "../scripts/post-review.mjs";
import { FIXTURES_DIR } from "./paths.mjs";
import { dataId } from "./data-ids.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(FIXTURES_DIR, "post-footer");
const BUILD_POST_FOOTER = join(HERE, "..", "scripts", "build-post-footer.mjs");
const PERSIST_REVIEW_RECORD = join(HERE, "..", "scripts", "persist-review-record.mjs");
const verbose = process.argv.includes("--verbose");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- 1 · pure formatting -----------------------------------------------

console.log("pure formatting");

check("formatTokenCount: 48064673 -> 48.1M", formatTokenCount(48_064_673) === "48.1M");
check("formatTokenCount: 349032 -> 349.0K", formatTokenCount(349_032) === "349.0K");
check("formatTokenCount: 999 -> 999 (no unit)", formatTokenCount(999) === "999");
check(
  "formatTokenCount: 999949 -> 999.9K (just under the M boundary)",
  formatTokenCount(999_949) === "999.9K",
  formatTokenCount(999_949)
);
check(
  "formatTokenCount: 999950 -> 1.0M (at the M boundary)",
  formatTokenCount(999_950) === "1.0M",
  formatTokenCount(999_950)
);

check("formatWallClock: 2998 -> 50m", formatWallClock(2998) === "50m", formatWallClock(2998));
check("formatWallClock: 45 -> 45s", formatWallClock(45) === "45s");
check("formatWallClock: 90 -> 2m", formatWallClock(90) === "2m");

check("formatCodexModel: null -> null", formatCodexModel(null) === null);
check("formatCodexModel: string -> itself", formatCodexModel("gpt-5.4") === "gpt-5.4");
check(
  "formatCodexModel: array -> joined",
  formatCodexModel(["gpt-5.4", "gpt-5.6-sol"]) === "gpt-5.4, gpt-5.6-sol"
);
check("formatCodexModel: empty array -> null", formatCodexModel([]) === null);

const scenarioA = buildFooterLine({
  harnessVersion: "pr-harness-49366450a283",
  reviewMode: "full",
  codexModelUsed: "gpt-5.4",
  agentCount: 36,
  inputTokens: 48_064_673,
  outputTokens: 349_032,
  wallClockSeconds: 2998,
  timestamp: "2026-07-31T15:26:13Z",
});
check(
  "buildFooterLine: scenario A (full Workflow row)",
  scenarioA ===
    "<sub>tribunal · pr-harness-49366450a283 · full review · codex gpt-5.4 · 36 agents · 48.1M in / ≥349.0K out tokens · 50m wall · 2026-07-31T15:26:13Z</sub>",
  scenarioA
);

const scenarioB = buildFooterLine({
  harnessVersion: "pr-harness-49366450a283",
  reviewMode: "incremental",
  agentCount: 9,
  timestamp: "2026-07-31T18:04:07Z",
});
check(
  "buildFooterLine: scenario B (in-context, no tokens, no codex)",
  scenarioB ===
    "<sub>tribunal · pr-harness-49366450a283 · incremental review · 9 agents · 2026-07-31T18:04:07Z</sub>",
  scenarioB
);

const scenarioC = buildFooterLine({
  harnessVersion: "pr-harness-49366450a283",
  reviewMode: "full",
  codexModelUsed: ["gpt-5.4", "gpt-5.6-sol"],
  agentCount: 22,
  inputTokens: 12_400_000,
  outputTokens: 88_500,
  timestamp: "2026-07-31T09:12:44Z",
});
check(
  "buildFooterLine: scenario C (mid-run codex model change, no wall clock)",
  scenarioC ===
    "<sub>tribunal · pr-harness-49366450a283 · full review · codex gpt-5.4, gpt-5.6-sol · 22 agents · 12.4M in / ≥88.5K out tokens · 2026-07-31T09:12:44Z</sub>",
  scenarioC
);

// agentCost.outputTokens comes from subagent transcripts, whose output_tokens is
// a placeholder (2026-10-17): the footer must present it as a lower bound and
// must not mark the exact input side the same way.
check("buildFooterLine: output is printed as a lower bound (≥Y out)", / · 48\.1M in \/ ≥349\.0K out tokens · /.test(scenarioA), scenarioA);
check("buildFooterLine: input is not marked as a bound", !/≥48\.1M in/.test(scenarioA), scenarioA);

{
  const singular = buildFooterLine({
    harnessVersion: "pr-harness-x",
    reviewMode: "full",
    agentCount: 1,
    timestamp: "t",
  });
  check("buildFooterLine: singular '1 agent', not '1 agents'", singular.includes(" · 1 agent · "), singular);
}

{
  let threw = false;
  try {
    buildFooterLine({ harnessVersion: "x", reviewMode: "bogus", agentCount: 1, timestamp: "t" });
  } catch {
    threw = true;
  }
  check("buildFooterLine: throws on invalid reviewMode", threw);
}
{
  let threw = false;
  try {
    buildFooterLine({ reviewMode: "full", agentCount: 1, timestamp: "t" });
  } catch {
    threw = true;
  }
  check("buildFooterLine: throws on missing harnessVersion", threw);
}

// The engine label opens every footer the CLI prints: which engine ran the
// review and from which plugin version.
const OWN_VERSION = ownPluginVersion();
const OWN_MAJOR = `v${String(OWN_VERSION).split(".")[0]}`;
const skillFooter = (line) => line.replace("<sub>tribunal ·", `<sub>tribunal ${OWN_MAJOR} (skill) ·`);
check("ownPluginVersion: reads this plugin's plugin.json", typeof OWN_VERSION === "string" && /^\d+\./.test(OWN_VERSION), String(OWN_VERSION));
check("engineLabel: a Workflow row (no engine) is the skill at this plugin's version", engineLabel({}, "105.0.0") === "v105 (skill)", engineLabel({}, "105.0.0"));
check("engineLabel: a Rust row names its build", engineLabel({ engine: "rust", engineVersion: "rust-abc", pluginVersion: "106.0.0" }, "105.0.0") === "v106 (rust rust-abc)");
check("engineLabel: the row's pluginVersion wins over the script's", engineLabel({ pluginVersion: "99.1.0" }, "105.0.0") === "v99 (skill)");
check("engineLabel: no version anywhere still names the engine", engineLabel({ engine: "rust" }, null) === "(rust)");
check("buildFooterLine: the engine label follows 'tribunal'", buildFooterLine({ engine: "v105 (skill)", harnessVersion: "h", reviewMode: "full", agentCount: 2, timestamp: "t" }) === "<sub>tribunal v105 (skill) · h · full review · 2 agents · t</sub>");

// --- 2 · Workflow-mode CLI, sandboxed metrics.jsonl ---------------------

console.log("\nWorkflow-mode CLI (TRIBUNAL_DATA_DIR-sandboxed)");

function sandboxWith(fixtureFile) {
  const dir = mkdtempSync(join(tmpdir(), "post-footer-"));
  cpSync(join(FIXTURES, fixtureFile), join(dir, "metrics.jsonl"));
  return dir;
}

function runFooter(args, env = {}) {
  try {
    const out = execFileSync("node", [BUILD_POST_FOOTER, ...args], {
      encoding: "utf8",
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { ok: true, out: out.trim(), err: "" };
  } catch (e) {
    return { ok: false, out: "", err: String(e.stderr || e.message).trim() };
  }
}

{
  const dir = sandboxWith("workflow-full.jsonl");
  const r = runFooter(
    ["--mode", "workflow", "--pr", "1947", "--head-sha", dataId("57db7090f376", 40), "--agent-count", "36"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  check(
    "workflow-full.jsonl: matches scenario A, reading the LAST matching row not the first",
    r.ok && r.out === skillFooter(scenarioA),
    r.ok ? r.out : r.err
  );
}

{
  const dir = sandboxWith("workflow-array-model.jsonl");
  const r = runFooter(
    ["--mode", "workflow", "--pr", "1968", "--head-sha", "aa11bb22cc33dd44ee55ff660011223344556677", "--agent-count", "22"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  check("workflow-array-model.jsonl: matches scenario C", r.ok && r.out === skillFooter(scenarioC), r.ok ? r.out : r.err);
}

{
  const dir = sandboxWith("workflow-degraded-no-cost.jsonl");
  const r = runFooter(
    ["--mode", "workflow", "--pr", "1935", "--head-sha", dataId("28a270af5861", 40), "--agent-count", "5"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  const expected = skillFooter(
    "<sub>tribunal · pr-harness-49366450a283 · incremental review · 5 agents · 2026-07-31T11:00:00Z</sub>");
  check(
    "workflow-degraded-no-cost.jsonl: no codex/tokens/wall-clock segments, still succeeds",
    r.ok && r.out === expected,
    r.ok ? r.out : r.err
  );
}

{
  const dir = mkdtempSync(join(tmpdir(), "post-footer-rust-"));
  writeFileSync(join(dir, "metrics.jsonl"), JSON.stringify({ pr: 77, headSha: "abc", harnessVersion: "pr-harness-x", reviewMode: "full", ts: "2026-10-23T11:12:35Z", engine: "rust", engineVersion: "rust-0123456789ab", pluginVersion: "106.0.0" }) + "\n");
  const r = runFooter(["--mode", "workflow", "--pr", "77", "--head-sha", "abc", "--agent-count", "17"], { TRIBUNAL_DATA_DIR: dir });
  check(
    "a Rust row's footer names the engine build and its plugin version",
    r.ok && r.out === "<sub>tribunal v106 (rust rust-0123456789ab) · pr-harness-x · full review · 17 agents · 2026-10-23T11:12:35Z</sub>",
    r.ok ? r.out : r.err
  );
}

{
  const dir = sandboxWith("mismatch.jsonl");
  const r = runFooter(
    ["--mode", "workflow", "--pr", "9999", "--head-sha", "deadbeef", "--agent-count", "5"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  check(
    "mismatched --pr: refuses, prints nothing, stderr says 'refusing'",
    !r.ok && r.out === "" && /refusing/.test(r.err),
    r.err
  );
}

{
  const dir = sandboxWith("mismatch.jsonl");
  const r = runFooter(
    ["--mode", "workflow", "--pr", "1200", "--head-sha", "wrongsha", "--agent-count", "5"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  check(
    "matching --pr but mismatched --head-sha: refuses",
    !r.ok && r.out === "" && /refusing/.test(r.err),
    r.err
  );
}

{
  const dir = mkdtempSync(join(tmpdir(), "post-footer-empty-"));
  cpSync(join(FIXTURES, "empty.jsonl"), join(dir, "metrics.jsonl"));
  const r = runFooter(["--mode", "workflow", "--pr", "1", "--head-sha", "x", "--agent-count", "1"], {
    TRIBUNAL_DATA_DIR: dir,
  });
  check("empty metrics.jsonl: refuses, prints nothing", !r.ok && r.out === "" && /refusing/.test(r.err), r.err);
}

// --- 3 · in-context-mode CLI, never touches metrics.jsonl ---------------

console.log("\nin-context-mode CLI");

{
  // A directory that does not exist at all — if the CLI ever tried to read
  // metrics.jsonl in this mode, this would surface as a failure.
  const missingDir = join(tmpdir(), `post-footer-missing-${process.pid}-${Date.now()}`);
  const r = runFooter(
    [
      "--mode", "in-context",
      "--harness-version", "pr-harness-49366450a283",
      "--review-mode", "incremental",
      "--agent-count", "9",
      "--lenses-planned", "2", "--lenses-dispatched", "2", "--critic-planned", "false", "--critic-ran", "false",
      "--timestamp", "2026-07-31T18:04:07Z",
    ],
    { TRIBUNAL_DATA_DIR: missingDir }
  );
  check(
    "in-context mode never touches metrics.jsonl (DATA_DIR doesn't exist)",
    r.ok && r.out === skillFooter(scenarioB),
    r.ok ? r.out : r.err
  );
}

{
  const r = runFooter([
    "--mode", "in-context",
    "--harness-version", "pr-harness-49366450a283",
    "--review-mode", "full",
    "--agent-count", "22",
    "--lenses-planned", "2", "--lenses-dispatched", "2", "--critic-planned", "false", "--critic-ran", "false",
    "--codex-model-json", JSON.stringify(["gpt-5.4", "gpt-5.6-sol"]),
    "--timestamp", "2026-07-31T09:12:44Z",
    "--input-tokens", "999999999",
  ]);
  // Same identity as scenario C (array codex model, same version/agents/ts),
  // but with NO token segment — in-context mode has no --input-tokens flag at
  // all, so the bogus one above is silently ignored rather than fabricating a
  // tokens segment scenario C's Workflow-mode data actually backs.
  const expected = skillFooter(
    "<sub>tribunal · pr-harness-49366450a283 · full review · codex gpt-5.4, gpt-5.6-sol · 22 agents · 2026-07-31T09:12:44Z</sub>");
  check(
    "in-context mode: array codex model, and an unrecognized flag is simply ignored (no token flags exist in this mode)",
    r.ok && r.out === expected,
    r.ok ? r.out : r.err
  );
}

{
  const r = runFooter(["--mode", "in-context", "--review-mode", "full", "--agent-count", "1"]);
  check("in-context mode: missing --harness-version is a usage error", !r.ok);
}

{
  const r = runFooter([
    "--mode", "in-context",
    "--harness-version", "pr-harness-x",
    "--review-mode", "bogus",
    "--agent-count", "1",
    "--lenses-planned", "2", "--lenses-dispatched", "2", "--critic-planned", "false", "--critic-ran", "false",
  ]);
  check("in-context mode: invalid --review-mode is rejected", !r.ok);
}

{
  const base = ["--mode", "in-context", "--harness-version", "pr-harness-x", "--review-mode", "full", "--agent-count", "3", "--timestamp", "2026-10-21T00:00:00Z"];
  const plan = (planned, dispatched, cp, cr) => ["--lenses-planned", planned, "--lenses-dispatched", dispatched, "--critic-planned", cp, "--critic-ran", cr];
  check("in-context mode: missing plan flags is a usage error", !runFooter(base).ok);
  const fewer = runFooter([...base, ...plan("2", "1", "false", "false")]);
  check("in-context mode: fewer lenses than planned refuses the footer", !fewer.ok && /1 lens\(es\) dispatched, 2 planned/.test(fewer.err), fewer.err);
  const noCritic = runFooter([...base, ...plan("2", "2", "true", "false")]);
  check("in-context mode: a planned critic that did not run refuses the footer", !noCritic.ok && /completeness critic/.test(noCritic.err), noCritic.err);
  const declared = runFooter([...base, ...plan("2", "1", "false", "false"), "--plan-deviation", "second lens crashed twice"]);
  check("in-context mode: a declared deviation lets the footer through", declared.ok, declared.err);
  const blank = runFooter([...base, ...plan("2", "1", "false", "false"), "--plan-deviation", "  "]);
  check("in-context mode: a blank deviation is not a reason", !blank.ok);
  check("in-context mode: more lenses than planned is fine", runFooter([...base, ...plan("2", "3", "false", "true")]).ok);
}

// --- 4 · shared mismatch guard, both call sites --------------------------

console.log("\nshared pr/headSha guard — build-post-footer.mjs and persist-review-record.mjs agree");

{
  const dir = sandboxWith("mismatch.jsonl");
  const footerResult = runFooter(
    ["--mode", "workflow", "--pr", "42", "--head-sha", "x", "--agent-count", "1"],
    { TRIBUNAL_DATA_DIR: dir }
  );
  let persistResult;
  try {
    const out = execFileSync("node", [PERSIST_REVIEW_RECORD, "--pr", "42", "--posted", "true", "--verdict", "COMMENT"], {
      encoding: "utf8",
      env: { ...process.env, TRIBUNAL_DATA_DIR: dir },
      stdio: ["ignore", "pipe", "pipe"],
    });
    persistResult = { ok: true, out: out.trim(), err: "" };
  } catch (e) {
    persistResult = { ok: false, out: "", err: String(e.stderr || e.message).trim() };
  }
  check(
    "both refuse the same bad --pr, same 'refusing: last row is pr' message shape",
    !footerResult.ok &&
      !persistResult.ok &&
      /refusing: last row is pr/.test(footerResult.err) &&
      /refusing: last row is pr/.test(persistResult.err),
    `footer: ${footerResult.err} | persist: ${persistResult.err}`
  );
}

// --- 5 · composeBody: footer survives the Draft-comments strip ----------

console.log("\ncomposeBody");

{
  const reportWithDraft =
    "## Peer review: PR #1\n\n### Blocking\n- nothing\n\n### Draft comments (ready to paste)\nsome draft text";
  const footer = "<sub>tribunal · pr-harness-x · full review · 5 agents · t</sub>";
  const body = composeBody(reportWithDraft, footer, "APPROVE");
  check("composeBody: Draft comments section is gone", !body.includes("Draft comments"));
  check("composeBody: footer is present and is the true end", body.trim().endsWith(footer));
  check("composeBody: verdict banner leads the body", body.startsWith("### ✅ Approved"));
}
{
  const reportNoDraft = "## Peer review: PR #1\n\n### Blocking\n- nothing\n";
  const footer = "<sub>tribunal · pr-harness-x · full review · 5 agents · t</sub>";
  const body = composeBody(reportNoDraft, footer, "REQUEST_CHANGES");
  check("composeBody: with no Draft comments section, footer still lands at the true end", body.trim().endsWith(footer));
  check("composeBody: request-changes banner leads the body", body.startsWith("### 🔴 Changes requested"));
}
{
  let threw = false;
  try {
    composeBody("report", "<sub>t</sub>", "NOT_A_VERDICT");
  } catch {
    threw = true;
  }
  check("composeBody: throws on an unknown verdict rather than posting an unbannered body", threw);
}

if (verbose) console.log(`\n(${FIXTURES})`);

if (failures.length) {
  console.log("\nfailures:");
  for (const f of failures) console.log(`  ${f}`);
  process.exit(1);
}
console.log("\nall post-footer expectations hold");
