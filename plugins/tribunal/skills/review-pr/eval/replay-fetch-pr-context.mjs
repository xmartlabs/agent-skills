#!/usr/bin/env node
// replay-fetch-pr-context.mjs — scripts/fetch-pr-context.mjs, driven through
// an injected `gh` so every branch runs offline.
//
// The script replaced a prompt that told a model which gh calls to make and
// how to fill each field, so each rule that prompt carried is asserted here
// against the code that now applies it: where a pinned run takes its counts,
// when `diff` carries text, which comments count as bots, and what a failed
// call leaves behind. Two of those were defects of the prompt version (no
// pagination, pinned churn read from the current head) and are pinned so they
// cannot come back.

import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { WORKFLOW } from "./paths.mjs";
// Literal: these are the deploy branches the pinned behaviour used.
const DEPLOY_BRANCHES = ["staging", "uat", "production"];
import { loadRegion } from "./extract-pure-region.mjs";
import { relayLines } from "../../../rust/crates/tribunal-context/src/js/relay.mjs";
import {
  BOT_LOGIN,
  DIFF_FIELD_MAX_BYTES,
  DIFF_FIELD_MIN_FILES,
  fetchPrContext,
  jiraTicketOf,
  postedBefore,
  boundPrep,
  PREP_RELAY_MAX_BYTES,
  prepareReview,
  pruneStaleWorktrees,
  relayContext,
  relayData,
  worktreePathOf,
} from "../scripts/fetch-pr-context.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const SCRIPT = new URL("../scripts/fetch-pr-context.mjs", import.meta.url).pathname;
const REPO = "acme/app";
const PIN = "1234567890abcdef1234567890abcdef12345678";

const fileEntry = (i, a = 1, d = 1) => ({ path: `src/f${i}.ts`, additions: a, deletions: d });
const baseView = (over = {}) => ({
  title: "[ABC-2399] Fix the thing",
  body: "Also see ABC-1",
  author: { login: "alice" },
  baseRefName: "development",
  baseRefOid: "base000",
  headRefName: "fix/thing",
  headRefOid: "livehead",
  files: [fileEntry(1, 10, 2), fileEntry(2, 3, 0)],
  additions: 13,
  deletions: 2,
  reviews: [
    { author: { login: "coderabbitai" }, state: "COMMENTED", submittedAt: "2026-09-10T00:00:00Z", body: "Actionable comments posted: 1" },
    { author: { login: "bob" }, state: "CHANGES_REQUESTED", submittedAt: "2026-09-11T00:00:00Z", body: "src/f1.ts: the guard is racy" },
    { author: { login: "carol" }, state: "APPROVED", submittedAt: "2026-09-12T00:00:00Z", body: "" },
  ],
  ...over,
});

// A fake gh that answers by the shape of the call and records every call.
function fakeGh({ view = baseView(), diff = "diff --git a/x b/x\n", compareFiles = null, inlinePages = [[]], threadPages = [[]], viewer = "me", fail = [] } = {}) {
  const calls = [];
  const gh = async (args) => {
    calls.push(args);
    const key = args.join(" ");
    const down = (name) => fail.includes(name);
    if (args[0] === "pr" && args[1] === "view") return down("view") ? { ok: false, detail: "HTTP 502" } : { ok: true, stdout: JSON.stringify(view) };
    if (args[0] === "pr" && args[1] === "diff") return down("diff") ? { ok: false, detail: "diff too large" } : { ok: true, stdout: diff };
    if (key.includes("/pulls/") && key.includes("/comments")) return down("inline") ? { ok: false, detail: "x" } : { ok: true, stdout: JSON.stringify(inlinePages) };
    if (key.includes("/issues/") && key.includes("/comments")) return down("thread") ? { ok: false, detail: "x" } : { ok: true, stdout: JSON.stringify(threadPages) };
    if (args[0] === "api" && args[1] === "user") return down("viewer") ? { ok: false, detail: "auth" } : { ok: true, stdout: `${viewer}\n` };
    if (key.includes("/compare/")) {
      if (args.includes("Accept: application/vnd.github.v3.diff")) return down("compare-diff") ? { ok: false, detail: "x" } : { ok: true, stdout: diff };
      return { ok: true, stdout: JSON.stringify({ files: compareFiles || [] }) };
    }
    return { ok: false, detail: `unexpected call: ${key}` };
  };
  return { gh, calls };
}

const tmp = () => mkdtempSync(join(tmpdir(), "fetch-pr-ctx-"));

