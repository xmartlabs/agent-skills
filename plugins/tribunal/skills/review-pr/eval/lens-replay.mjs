#!/usr/bin/env node
// lens-replay.mjs — replay one PR's review lenses offline, n times, and score
// what a pipeline would have kept.
//
// Rung 1 of the detection protocol: cheaper than a full A/B (no panel, no
// critic, no synthesis) and closer to production than a hand-written prompt.
// Hand-written single-lens replays fed the lens a focus that named the defect
// class and scored lens output directly, so a finding filed outside the diff
// or rated nit counted as caught although the pipeline drops both before the
// verdict. This runner takes every piece from the shipping code instead:
//
//   prompt text   workflow LENS-PROMPT region (byte-identical to a Workflow lens)
//   foci          workflow LENS-FOCI region (selectFoci) over the profile's foci
//   lens count    the Workflow's budget (build-lens-prompts workflowLensBudget) or --lenses / --foci
//   dependents    rust/.../js/blast-radius.mjs rendered by the DEPENDENTS-BLOCK region
//   schema        the workflow's FINDINGS_SCHEMA, requested as a JSON block
//
// Policy is pinned by the caller: pass the methodology, conventions and
// profile as they were at the base the original review read, or the replay
// measures today's rules against yesterday's code. The PR thread is never
// read (the context block is empty), so in-thread decisions cannot steer it.
//
// It does not reproduce: the Workflow's churn ladder and file partition (PRs
// over 12 files), the in-context path's own lens prose, the panel, the critic.
// Scoring is pipeline-level only as far as those gates are offline-computable:
// out-of-diff drop and needsPanel's nit rule.
//
// Usage:
//   node eval/lens-replay.mjs --repo-dir <clone> --repo <owner/name> --pr <n>
//     --sha <reviewed sha> --base <base sha> --profile <review-profile.json>
//     --methodology <assembled file> [--conventions <file>]
//     [--plan workflow | --lenses <n> | --foci <key,key>] [--dependents]
//     [--model claude-sonnet-5-5] [--effort <level>] [--n 3] [--arm <label>]
//     [--corpus <corpus dir>] [--concurrency 3] [--max-budget-usd 2]
//     --out <dir> [--worktree-root <dir>] [--dry-run]
//   node eval/lens-replay.mjs --score <out dir> --corpus <corpus dir>

import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { matches } from "./annotation-match.mjs";
import {
  RETURN_SUFFIX,
  dispatchHeader,
  findingsSchemaOf,
  lensPromptBodies,
  loadWorkflowPieces as loadPieces,
  planLenses,
} from "../scripts/build-lens-prompts.mjs";
import { blastRadius, makeRepo } from "../../../rust/crates/tribunal-context/src/js/blast-radius.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW_SRC = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");
const SEVERITY_RANK = { nit: 0, important: 1, critical: 2 };
const LENS_AGENT = { backend: "tribunal:code-review-expert", frontend: "tribunal:fe-code-review-expert", e2e: "tribunal:e2e-code-review-expert" };
const LENS_TOOLS = ["Read", "Grep", "Glob", "Bash(git show:*)", "Bash(git ls-tree:*)", "Bash(git grep:*)", "Bash(grep:*)", "Bash(ls:*)"];

const sha256 = (s) => createHash("sha256").update(s).digest("hex");

// The prompt pieces are the ones scripts/build-lens-prompts.mjs ships to the
// in-context path; the replay reads them from the unbuilt source so an edit can
// be measured before it is built.
export const loadWorkflowPieces = (src = readFileSync(WORKFLOW_SRC, "utf8")) => loadPieces(src);
export { findingsSchemaOf, planLenses };

// Raw-decode every JSON value in a free-text result and keep the one that
// carries findings: regex parsers miss nested verifiedBy objects.
function balancedEnd(s, i) {
  let depth = 0, inString = false;
  for (let j = i; j < s.length; j++) {
    const c = s[j];
    if (inString) {
      if (c === "\\") j++;
      else if (c === '"') inString = false;
    } else if (c === '"') inString = true;
    else if (c === "{" || c === "[") depth++;
    else if ((c === "}" || c === "]") && --depth === 0) return j + 1;
  }
  return -1;
}

