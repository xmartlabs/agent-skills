#!/usr/bin/env node
// fetch-pr-context.mjs — the mechanical half of the Workflow's fetch-pr step, in
// one process.
//
// fetch-pr used to hand a haiku agent six `gh` commands and a page of field
// rules. The agent ran them one Bash call at a time, re-typed the gh JSON into
// PR_CONTEXT_SCHEMA, and every run waited on it: nothing downstream starts
// until fetch-pr returns. On BE #2094 (2026-10-16, wf_57606a33-7eb and
// wf_03d6b056-3bf) it ended at +38 to +41 s while the bookkeeping dispatch
// beside it ended at +7 s, and it was the costliest mechanical label of the
// run. Every field except two is a copy or a computation, so this script
// produces them, with the gh calls in parallel.
//
// Two fields still need a model, and this script does not pretend otherwise:
// `inThreadDecisions` (which comments record a decision a reviewer must not
// re-litigate) and the `summary` of each CHANGES_REQUESTED review. The raw
// human text they are written from is printed under `raw`, so the agent reads
// it once instead of fetching it.
//
// Two defects of the prompt version are fixed here rather than carried over:
//   * The inline and thread comment calls had no --paginate, so a PR with more
//     than 30 comments lost the rest. An author's rebuttal is usually the
//     newest comment, which is the one that fell off.
//   * On a pinned run the counts came from the compare API but `fileChurn`
//     still came from `gh pr view`, i.e. the PR's CURRENT head. Both now come
//     from compare.
//
// Bot comments are returned only with --bot-comments. Their one consumer is the
// bot-vetting dispatch, which runs only under the same flag (tribunal.js
// `vetBotComments`), and the agent has to re-type every field it returns. On
// BE #2094 the CodeRabbit bodies alone are most of a 31 KB payload, re-typed on
// every run for a field nothing read.
//
// --no-diff-text keeps `context.diff` empty whatever the file count. The
// in-context path runs this script in the orchestrator's own session, and its
// Step 1 rule is that the diff must not enter that context before Step 2 hands
// it to the lenses. The patch is still written to diffPath.
//
// --relay <dir> is the Workflow's form (v81). The mechanic used to copy this
// whole line back into a schema, and on BE #1892 (2026-10-18) it was 26 KB
// without the diff, near the tool-result limit. With --relay the script writes
// `raw` to <dir>/raw.json and the bot comments to <dir>/bot-comments.json, and
// prints a checksummed relay line (lib/relay.mjs) of the mechanical context
// alone: bot comments as an index, plus the two paths, rawBytes and
// rawHasHumanText. The in-context path keeps the one-line form below.
//
// Usage:
// --thread-before <iso> keeps only the reviews and comments posted before that
// instant, so a pinned re-run of an old head reads the thread that head had.
//
// --prep <script> also prepares what the Workflow used to ask three more
// mechanics for, one after the other: the pinned checkout, the per-file diff
// sections and the blast radius. <script> is the workflow's own head-worktree
// bash, written to disk by the same dispatch with the sha left as $PREP_SHA, so
// the checkout is built at the headRefOid this script reports and nothing else.
// The results travel under `prep` in the relay; the workflow parses them with
// the same functions it applied to those dispatches and falls back to them when
// a part is missing. A deployment PR gets no prep: the run stops right after.
// --prep-sections-above <n> cuts sections only above n files, the workflow's
// threshold.
//
// Usage:
//   node fetch-pr-context.mjs --pr <n> --repo <owner/name> --deploy-branches <a,b,c> [--head-sha <sha>] [--thread-before <iso>] [--bot-comments] [--no-diff-text] [--relay <dir>] [--prep <script> [--prep-sections-above <n>]]
//
// Prints ONE line of JSON: {context, raw, errors}. `context` carries every
// PR_CONTEXT_SCHEMA field except inThreadDecisions, with priorReviews[].summary
// left "". Exit code is always 0: a failed call is reported in `errors` and
// leaves its fields at the empty values the schema rules name, never a crash
// that would leave the agent to improvise.

import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { RELAY_CRC_PREFIX, relayLines } from "../../../rust/crates/tribunal-context/src/js/relay.mjs";

const LIB = join(dirname(fileURLToPath(import.meta.url)), "../../../lib");