console.log("--- unpinned: fields come from gh pr view ---");
{
  const dir = tmp();
  const { gh, calls } = fakeGh();
  const { context: c, errors } = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 77, repo: REPO, gh, tmpdir: dir });
  check("no errors on a clean fetch", errors.length === 0, JSON.stringify(errors));
  check("headRefOid is the PR's head when unpinned", c.headRefOid === "livehead");
  check("files, counts and churn come from the view", c.fileCount === 2 && c.additions === 13 && c.deletions === 2 && c.fileChurn[0].churn === 12, JSON.stringify(c));
  check("the patch lands at the path SKILL.md and build-tier1-prompt expect", c.diffPath === join(dir, "peer-review-diff-77.patch"));
  check("diffBytes is the size of the written file", c.diffBytes === readFileSync(c.diffPath).length);
  check("the diff uses gh pr diff and never the compare API", calls.some((a) => a[0] === "pr" && a[1] === "diff") && !calls.some((a) => a.join(" ").includes("/compare/")));
  check("both comment endpoints paginate", ["/pulls/77/comments", "/issues/77/comments"].every((ep) => calls.some((a) => a.join(" ").includes(ep) && a.includes("--paginate") && a.includes("--slurp"))));
  check("jiraTicket is the first ticket key in the title before the body", c.jiraTicket === "ABC-2399" && jiraTicketOf("no ticket", "body XY-7") === "XY-7" && jiraTicketOf("", "") === "");
  check("a key from any project is found", jiraTicketOf("[OPS12-88] Rotate keys", "") === "OPS12-88");
  check("a character range in a body is not a ticket", jiraTicketOf("no ticket", "match [A-Z0-9]+ here") === "");
  check("isDeploymentPR is false off a deploy base", c.isDeploymentPR === false);
  const dep = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 77, repo: REPO, gh: fakeGh({ view: baseView({ baseRefName: "staging" }) }).gh, tmpdir: tmp() });
  check("isDeploymentPR is true on staging", dep.context.isDeploymentPR === true);
  // A fixture with only the default list cannot tell "reads the caller's list"
  // from "has that list hardcoded again": both answer the same. A foreign list can.
  const foreign = (base) => fetchPrContext({ deployBranches: ["release"], prNumber: 77, repo: REPO, gh: fakeGh({ view: baseView({ baseRefName: base }) }).gh, tmpdir: tmp() });
  check("another project's deploy list is honoured, not the default one", (await foreign("release")).context.isDeploymentPR === true && (await foreign("staging")).context.isDeploymentPR === false);
  let refused = false;
  try { await fetchPrContext({ prNumber: 77, repo: REPO, gh: fakeGh().gh, tmpdir: tmp() }); } catch { refused = true; }
  check("a caller that passes no deploy list is refused", refused);
}

console.log("--- pinned: counts and churn describe the PINNED diff ---");
{
  const compareFiles = [
    { filename: "src/f1.ts", additions: 5, deletions: 5 },
    { filename: "src/f9.ts", additions: 1, deletions: 0 },
  ];
  const { gh, calls } = fakeGh({ compareFiles });
  const { context: c } = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 77, repo: REPO, headSha: PIN, gh, tmpdir: tmp() });
  check("headRefOid is the pin, not the live head", c.headRefOid === PIN);
  check("files come from compare", JSON.stringify(c.files) === JSON.stringify(["src/f1.ts", "src/f9.ts"]), JSON.stringify(c.files));
  check("counts are summed from compare", c.additions === 6 && c.deletions === 5 && c.fileCount === 2);
  check("churn comes from compare (the prompt version read the current head)", c.fileChurn[0].churn === 10 && c.fileChurn[1].churn === 1, JSON.stringify(c.fileChurn));
  check("the compare range is base...pin", calls.some((a) => a.join(" ").includes(`compare/base000...${PIN}`)));
  check("a pinned run never calls gh pr diff", !calls.some((a) => a[0] === "pr" && a[1] === "diff"));
  const noBase = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 77, repo: REPO, headSha: PIN, gh: fakeGh({ view: baseView({ baseRefOid: "" }) }).gh, tmpdir: tmp() });
  check("a pinned run with no base reports it and leaves the diff empty", noBase.errors.some((e) => e.step === "compare-diff") && noBase.context.diffPath === "" && noBase.context.diffBytes === null);
}