export function findingsFromText(text) {
  const s = String(text || "");
  for (let i = 0; i < s.length; i++) {
    if (s[i] !== "{" && s[i] !== "[") continue;
    const end = balancedEnd(s, i);
    if (end === -1) continue;
    let v;
    try {
      v = JSON.parse(s.slice(i, end));
    } catch {
      continue;
    }
    if (v && Array.isArray(v.findings)) return { findings: v.findings, notReached: v.notReached || [] };
    if (Array.isArray(v) && v.length && v.every((x) => x && typeof x === "object" && "severity" in x))
      return { findings: v, notReached: [] };
    i = end - 1;
  }
  return null;
}


function git(dir, args) {
  return execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", maxBuffer: 64 << 20 });
}

function prepare(o) {
  mkdirSync(o.out, { recursive: true });
  const wtRoot = o.worktreeRoot || o.out;
  mkdirSync(wtRoot, { recursive: true });
  const wt = join(wtRoot, `wt-${o.sha.slice(0, 8)}`);
  if (!existsSync(wt)) {
    // A deleted output folder leaves its worktree registered, and add refuses it.
    git(o.repoDir, ["worktree", "prune"]);
    git(o.repoDir, ["worktree", "add", "--detach", wt, o.sha]);
  }
  const head = git(wt, ["rev-parse", "HEAD"]).trim();
  if (head !== o.sha && !head.startsWith(o.sha)) throw new Error(`worktree ${wt} is at ${head}, not ${o.sha}`);
  const diff = git(wt, ["diff", `${o.base}...${head}`]);
  const diffPath = join(o.out, "diff.patch");
  writeFileSync(diffPath, diff);
  const files = git(wt, ["diff", "--name-only", `${o.base}...${head}`]).split("\n").filter(Boolean);
  let additions = 0, deletions = 0;
  for (const l of git(wt, ["diff", "--numstat", `${o.base}...${head}`]).split("\n").filter(Boolean)) {
    const [a, d] = l.split("\t");
    additions += Number(a) || 0;
    deletions += Number(d) || 0;
  }
  return { wt, head, diff, diffPath, files, additions, deletions };
}

function runOne({ prompt, cwd, model, effort, agent, addDirs, budget }) {
  const args = [
    "-p", "--agent", agent, "--model", model, "--no-session-persistence", "--strict-mcp-config",
    "--setting-sources", "user", "--permission-mode", "dontAsk", "--output-format", "json",
    "--max-budget-usd", String(budget),
    "--allowedTools", ...LENS_TOOLS, ...addDirs.flatMap((d) => ["--add-dir", d]),
  ];
  if (effort) args.push("--effort", effort);
  return new Promise((res) => {
    const child = spawn("claude", args, { cwd, stdio: ["pipe", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("close", (code) => res({ code, out, err }));
    child.stdin.end(prompt);
  });
}

async function pool(tasks, n) {
  const results = new Array(tasks.length);
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(n, tasks.length) }, async () => {
      while (next < tasks.length) {
        const i = next++;
        results[i] = await tasks[i]();
      }
    }),
  );
  return results;
}

function summarizeResult(raw) {
  let d;
  try {
    d = JSON.parse(raw.out);
  } catch {
    return { ok: false, error: `unparseable output (exit ${raw.code}): ${raw.err.slice(0, 300)}` };
  }
  const parsed = d.structured_output && Array.isArray(d.structured_output.findings)
    ? { findings: d.structured_output.findings, notReached: d.structured_output.notReached || [] }
    : findingsFromText(d.result);
  return {
    ok: Boolean(parsed) && !d.is_error,
    error: d.is_error
      ? `API error ${d.api_error_status ?? ""}: ${String(d.result || "").slice(0, 200)}`
      : parsed
        ? null
        : "no findings JSON in the result",
    models: Object.keys(d.modelUsage || {}),
    costUsd: d.total_cost_usd ?? null,
    turns: d.num_turns ?? null,
    findings: parsed ? parsed.findings : [],
    notReached: parsed ? parsed.notReached : [],
  };
}

// Offline pipeline gates: a finding filed on a file outside the PR is dropped
// before any panel (fileInDiff), and a nit never reaches the panel or the
// verdict (needsPanel). Both decide whether a seen defect can block.
export function gate(f, files) {
  const inDiff = files.some((p) => p === f.file || p.endsWith(f.file || "\0") || String(f.file || "").endsWith(p));
  const rank = SEVERITY_RANK[String(f.severity || "").toLowerCase()] ?? 0;
  return { inDiff, raised: rank >= 1, canBlock: inDiff && rank >= 1 };
}