// Copied from workflows/tribunal.js (PRIOR-REVIEWS region), which cannot import
// it: Workflow scripts have no module access. eval/check-mirror.mjs pins the two
// copies to the same source text.
export const BOT_LOGIN =
  /(\[bot\]$)|^(coderabbitai|github-actions|dependabot|renovate)$/i;

// `diff` is non-empty only inside the window where a recipient needs the text
// in-band: more than 12 files (below that every recipient reads the whole file
// from diffPath anyway) and at most 60000 bytes (above that the tool-result
// channel truncates it, and a truncated diff is worse than an empty one).
export const DIFF_FIELD_MIN_FILES = 13;
export const DIFF_FIELD_MAX_BYTES = 60000;


export function runGh(args) {
  return new Promise((resolve) => {
    execFile(
      "gh",
      args,
      { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
      (err, stdout, stderr) => {
        if (err) {
          resolve({
            ok: false,
            stdout: "",
            detail: (String(stderr || "").trim() || String(err.message)).slice(0, 300),
          });
        } else {
          resolve({ ok: true, stdout });
        }
      },
    );
  });
}

const login = (u) => (u && typeof u === "object" ? u.login || "" : u || "");

// `gh api --paginate --slurp` returns an array of pages, each a JSON array.
function slurpedPages(stdout) {
  const pages = JSON.parse(stdout);
  return Array.isArray(pages) ? pages.flat() : [];
}

// A ticket key in any project, title first. Two leading letters keep a range
// like `A-Z0-9` in a PR body from reading as a key (BE #2191). Over 200 PRs
// of each repo it was built on, it returns what the old project-only pattern
// did on every one.
const TICKET_KEY = /\b[A-Z]{2,}[A-Z0-9]*-\d+\b/;

export function jiraTicketOf(title, body) {
  const m = String(title || "").match(TICKET_KEY) || String(body || "").match(TICKET_KEY);
  return m ? m[0] : "";
}

// With a cutoff, an item with no timestamp is dropped: nothing shows it predates
// the cutoff.
export function postedBefore(at, cutoff) {
  if (!cutoff) return true;
  const t = Date.parse(at || "");
  return Number.isFinite(t) && t < Date.parse(cutoff);
}

export async function fetchPrContext({
  prNumber,
  repo,
  headSha = "",
  threadBefore = "",
  withBotComments = false,
  withDiffText = true,
  gh = runGh,
  tmpdir = process.env.TMPDIR || "/tmp",
  writeFile = writeFileSync,
  fileSize = (p) => statSync(p).size,
  deployBranches,
}) {
  if (!Array.isArray(deployBranches))
    throw new Error("fetchPrContext: deployBranches is required (the project profile's list; [] for none)");
  const pr = String(prNumber);
  const errors = [];
  const fail = (step, res) => errors.push({ step, detail: res.detail || "unparseable output" });
  const parse = (step, res, fn) => {
    if (!res.ok) return fail(step, res), null;
    try {
      return fn(res.stdout);
    } catch {
      return fail(step, { detail: "unparseable output" }), null;
    }
  };

  const viewP = gh([
    "pr", "view", pr, "--repo", repo, "--json",
    "title,body,author,baseRefName,baseRefOid,headRefName,headRefOid,files,additions,deletions,reviews",
  ]);
  const inlineP = gh(["api", "--paginate", "--slurp", `repos/${repo}/pulls/${prNumber}/comments`]);
  const threadP = gh(["api", "--paginate", "--slurp", `repos/${repo}/issues/${prNumber}/comments`]);
  const viewerP = gh(["api", "user", "--jq", ".login"]);
  // Unpinned, the diff does not wait on anything. Pinned, it needs the base
  // commit, which only the view call returns.
  const unpinnedDiffP = headSha ? null : gh(["pr", "diff", pr, "--repo", repo]);

  const view = parse("pr-view", await viewP, (s) => JSON.parse(s)) || {};

  let diffRes;
  let compareFiles = null;
  if (headSha) {
    if (!view.baseRefOid) {
      diffRes = { ok: false, detail: "no baseRefOid from pr-view, cannot build the pinned compare" };
    } else {
      const range = `repos/${repo}/compare/${view.baseRefOid}...${headSha}`;
      const [d, c] = await Promise.all([
        gh(["api", range, "-H", "Accept: application/vnd.github.v3.diff"]),
        gh(["api", range]),
      ]);
      diffRes = d;
      const cmp = parse("compare-files", c, (s) => JSON.parse(s));
      if (cmp && Array.isArray(cmp.files)) compareFiles = cmp.files;
    }
  } else {
    diffRes = await unpinnedDiffP;
  }

  // Redirect to the file FIRST: the file is the authoritative copy, and the size
  // is read back from it rather than from the in-memory string.
  const diffPath = join(tmpdir, `peer-review-diff-${pr}.patch`);
  let writtenPath = "";
  let diffBytes = null;
  let diffText = "";
  if (diffRes.ok) {
    try {
      writeFile(diffPath, diffRes.stdout);
      writtenPath = diffPath;
      diffBytes = fileSize(diffPath);
      diffText = diffRes.stdout;
    } catch (e) {
      fail("diff-write", { detail: String(e && e.message) });
    }
  } else {
    fail(headSha ? "compare-diff" : "pr-diff", diffRes);
  }

  // Pinned: every count and the churn describe the PINNED diff. Unpinned: the
  // view's own files array, which already carries per-file additions/deletions.
  const fileRows = headSha
    ? (compareFiles || []).map((f) => ({ path: f.filename, additions: f.additions, deletions: f.deletions }))
    : (view.files || []).map((f) => ({ path: f.path, additions: f.additions, deletions: f.deletions }));
  const files = fileRows.map((f) => f.path).filter(Boolean);
  const sum = (k) => fileRows.reduce((a, f) => a + (Number(f[k]) || 0), 0);
  const additions = headSha ? sum("additions") : Number(view.additions) || 0;
  const deletions = headSha ? sum("deletions") : Number(view.deletions) || 0;
  const fileChurn = fileRows.every((f) => Number.isFinite(f.additions) && Number.isFinite(f.deletions))
    ? fileRows.map((f) => ({ file: f.path, churn: f.additions + f.deletions }))
    : [];

  const reviews = (Array.isArray(view.reviews) ? view.reviews : []).filter((r) => postedBefore(r.submittedAt, threadBefore));
  const inline = (parse("inline-comments", await inlineP, slurpedPages) || []).filter((c) => postedBefore(c.created_at, threadBefore));
  const thread = (parse("thread-comments", await threadP, slurpedPages) || []).filter((c) => postedBefore(c.created_at, threadBefore));
  const viewerRes = await viewerP;
  const viewer = viewerRes.ok ? viewerRes.stdout.trim() : (fail("viewer", viewerRes), "");

  const isBot = (who) => BOT_LOGIN.test(who);
  const botComments = !withBotComments ? [] : [
    ...inline
      .filter((c) => isBot(login(c.user)))
      .map((c) => ({ user: login(c.user), path: c.path || "", line: c.line ?? c.original_line ?? null, body: c.body || "" })),
    ...reviews
      .filter((r) => isBot(login(r.author)) && String(r.body || "").trim())
      .map((r) => ({ user: login(r.author), body: r.body })),
  ];

  const priorReviews = reviews.map((r) => ({
    author: login(r.author),
    state: r.state,
    at: r.submittedAt || "",
    summary: "",
  }));

  const context = {
    title: view.title || "",
    baseRefName: view.baseRefName || "",
    headRefName: view.headRefName || "",
    headRefOid: headSha || view.headRefOid || "",
    author: login(view.author),
    jiraTicket: jiraTicketOf(view.title, view.body),
    fileCount: files.length,
    additions,
    deletions,
    files,
    fileChurn,
    diffPath: writtenPath,
    diffBytes,
    diff:
      withDiffText &&
      files.length >= DIFF_FIELD_MIN_FILES && diffBytes !== null && diffBytes <= DIFF_FIELD_MAX_BYTES
        ? diffText
        : "",
    botComments,
    viewer,
    priorReviews,
    isDeploymentPR: deployBranches.includes(view.baseRefName),
  };

  const raw = {
    // The PR description, where an author declares a stacked or follow-up PR.
    // BE #2200 (2026-10-21) said "the multi-order half is in the PR stacked on
    // this branch" only there, and the run reported that half missing.
    description:
      String(view.body || "").trim() && !isBot(login(view.author))
        ? [{ author: login(view.author), body: view.body }]
        : [],
    changesRequested: reviews
      .map((r, index) => ({ index, author: login(r.author), at: r.submittedAt || "", body: r.body || "" }))
      .filter((r, i) => reviews[i].state === "CHANGES_REQUESTED"),
    humanReviews: reviews
      .filter((r) => !isBot(login(r.author)) && String(r.body || "").trim())
      .map((r) => ({ author: login(r.author), state: r.state, at: r.submittedAt || "", body: r.body })),
    threadComments: thread
      .filter((c) => !isBot(login(c.user)))
      .map((c) => ({ user: login(c.user), at: c.created_at || "", body: c.body || "" })),
    inlineComments: inline
      .filter((c) => !isBot(login(c.user)))
      .map((c) => ({ user: login(c.user), path: c.path || "", at: c.created_at || "", body: c.body || "" })),
  };

  return { context, raw, errors };
}

export function relayContext({ context, raw, errors }, dir, writeFile = writeFileSync) {
  mkdirSync(dir, { recursive: true });
  const rawText = JSON.stringify(raw);
  const rawPath = join(dir, "raw.json");
  const botCommentsPath = join(dir, "bot-comments.json");
  writeFile(rawPath, rawText);
  writeFile(botCommentsPath, JSON.stringify(context.botComments));
  return {
    ...context,
    // Which machine ran this: the workflow sizes its Bash commands for Windows.
    platform: process.platform,
    botComments: context.botComments.map((c, i) => ({
      i,
      user: c.user,
      path: c.path || "",
      line: c.line ?? null,
      bytes: Buffer.byteLength(c.body || "", "utf8"),
    })),
    botCommentsPath,
    rawPath,
    rawBytes: Buffer.byteLength(rawText, "utf8"),
    rawHasHumanText: Object.values(raw).some((list) => list.length > 0),
    errors,
  };
}

export function run(cmd, args, env = process.env) {
  return new Promise((resolve) => {
    execFile(cmd, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env }, (err, stdout, stderr) =>
      resolve({ ok: !err, stdout: String(stdout || ""), stderr: String(stderr || "") }),
    );
  });
}