console.log("--- the diff field window ---");
{
  const files = (n) => Array.from({ length: n }, (_, i) => fileEntry(i));
  const run = async (n, bytes) =>
    (await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 5, repo: REPO, gh: fakeGh({ view: baseView({ files: files(n) }), diff: "x".repeat(bytes) }).gh, tmpdir: tmp() })).context.diff;
  check("constants match the rule (more than 12 files, at most 60000 bytes)", DIFF_FIELD_MIN_FILES === 13 && DIFF_FIELD_MAX_BYTES === 60000);
  check("12 files: empty", (await run(12, 100)) === "");
  check("13 files: the text", (await run(13, 100)).length === 100);
  check("13 files at 60000 bytes: the text", (await run(13, 60000)).length === 60000);
  check("13 files at 60001 bytes: empty", (await run(13, 60001)) === "");
  const quiet = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 5, repo: REPO, withDiffText: false, gh: fakeGh({ view: baseView({ files: files(13) }), diff: "x".repeat(100) }).gh, tmpdir: tmp() });
  check("--no-diff-text keeps the diff out at 13 files, for the in-context orchestrator", quiet.context.diff === "");
  check("--no-diff-text still writes the patch and reports its size", quiet.context.diffBytes === 100 && readFileSync(quiet.context.diffPath, "utf8").length === 100);
}

console.log("--- bots, humans and pagination ---");
{
  const inlinePages = [[
    { user: { login: "coderabbitai[bot]" }, path: "src/f1.ts", line: 12, body: "bot inline", created_at: "t1" },
    { user: { login: "bob" }, path: "src/f1.ts", body: "human inline", created_at: "t2" },
  ]];
  const page1 = Array.from({ length: 30 }, (_, i) => ({ user: { login: "dave" }, body: `c${i}`, created_at: `d${i}` }));
  const page2 = [
    { user: { login: "alice" }, body: "fixed in abc123", created_at: "d30" },
    { user: { login: "github-actions[bot]" }, body: "ci", created_at: "d31" },
  ];
  const opts = { inlinePages, threadPages: [page1, page2] };
  const off = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, gh: fakeGh(opts).gh, tmpdir: tmp() });
  const on = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, withBotComments: true, gh: fakeGh(opts).gh, tmpdir: tmp() });
  check("botComments is empty without --bot-comments", off.context.botComments.length === 0);
  check("bot inline comments carry their line, which in-context vetting reads", on.context.botComments[0].line === 12, JSON.stringify(on.context.botComments[0]));
  check("with --bot-comments: bot inline plus bot review bodies", on.context.botComments.length === 2 && on.context.botComments[0].path === "src/f1.ts" && on.context.botComments[1].user === "coderabbitai", JSON.stringify(on.context.botComments));
  check("every page of the thread is read (31 human comments across 2 pages)", off.raw.threadComments.length === 31, `${off.raw.threadComments.length}`);
  check("the newest human comment, on page 2, is present", off.raw.threadComments.some((c) => c.body === "fixed in abc123"));
  check("raw never carries a bot", [...off.raw.threadComments, ...off.raw.inlineComments].every((c) => !BOT_LOGIN.test(c.user)) && off.raw.humanReviews.every((r) => !BOT_LOGIN.test(r.author)));
  check("raw.humanReviews skips empty review bodies", off.raw.humanReviews.length === 1 && off.raw.humanReviews[0].author === "bob");
  check("BOT_LOGIN covers the logins the workflow treats as bots", ["coderabbitai", "coderabbitai[bot]", "github-actions", "dependabot", "renovate"].every((l) => BOT_LOGIN.test(l)) && !BOT_LOGIN.test("bob"));
}

console.log("--- priorReviews: every review, summaries left to the model ---");
{
  const { context: c, raw } = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 3, repo: REPO, gh: fakeGh().gh, tmpdir: tmp() });
  check("every review, bots included, in order", c.priorReviews.map((r) => r.author).join(",") === "coderabbitai,bob,carol");
  check("states and timestamps are verbatim", c.priorReviews[1].state === "CHANGES_REQUESTED" && c.priorReviews[1].at === "2026-09-11T00:00:00Z");
  check("every summary is left empty for the model", c.priorReviews.every((r) => r.summary === ""));
  check("raw.changesRequested indexes into priorReviews", raw.changesRequested.length === 1 && raw.changesRequested[0].index === 1 && raw.changesRequested[0].body.includes("racy"));
}