export function scoreReplicates(replicates, entry, files) {
  const anns = [
    ...(entry.knownReal || []).map((a) => ({ ...a, bucket: "knownReal" })),
    ...(entry.undecidable || []).map((a) => ({ ...a, bucket: "undecidable" })),
  ];
  const ok = replicates.filter((r) => r.ok);
  const rows = anns.map((a) => {
    const per = ok.map((r) => {
      const hits = r.findings.filter((f) => matches(f, a)).map((f) => ({ ...f, ...gate(f, files) }));
      return {
        seen: hits.length > 0,
        raised: hits.some((h) => h.raised),
        canBlock: hits.some((h) => h.canBlock),
        severities: hits.map((h) => h.severity),
      };
    });
    const k = (key) => per.filter((p) => p[key]).length;
    return { id: a.id, bucket: a.bucket, expectedSeverity: a.expectedSeverity || null, n: ok.length, seen: k("seen"), raised: k("raised"), canBlock: k("canBlock"), per };
  });
  const otherImportants = ok.map((r) =>
    r.findings.filter((f) => gate(f, files).canBlock && !anns.some((a) => matches(f, a))).length,
  );
  return { n: ok.length, failed: replicates.length - ok.length, annotations: rows, unmatchedBlockingPerRun: otherImportants };
}

function parseArgs(argv) {
  const o = { n: 3, concurrency: 3, model: "claude-sonnet-5-5", budget: 2, plan: "workflow" };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--repo-dir") o.repoDir = resolve(v());
    else if (k === "--repo") o.repo = v();
    else if (k === "--pr") o.pr = Number(v());
    else if (k === "--sha") o.sha = v();
    else if (k === "--base") o.base = v();
    else if (k === "--profile") o.profile = resolve(v());
    else if (k === "--methodology") o.methodology = resolve(v());
    else if (k === "--conventions") o.conventions = resolve(v());
    else if (k === "--plan") o.plan = v();
    else if (k === "--lenses") o.lenses = Number(v());
    else if (k === "--foci") o.foci = v().split(",").filter(Boolean);
    else if (k === "--dependents") o.dependents = true;
    else if (k === "--model") o.model = v();
    else if (k === "--effort") o.effort = v();
    else if (k === "--n") o.n = Number(v());
    else if (k === "--arm") o.arm = v();
    else if (k === "--corpus") o.corpus = resolve(v());
    else if (k === "--concurrency") o.concurrency = Number(v());
    else if (k === "--max-budget-usd") o.budget = Number(v());
    else if (k === "--out") o.out = resolve(v());
    else if (k === "--worktree-root") o.worktreeRoot = resolve(v());
    else if (k === "--dry-run") o.dryRun = true;
    else if (k === "--score") o.score = resolve(v());
    else throw new Error(`unknown argument ${k}`);
  }
  return o;
}

function corpusEntry(dir, pr) {
  const p = join(dir, `pr-${pr}.json`);
  return existsSync(p) ? JSON.parse(readFileSync(p, "utf8")) : null;
}