// The data line of a lib script's relay, or {error} with what it printed.
export function relayData(res) {
  const lines = res.stdout.split("\n");
  const i = lines.findIndex((l) => l.startsWith(RELAY_CRC_PREFIX));
  if (i >= 0 && lines[i + 1]) {
    try {
      return JSON.parse(lines[i + 1]);
    } catch {}
  }
  return { error: (res.stdout + res.stderr).trim().slice(0, 300) || "no relay line" };
}

export function worktreePathOf(text) {
  for (const l of String(text || "").split("\n").map((s) => s.trim())) {
    if (l.startsWith("CREATED_WORKTREE ") && l.slice(17).trim()) return l.slice(17).trim();
    if (l.startsWith("CLONED:") && l.slice(7).trim()) return l.slice(7).trim();
  }
  return null;
}

// A run that dies, or a fetch-pr copy that is retried, leaves the checkout it
// made: the retry builds its own and teardown removes only the one the verified
// copy names (BE #2119 and #2167, 2026-10-21, ab7: one leak per retried run).
// Nothing identifies a run before fetch-pr, and A/B runs review the same PR side
// by side, so what is removed is decided by age alone: a review lasts minutes.
export const STALE_WORKTREE_MS = 2 * 60 * 60 * 1000;
const REVIEW_WORKTREE = /^peer-review-wt-\d+-[A-Za-z0-9]+$/;