console.log("--- --thread-before: a re-run of an old head reads that head's thread ---");
{
  const inlinePages = [[
    { user: { login: "bob" }, path: "src/f1.ts", body: "early inline", created_at: "2026-09-10T12:00:00Z" },
    { user: { login: "coderabbitai[bot]" }, path: "src/f1.ts", line: 3, body: "late bot", created_at: "2026-09-12T00:00:00Z" },
  ]];
  const threadPages = [[
    { user: { login: "dave" }, body: "early", created_at: "2026-09-10T00:00:00Z" },
    { user: { login: "alice" }, body: "RESOLVED in abc123", created_at: "2026-09-11T00:00:00Z" },
    { user: { login: "erin" }, body: "no timestamp" },
  ]];
  const cut = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 8, repo: REPO, threadBefore: "2026-09-11T00:00:00Z", withBotComments: true, gh: fakeGh({ inlinePages, threadPages }).gh, tmpdir: tmp() });
  check("reviews at or after the cutoff are gone", cut.context.priorReviews.map((r) => r.author).join(",") === "coderabbitai", JSON.stringify(cut.context.priorReviews));
  check("thread comments at or after the cutoff are gone", cut.raw.threadComments.map((c) => c.body).join("|") === "early", JSON.stringify(cut.raw.threadComments));
  check("inline comments and bot comments are cut too", cut.raw.inlineComments.length === 1 && cut.context.botComments.every((c) => c.body !== "late bot"), JSON.stringify(cut.context.botComments));
  check("changesRequested follows the cut reviews", cut.raw.changesRequested.length === 0);
  const whole = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 8, repo: REPO, gh: fakeGh({ inlinePages, threadPages }).gh, tmpdir: tmp() });
  check("without a cutoff nothing is dropped, undated items included", whole.raw.threadComments.length === 3 && whole.context.priorReviews.length === 3);
  check("postedBefore compares instants, not strings", postedBefore("2026-09-10T23:59:59Z", "2026-09-11T00:00:00.000Z") && !postedBefore("2026-09-11T00:00:00Z", "2026-09-11T00:00:00.000Z") && !postedBefore("", "2026-09-11T00:00:00Z"));
  const bad = spawnSync(process.execPath, [SCRIPT, "--pr", "1", "--repo", REPO, "--deploy-branches", "staging", "--thread-before", "yesterday"], { encoding: "utf8" });
  check("a cutoff that is not a date is a usage error", /"step":"usage"/.test(bad.stdout) && bad.stdout.includes("--thread-before"), bad.stdout);
}

console.log("--- failures leave the schema's empty values, never a crash ---");
{
  const all = ["view", "diff", "inline", "thread", "viewer"];
  const out = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 4, repo: REPO, gh: fakeGh({ fail: all }).gh, tmpdir: tmp() });
  const steps = out.errors.map((e) => e.step).sort().join(",");
  check("each failed call is named once", steps === "inline-comments,pr-diff,pr-view,thread-comments,viewer", steps);
  check("a failed diff leaves diffPath empty and diffBytes null", out.context.diffPath === "" && out.context.diffBytes === null && out.context.diff === "");
  check("required schema fields are still present", ["title", "baseRefName", "fileCount", "diff", "isDeploymentPR", "files"].every((k) => k in out.context));
  const garbage = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 4, repo: REPO, gh: async (a) => ({ ok: true, stdout: a[0] === "pr" && a[1] === "diff" ? "d" : "not json" }), tmpdir: tmp() });
  check("unparseable gh output is an error, not a throw", garbage.errors.some((e) => e.step === "pr-view" && /unparseable/.test(e.detail)));
}

console.log("--- the CLI contract ---");
{
  const r = spawnSync(process.execPath, [SCRIPT], { encoding: "utf8" });
  const line = r.stdout.trim();
  let parsed = null;
  try { parsed = JSON.parse(line); } catch {}
  const src = readFileSync(SCRIPT, "utf8");
  check("the CLI maps --no-diff-text and --bot-comments to their options", src.includes('withDiffText: !argv.includes("--no-diff-text")') && src.includes('withBotComments: argv.includes("--bot-comments")'));
  check(
    "--relay prints relay lines of relayContext, with prep only under --prep",
    src.includes("const relayed = relayContext(out, relayDir);") &&
      src.includes("if (worktreeScript) {") &&
      src.includes("process.stdout.write(relayLines(relayed));"),
  );
  check("a usage error still exits 0 with one line of JSON", r.status === 0 && line.split("\n").length === 1 && parsed && parsed.errors[0].step === "usage", r.stdout + r.stderr);
}