function report(manifest, score) {
  console.log(`\n${manifest.arm}  PR #${manifest.pr} @ ${manifest.sha.slice(0, 8)}  ${manifest.plan.mode} x${manifest.plan.lensCount}  n=${score.n}${score.failed ? ` (${score.failed} failed)` : ""}`);
  for (const a of score.annotations)
    console.log(`  ${a.bucket === "knownReal" ? "real" : "undc"}  ${a.id}: seen ${a.seen}/${a.n}  important+ ${a.raised}/${a.n}  can-block ${a.canBlock}/${a.n}  (expected ${a.expectedSeverity || "?"})`);
  console.log(`  unmatched can-block findings per run: ${JSON.stringify(score.unmatchedBlockingPerRun)}`);
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  if (o.score) {
    const manifest = JSON.parse(readFileSync(join(o.score, "manifest.json"), "utf8"));
    const entry = o.corpus && corpusEntry(o.corpus, manifest.pr);
    if (!entry) throw new Error(`no corpus entry for PR ${manifest.pr}`);
    const s = scoreReplicates(manifest.replicates, entry, manifest.files);
    report(manifest, s);
    return;
  }
  for (const k of ["repoDir", "repo", "pr", "sha", "base", "profile", "methodology", "out"])
    if (!o[k]) throw new Error(`--${k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)} is required`);
  const profile = JSON.parse(readFileSync(o.profile, "utf8"));
  const pieces = loadWorkflowPieces();
  const ctx = prepare(o);
  const plan = planLenses({ ...ctx, profile, plan: o.plan, lenses: o.lenses, foci: o.foci, selectFoci: pieces.selectFoci });
  const br = o.dependents ? blastRadius(ctx.diff, makeRepo(ctx.wt)) : null;
  const dependents = br ? pieces.dependentsBlock(br) : "";
  const jira = (ctx.files.length && /\b[A-Z][A-Z0-9]+-\d+\b/.exec(git(ctx.wt, ["log", "-1", "--format=%s %b"])))?.[0] || null;
  const header = dispatchHeader(o.methodology, o.conventions);
  const prompts = lensPromptBodies({
    pieces,
    foci: plan.foci,
    prNumber: o.pr,
    repo: o.repo,
    diffPath: ctx.diffPath,
    worktree: ctx.wt,
    headSha: ctx.head,
    profile,
    dependents,
    jiraTicket: jira,
  }).map((body) => header + body + RETURN_SUFFIX(pieces.findingsSchema));
  const arm = o.arm || `${o.plan}${o.dependents ? "+dep" : ""}`;
  prompts.forEach((p, i) => writeFileSync(join(o.out, `prompt-${arm}-lens${i + 1}.txt`), p));
  const agent = LENS_AGENT[profile.reviewer];
  if (!agent) throw new Error(`no lens agent for reviewer kind ${profile.reviewer}`);
  const addDirs = [...new Set([ctx.wt, dirname(o.methodology), o.conventions && dirname(o.conventions), o.out].filter(Boolean))];
  const manifest = {
    arm, repo: o.repo, pr: o.pr, sha: ctx.head, base: o.base, files: ctx.files,
    additions: ctx.additions, deletions: ctx.deletions, plan, agent, model: o.model, effort: o.effort || null,
    methodology: { path: o.methodology, sha256: sha256(readFileSync(o.methodology)) },
    conventions: o.conventions ? { path: o.conventions, sha256: sha256(readFileSync(o.conventions)) } : null,
    profile: { path: o.profile, sha256: sha256(readFileSync(o.profile)) },
    dependents: br ? { total: br.total, shown: (br.callers || []).length } : null,
    workflowSrcSha256: sha256(readFileSync(WORKFLOW_SRC)),
    promptSha256: prompts.map(sha256),
    threadNeutralized: true,
    returnChannel: "text-json (StructuredOutput unavailable under claude -p --agent)",
    replicates: [],
  };
  if (o.dryRun) {
    writeFileSync(join(o.out, "manifest.json"), JSON.stringify(manifest, null, 2));
    console.log(`dry run: ${prompts.length} prompt(s) in ${o.out}; plan ${JSON.stringify(plan)}`);
    return;
  }
  const tasks = [];
  for (let r = 1; r <= o.n; r++)
    prompts.forEach((prompt, li) =>
      tasks.push(async () => {
        const raw = await runOne({ prompt, cwd: o.repoDir, model: o.model, effort: o.effort, agent, addDirs, budget: o.budget });
        writeFileSync(join(o.out, `out-${arm}-r${r}-lens${li + 1}.json`), raw.out || raw.err);
        return { r, li, ...summarizeResult(raw) };
      }),
    );
  const results = await pool(tasks, o.concurrency);
  for (let r = 1; r <= o.n; r++) {
    const lenses = results.filter((x) => x.r === r);
    manifest.replicates.push({
      replicate: r,
      ok: lenses.every((l) => l.ok),
      errors: lenses.filter((l) => !l.ok).map((l) => `lens${l.li + 1}: ${l.error}`),
      models: [...new Set(lenses.flatMap((l) => l.models || []))],
      costUsd: lenses.reduce((s, l) => s + (l.costUsd || 0), 0),
      turns: lenses.map((l) => l.turns),
      findings: lenses.flatMap((l) => (l.findings || []).map((f) => ({ ...f, lens: `lens${l.li + 1}` }))),
      notReached: lenses.flatMap((l) => l.notReached || []),
    });
  }
  writeFileSync(join(o.out, "manifest.json"), JSON.stringify(manifest, null, 2));
  const entry = o.corpus && corpusEntry(o.corpus, o.pr);
  if (entry) report(manifest, scoreReplicates(manifest.replicates, entry, manifest.files));
  else console.log(`wrote ${join(o.out, "manifest.json")} (no corpus entry to score against)`);
}

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly)
  main().catch((e) => {
    console.error(`lens-replay: ${e.message}`);
    process.exit(1);
  });