export async function pruneStaleWorktrees({ exec = run, now = Date.now(), maxAgeMs = STALE_WORKTREE_MS, stat = statSync } = {}) {
  const list = await exec("git", ["worktree", "list", "--porcelain"]);
  if (!list.ok) return [];
  const removed = [];
  for (const line of list.stdout.split("\n")) {
    if (!line.startsWith("worktree ")) continue;
    const path = line.slice(9).trim();
    if (!REVIEW_WORKTREE.test(path.split("/").pop())) continue;
    let st;
    try {
      st = stat(path);
    } catch {
      continue;
    }
    if (now - (st.birthtimeMs || st.mtimeMs) <= maxAgeMs) continue;
    if ((await exec("git", ["worktree", "remove", "--force", path])).ok) removed.push(path);
  }
  if (removed.length) await exec("git", ["worktree", "prune"]);
  return removed;
}

export async function prepareReview(context, { worktreeScript, sectionsAbove = Infinity, pr, tmpdir = process.env.TMPDIR || "/tmp", exec = run }) {
  const prep = { worktree: null, sections: null, blastRadius: null };
  if (context.isDeploymentPR || !context.headRefOid) return prep;
  await pruneStaleWorktrees({ exec });
  const sectionsP =
    context.diffPath && context.files.length > sectionsAbove
      ? exec(process.execPath, [
          join(LIB, "section-diff.mjs"),
          context.diffPath,
          mkdtempSync(join(tmpdir, `tribunal-sections-${pr}-`)),
          String(context.additions || 0),
          String(context.deletions || 0),
        ]).then(relayData)
      : Promise.resolve(null);
  const wt = await exec("bash", [worktreeScript], { ...process.env, PREP_SHA: context.headRefOid });
  prep.worktree = wt.stdout.trim();
  const path = worktreePathOf(prep.worktree);
  if (path && context.diffPath)
    prep.blastRadius = relayData(await exec(process.execPath, [join(LIB, "../rust/crates/tribunal-context/src/js/blast-radius.mjs"), context.diffPath, path]));
  prep.sections = await sectionsP;
  return prep;
}