console.log("--- --relay: raw text stays on disk, the context comes back checksummed ---");
{
  const relay = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["checkRelay"]).api;
  const build = loadRegion(WORKFLOW, "FETCH-PR-CONTEXT", ["buildPrContext"]);
  const opts = {
    inlinePages: [[{ user: { login: "coderabbitai[bot]" }, path: "src/f1.ts", line: 12, body: "Potential race. ".repeat(40), created_at: "t" }, { user: { login: "bob" }, path: "src/f1.ts", body: "why?", created_at: "t" }]],
    threadPages: [[{ user: { login: "alice" }, body: "BY-DESIGN: the guard stays", created_at: "t2" }]],
  };
  const out = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, withBotComments: true, withDiffText: false, gh: fakeGh(opts).gh, tmpdir: tmp() });
  const dir = join(tmp(), "ctx");
  const data = relayContext(out, dir);
  check("raw.json on disk is the raw object, byte for byte", readFileSync(data.rawPath, "utf8") === JSON.stringify(out.raw) && data.rawBytes === Buffer.byteLength(JSON.stringify(out.raw)));
  check("bot-comments.json holds the bodies", JSON.parse(readFileSync(data.botCommentsPath, "utf8"))[0].body === out.context.botComments[0].body);
  check(
    "the relayed context carries the bots as an index, never a body",
    data.botComments.length === out.context.botComments.length && data.botComments.every((c, i) => c.i === i && !("body" in c)) && data.botComments[0].bytes === Buffer.byteLength(out.context.botComments[0].body),
    JSON.stringify(data.botComments),
  );
  check("and every other context field unchanged", Object.keys(out.context).every((k) => k === "botComments" || JSON.stringify(data[k]) === JSON.stringify(out.context[k])));
  check("rawHasHumanText says whether there is anything to harvest", data.rawHasHumanText === true);
  const quiet = relayContext(await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, gh: fakeGh({ view: baseView({ reviews: [], body: "" }) }).gh, tmpdir: tmp() }), join(tmp(), "q"));
  check("…and false on a PR with no human text", quiet.rawHasHumanText === false);
  check("the PR description reaches raw.json with its author", JSON.stringify(out.raw.description) === JSON.stringify([{ author: "alice", body: "Also see ABC-1" }]), JSON.stringify(out.raw.description));
  const described = relayContext(await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, gh: fakeGh({ view: baseView({ reviews: [], body: "Stacked PR: the multi-order half is in #12" }) }).gh, tmpdir: tmp() }), join(tmp(), "d"));
  check("a description alone is human text to harvest", described.rawHasHumanText === true);
  const botBody = await fetchPrContext({ deployBranches: DEPLOY_BRANCHES, prNumber: 9, repo: REPO, gh: fakeGh({ view: baseView({ reviews: [], author: { login: "dependabot[bot]" } }) }).gh, tmpdir: tmp() });
  check("a bot's description is left out", botBody.raw.description.length === 0, JSON.stringify(botBody.raw.description));
  const lines = relayLines(data).trimEnd().split("\n");
  check("the relay line of it verifies, and a copy missing one review does not", relay.checkRelay({ relayCrc: lines[0].slice(10), data: JSON.parse(lines[1]) }).ok && !relay.checkRelay({ relayCrc: lines[0].slice(10), data: { ...data, priorReviews: data.priorReviews.slice(1) } }).ok);

  check("the FETCH-PR-CONTEXT region is pure", build.violations.length === 0, JSON.stringify(build.violations));
  const answer = {
    inThreadDecisions: [{ category: "BY-DESIGN", summary: "the guard stays", source: "alice, t2", files: ["src/f1.ts"] }],
    priorReviewSummaries: [{ index: 1, summary: "src/f1.ts: the guard is racy" }, { index: 2, summary: "not a change request" }, { index: "1", summary: "string index" }],
  };
  const ctx = build.api.buildPrContext(data, answer);
  check("the harvest's decisions are the context's decisions", ctx.inThreadDecisions === answer.inThreadDecisions);
  check("a summary lands on its CHANGES_REQUESTED review by index", ctx.priorReviews[1].summary === "src/f1.ts: the guard is racy" && ctx.priorReviews[1].state === "CHANGES_REQUESTED");
  check("never on another review, and never from a non-integer index", ctx.priorReviews[2].summary === "" && ctx.priorReviews[0].summary === "");
  check("no review is dropped, added or reordered", ctx.priorReviews.map((r) => r.author).join() === data.priorReviews.map((r) => r.author).join());
  const bare = build.api.buildPrContext(data, null);
  check("no harvest means no decisions and empty summaries, not a crash", bare.inThreadDecisions.length === 0 && bare.priorReviews.every((r) => r.summary === ""));
}

console.log("--- --prep: worktree, sections and blast radius in the same process ---");
{
  const ctx = (over = {}) => ({ headRefOid: PIN, isDeploymentPR: false, diffPath: "/tmp/p.patch", files: ["a.ts", "b.ts"], additions: 3, deletions: 1, ...over });
  const relayOut = (obj) => ({ ok: true, stdout: relayLines(obj), stderr: "" });
  const fakeExec = (answers) => {
    const calls = [];
    const exec = async (cmd, args, env) => {
      calls.push({ cmd, args, env });
      if (cmd === "git") return answers.git ? answers.git(args) : { ok: true, stdout: "", stderr: "" };
      if (cmd === "bash") return answers.worktree;
      if (args[0].endsWith("blast-radius.mjs")) return answers.blast;
      if (args[0].endsWith("section-diff.mjs")) return answers.sections;
      throw new Error(`unexpected ${cmd} ${args.join(" ")}`);
    };
    return { exec, calls };
  };
  const ok = { worktree: { ok: true, stdout: "CREATED_WORKTREE /tmp/wt-1\n", stderr: "" }, blast: relayOut({ callers: [{ file: "c.ts" }] }), sections: relayOut({ usable: true, sections: [] }) };

  const a = fakeExec(ok);
  const p = await prepareReview(ctx(), { worktreeScript: "/tmp/wt.sh", sectionsAbove: 12, pr: "7", exec: a.exec, tmpdir: tmp() });
  const bashCall = a.calls.find((c) => c.cmd === "bash");
  check("the worktree script runs with the reported headRefOid as PREP_SHA", bashCall && bashCall.args[0] === "/tmp/wt.sh" && bashCall.env.PREP_SHA === PIN);
  check("stale review checkouts are looked for before a new one is made", a.calls[0].cmd === "git" && a.calls[0].args.join(" ") === "worktree list --porcelain");
  check("its marker line is relayed as printed", p.worktree === "CREATED_WORKTREE /tmp/wt-1");
  check("the blast radius runs on that checkout and comes back as its data line", a.calls.some((c) => c.args[0].endsWith("blast-radius.mjs") && c.args[1] === "/tmp/p.patch" && c.args[2] === "/tmp/wt-1") && p.blastRadius.callers[0].file === "c.ts");
  check("no sections at or below the threshold", p.sections === null && !a.calls.some((c) => c.args[0].endsWith("section-diff.mjs")));

  const big = fakeExec(ok);
  const files = Array.from({ length: 13 }, (_, i) => `f${i}.ts`);
  const pb = await prepareReview(ctx({ files }), { worktreeScript: "/tmp/wt.sh", sectionsAbove: 12, pr: "7", exec: big.exec, tmpdir: tmp() });
  const sec = big.calls.find((c) => c.args[0].endsWith("section-diff.mjs"));
  check("sections above the threshold, with the diff, a fresh dir and the counts", pb.sections && pb.sections.usable === true && sec && sec.args[1] === "/tmp/p.patch" && /tribunal-sections-7-/.test(sec.args[2]) && sec.args[3] === "3" && sec.args[4] === "1");

  const cloned = fakeExec({ ...ok, worktree: { ok: true, stdout: "CLONED:/tmp/clone-1\n", stderr: "" } });
  await prepareReview(ctx(), { worktreeScript: "/tmp/wt.sh", pr: "7", exec: cloned.exec, tmpdir: tmp() });
  check("a clone fallback is a checkout too", cloned.calls.some((c) => c.args[0].endsWith("blast-radius.mjs") && c.args[2] === "/tmp/clone-1"));

  const failed = fakeExec({ ...ok, worktree: { ok: true, stdout: "WORKTREE_FAILED\n", stderr: "" } });
  const pf = await prepareReview(ctx(), { worktreeScript: "/tmp/wt.sh", pr: "7", exec: failed.exec, tmpdir: tmp() });
  check("no checkout, no blast radius, and the failure marker is relayed", pf.worktree === "WORKTREE_FAILED" && pf.blastRadius === null && failed.calls.filter((c) => c.cmd !== "git").length === 1);

  const broken = fakeExec({ ...ok, blast: { ok: false, stdout: "FATAL: cannot read diff\n", stderr: "" } });
  const pe = await prepareReview(ctx(), { worktreeScript: "/tmp/wt.sh", pr: "7", exec: broken.exec, tmpdir: tmp() });
  check("a lib script that prints no relay comes back as {error}, so the workflow falls back", typeof pe.blastRadius.error === "string" && /FATAL/.test(pe.blastRadius.error));

  for (const [name, over] of [["a deployment PR", { isDeploymentPR: true }], ["no head sha", { headRefOid: "" }]]) {
    const n = fakeExec(ok);
    const pn = await prepareReview(ctx(over), { worktreeScript: "/tmp/wt.sh", pr: "7", exec: n.exec, tmpdir: tmp() });
    check(`${name} prepares nothing`, n.calls.length === 0 && pn.worktree === null && pn.blastRadius === null && pn.sections === null);
  }
  {
    const HOUR = 60 * 60 * 1000;
    const now = 100 * HOUR;
    const ages = { "/T/peer-review-wt-2119-XnJwy9": 3 * HOUR, "/T/peer-review-wt-2167-ibEynJ": 0.5 * HOUR, "/repo": 50 * HOUR, "/T/peer-review-wt-gone-x": 9 * HOUR, "/T/other-wt-1-abc": 9 * HOUR };
    const listing = Object.keys(ages).map((p) => `worktree ${p}\nHEAD abc\ndetached\n`).join("\n") + "worktree /T/peer-review-wt-9-missing\n";
    const gitCalls = [];
    const exec = async (cmd, args) => {
      gitCalls.push(args.join(" "));
      return { ok: true, stdout: args[1] === "list" ? listing : "", stderr: "" };
    };
    const stat = (p) => {
      if (!(p in ages)) throw new Error("ENOENT");
      return { birthtimeMs: now - ages[p], mtimeMs: now };
    };
    const removed = await pruneStaleWorktrees({ exec, now, stat });
    check("a review checkout older than two hours is removed, then pruned", removed.length === 1 && removed[0] === "/T/peer-review-wt-2119-XnJwy9" && gitCalls.includes("worktree remove --force /T/peer-review-wt-2119-XnJwy9") && gitCalls.at(-1) === "worktree prune", JSON.stringify(gitCalls));
    check("a younger one, the main checkout, another prefix and a missing path are left alone", !gitCalls.some((c) => /remove --force \/(repo|T\/peer-review-wt-2167|T\/other|T\/peer-review-wt-9)/.test(c)));
    const none = [];
    await pruneStaleWorktrees({ exec: async (c, a) => (none.push(a.join(" ")), { ok: true, stdout: a[1] === "list" ? "worktree /T/peer-review-wt-1-a\n" : "", stderr: "" }), now, stat: () => ({ birthtimeMs: now - HOUR, mtimeMs: now }) });
    check("nothing stale means no remove and no prune", none.length === 1);
  }
  check("worktreePathOf reads the same markers as the workflow's parser", worktreePathOf("x\nCREATED_WORKTREE /a\n") === "/a" && worktreePathOf("CLONED:/b") === "/b" && worktreePathOf("CREATED_WORKTREE ") === null && worktreePathOf("WORKTREE_FAILED") === null);
  check("relayData takes the line after RELAY-CRC", relayData(relayOut({ x: 1 })).x === 1 && typeof relayData({ stdout: "garbage", stderr: "" }).error === "string");

  // BE #2167, 2026-10-21: a 14 KB relay came back from haiku as invalid JSON five times.
  const caller = (i) => ({ file: `src/c${i}.ts`, line: i, text: `await enqueue("job", { id: ${i} }) // "quoted" \\ backslash` });
  const relayedWith = (blastN, sectionsN) => ({
    title: "t",
    files: ["a.ts"],
    prep: {
      worktree: "CREATED_WORKTREE /tmp/wt",
      blastRadius: { callers: Array.from({ length: blastN }, (_, i) => caller(i)) },
      sections: sectionsN ? { usable: true, sections: Array.from({ length: sectionsN }, (_, i) => ({ file: `f${i}.ts`, path: `/tmp/s/${i}.slice`, chars: 100 })) } : null,
    },
  });
  const bytes = (o) => Buffer.byteLength(JSON.stringify(o), "utf8");
  const small = boundPrep(relayedWith(3, 0));
  check("a small prep stays in the relay whole", !small.prep.blastRadius.error && small.prep.blastRadius.callers.length === 3 && bytes(small) <= PREP_RELAY_MAX_BYTES);
  const mid = boundPrep(relayedWith(10, 60));
  check("the sections leave first, and only as far as needed", mid.prep.sections.error && !mid.prep.blastRadius.error && bytes(mid) <= PREP_RELAY_MAX_BYTES, `${bytes(mid)}`);
  const huge = boundPrep(relayedWith(60, 60));
  check(
    "a blast radius too big for the relay is deferred to its own dispatch, as an {error} the workflow falls back on",
    /^deferred/.test(huge.prep.sections.error) && /^deferred/.test(huge.prep.blastRadius.error) && huge.prep.worktree === "CREATED_WORKTREE /tmp/wt",
  );
  const noSections = boundPrep(relayedWith(60, 0));
  check("with no sections to drop, the blast radius goes", noSections.prep.sections === null && /^deferred/.test(noSections.prep.blastRadius.error));
  check("the CLI bounds the relay after preparing it", readFileSync(SCRIPT, "utf8").includes("boundPrep(relayed);"));
}