// The mechanic copies the whole relay into one structured answer, and haiku
// fails at that well before the tool-result limit: on BE #2167 (2026-10-21) a
// 14 KB relay, most of it blast-radius code snippets, came back as invalid JSON
// five times and the run died at fetch-pr. A blast radius of up to 4.6 KB
// relayed fine in a dispatch of its own. So the sections, then the blast
// radius, leave the relay as {error} when the line would pass this size, and
// the workflow dispatches their own relays for them, as it did before --prep.
export const PREP_RELAY_MAX_BYTES = 4000;

export function boundPrep(relayed, max = PREP_RELAY_MAX_BYTES) {
  const size = () => Buffer.byteLength(JSON.stringify(relayed), "utf8");
  for (const key of ["sections", "blastRadius"]) {
    if (size() <= max) break;
    if (relayed.prep[key] && !relayed.prep[key].error)
      relayed.prep[key] = { error: `deferred: the fetch-pr relay would pass ${max} bytes` };
  }
  return relayed;
}

function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isMainModule()) {
  const argv = process.argv.slice(2);
  const flag = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : "";
  };
  const prNumber = flag("--pr");
  const repo = flag("--repo");
  const deployIdx = argv.indexOf("--deploy-branches");
  const deployBranches =
    deployIdx >= 0 && typeof argv[deployIdx + 1] === "string" && !argv[deployIdx + 1].startsWith("--")
      ? argv[deployIdx + 1].split(",").map((b) => b.trim()).filter(Boolean)
      : null;
  const threadBefore = flag("--thread-before").trim();
  if (threadBefore && !Number.isFinite(Date.parse(threadBefore))) {
    process.stdout.write(JSON.stringify({ context: null, raw: null, errors: [{ step: "usage", detail: `--thread-before is not a date: ${threadBefore}` }] }) + "\n");
    process.exit(0);
  }
  if (!/^\d+$/.test(prNumber) || !repo || !deployBranches) {
    process.stdout.write(JSON.stringify({ context: null, raw: null, errors: [{ step: "usage", detail: "--pr <n> --repo <owner/name> --deploy-branches <a,b,c> required (the list comes from the project profile)" }] }) + "\n");
    process.exit(0);
  }
  const out = await fetchPrContext({
    prNumber,
    repo,
    headSha: flag("--head-sha").trim(),
    threadBefore,
    withBotComments: argv.includes("--bot-comments"),
    withDiffText: !argv.includes("--no-diff-text"),
    deployBranches,
  });
  const relayDir = flag("--relay");
  if (relayDir) {
    if (!out.context) {
      process.stdout.write(`FATAL: ${JSON.stringify(out.errors)}\n`);
      process.exit(1);
    }
    const relayed = relayContext(out, relayDir);
    const worktreeScript = flag("--prep");
    if (worktreeScript) {
      const above = Number(flag("--prep-sections-above"));
      relayed.prep = await prepareReview(out.context, {
        worktreeScript,
        sectionsAbove: Number.isFinite(above) && flag("--prep-sections-above") ? above : Infinity,
        pr: prNumber,
      });
      boundPrep(relayed);
    }
    process.stdout.write(relayLines(relayed));
  } else process.stdout.write(JSON.stringify(out) + "\n");
}