console.log("--- the workflow dispatch ---");
{
  const src = readFileSync(WORKFLOW, "utf8");
  check("fetch-pr writes the workflow's own worktree bash, verified, with the sha left to the script", /verifiedHeredoc\(\s*`"\$\{CONTEXT_DIR\}\/head-worktree\.sh"`,\s*"TRIBUNAL_HEAD_WORKTREE",\s*headWorktreeBash\("\$PREP_SHA", repo, prNumber\)/.test(src));
  check("and asks for prep except on a preflight", src.includes("const wantsPrep = parsedArgs.preflight !== true;") && src.includes('wantsPrep ? ` --prep "${CONTEXT_DIR}/head-worktree.sh" --prep-sections-above ${SECTIONS_ABOVE_FILES}` : ""'));
  check("prep leaves prContext before anything persists it", /const contextPrep = prContext\.prep[^\n]*\ndelete prContext\.prep;/.test(src));
  check("each prepared part replaces its dispatch, which stays as the fallback", src.includes("headPin = parseHeadWorktree(contextPrep.worktree);") && src.includes('{ label: "setup-head-worktree", phase: "Context", ...MECHANICAL }') && src.includes("? { ok: true, data: preparedBlast }") && src.includes("? Promise.resolve({ ok: true, data: preparedSections })"));
  check("the sections threshold is the one the lens partition uses", src.includes("const SECTIONS_ABOVE_FILES = 12;") && src.includes("const FILES_PER_GROUP = 6;") && src.includes("allPrFiles.length > SECTIONS_ABOVE_FILES"));
  check("the prompt keeps the prefix the cost classifier keys on", /`Fetch PR #\$\{prNumber\} from GitHub repo \$\{repo\} and return structured data\./.test(src));
  check("it runs the script through tribunal-root", src.includes('node "$(tribunal-root)"/skills/review-pr/scripts/fetch-pr-context.mjs --pr ${prNumber} --repo ${repo}') && src.includes('--deploy-branches "${deployBranchList(PROFILE)}"'));
  check("a pinned run passes the pin", src.includes("pinnedSha ? ` --head-sha ${pinnedSha}` : \"\""));
  check("threadBefore reaches the script, and only as a validated UTC instant", src.includes("threadBefore ? ` --thread-before ${threadBefore}` : \"\"") && src.includes("if (threadBefore && !/^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(\\.\\d+)?Z$/.test(threadBefore))"));
  check("bot comments are fetched only when they will be vetted", src.includes('vetBotComments ? " --bot-comments" : ""'));
  check("the prompt no longer lists gh commands for the model to run", !/\n1\. gh pr view \$\{prNumber\}/.test(src) && !src.includes("const diffStep"));
  const thunk = src.slice(src.indexOf("const prContextThunk = () =>"), src.indexOf("{ label: \"fetch-pr\", phase: \"Context\", schema: FETCH_PR_SCHEMA }"));
  check("fetch-pr is a verified relay that also returns the harvest", thunk.includes("relayAgent(") && thunk.includes("${RELAY_RULES}") && /inThreadDecisions: PR_CONTEXT_SCHEMA\.properties\.inThreadDecisions/.test(src));
  check("the harvest reads raw.json from disk, only when there is human text", thunk.includes('cat "${CONTEXT_DIR}/raw.json"') && thunk.includes('"rawHasHumanText": true'));
  check("the decisions rule is carried over whole", thunk.includes("raw.threadComments IS THE PR's TOP-LEVEL COMMENT THREAD") && thunk.includes("do NOT guess a file just to fill the field"));
  check(
    "the harvest reads the description, for split-outs and deferrals only",
    thunk.includes("raw.description, raw.humanReviews") &&
      thunk.includes("From it take ONLY SPLIT-OUT and DEFERRED entries") &&
      thunk.includes("Never take a RESOLVED or BY-DESIGN entry from the description"),
  );
  check("the dispatch is told to answer once, with all four fields", thunk.includes("Answer ONCE, after both steps below: a single structured answer carrying relayFile (or relayCrc and data), inThreadDecisions and priorReviewSummaries together."));
  check("an unverified context stops the run", src.includes("fetched && fetched.ok ? buildPrContext(fetched.data, fetched.answer) : null"));
  check("bot vetting reads the bodies by path and returns indexes", src.includes("The bot comments are a JSON array in ${prContext.botCommentsPath}") && /required: \["index", "classification", "summary", "reason"\]/.test(src) && !src.includes("JSON.stringify(prContext.botComments"));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall fetch-pr-context expectations hold");
