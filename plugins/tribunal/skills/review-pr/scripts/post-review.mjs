#!/usr/bin/env node
// post-review.mjs — Step 4's posting half, consolidated.
//
// Why this exists. SKILL.md used to teach "Posting" as a five-step manual
// sequence (strip draft comments -> write body file -> gh api/gh pr review
// -> grep for a leak -> gh pr view for the URL), each a separate top-level
// tool call in the ORCHESTRATOR's own session turn. This script does the
// same sequence in one call, with the same guards, and makes the leak-check
// a hard failure instead of a printed warning.
//
// Safety invariants preserved:
//   - Draft comments are stripped by IMPORTING stripDraftComments from
//     strip-draft-comments.mjs, never reimplemented here.
//   - This script never decides WHETHER to post or WHAT verdict to use —
//     that decision is made entirely in SKILL.md prose / AskUserQuestion,
//     before this script is ever invoked. It only executes a decision
//     already made.
//   - After posting, the posted body is re-fetched and grepped for
//     "Draft comments" — a match is a hard failure (non-zero exit), not
//     merely a printed count.
//
// Usage:
//   node post-review.mjs --pr <n> --repo <owner/repo> --report-file <path> --footer-file <path> --approve
//   node post-review.mjs --pr <n> --repo <owner/repo> --report-file <path> --footer-file <path> --request-changes
//   node post-review.mjs --pr <n> --repo <owner/repo> --report-file <path> --footer-file <path> --comment
//   node post-review.mjs --pr <n> --repo <owner/repo> --report-file <path> --footer-file <path> --overwrite <review-id>
//   node post-review.mjs --pr <n> --repo <owner/repo> --find-review-id <author-login>   # prints numeric review id, does not post
//
// --footer-file is required on every posting mode: build it with
// scripts/build-post-footer.mjs. Every posted review ends with a metadata
// footer (harness version, review mode, Codex model, agent count, tokens
// when known, timestamp) — never hand-typed by the orchestrator. It is
// appended AFTER stripDraftComments() runs, so it can never collide with the
// Draft-comments strip and never passes through the homosapiens pass Step 4
// runs before this script is invoked (the footer is data, not prose).

import { readFileSync, writeFileSync, unlinkSync, realpathSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { stripDraftComments } from "./strip-draft-comments.mjs";

// A strict `import.meta.url === file://${process.argv[1]}` comparison breaks
// when the script is invoked through a symlink (e.g. the ~/.claude/skills/...
// symlink into this plugin's real, version-keyed cache directory): argv[1]
// keeps the symlink path while import.meta.url resolves to the real one, so
// they never match, main() silently never runs, and the script exits 0 having
// done nothing. Resolve both sides to their real path before comparing.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) {
      out[key] = true;
    } else {
      out[key] = next;
      i++;
    }
  }
  return out;
}

function gh(args) {
  return execFileSync("gh", args, { encoding: "utf8" });
}

// Verdict banner: printed FIRST, above even the report body — a reader must
// see the outcome without reading prose or hunting for GitHub's small state
// badge. Driven by the actual gh pr review flag being submitted, never by
// anything hand-typed into the report body — that's what let an APPROVE post
// go out reading as flat technical prose with no visible verdict
// (2026-08-19).
const VERDICT_BANNERS = {
  APPROVE: "### ✅ Approved",
  REQUEST_CHANGES: "### 🔴 Changes requested",
  COMMENT: "### 💬 Comment",
};

function buildVerdictBanner(verdict) {
  const banner = VERDICT_BANNERS[verdict];
  if (!banner) throw new Error(`buildVerdictBanner: unknown verdict "${verdict}"`);
  return banner;
}

// Strips "### Draft comments" (never posted) and appends the metadata footer
// AFTER that strip — so the footer survives regardless of where in the report
// heredoc anyone put it, and is structurally outside both the Draft-comments
// leak this script already guards against and the homosapiens pass Step 4 runs
// before this script is ever invoked (the footer is data, not prose, and must
// never pass through either). The verdict banner goes above everything else.
function composeBody(rawReport, footerText, verdict) {
  const stripped = stripDraftComments(rawReport).replace(/\s+$/, "");
  const banner = buildVerdictBanner(verdict);
  return `${banner}\n\n${stripped}\n\n${footerText.trim()}\n`;
}

function findReviewId(pr, repo, authorLogin) {
  const raw = gh(["api", `repos/${repo}/pulls/${pr}/reviews`]);
  const reviews = JSON.parse(raw);
  const matches = reviews.filter((r) => r.user && r.user.login === authorLogin);
  if (matches.length === 0) return null;
  // Most recent by submitted_at.
  matches.sort((a, b) => new Date(b.submitted_at) - new Date(a.submitted_at));
  return matches[0].id;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.pr || !args.repo) {
    process.stderr.write(
      "usage: --pr <n> --repo <owner/repo> [--find-review-id <login> | --report-file <path> (--approve|--request-changes|--comment|--overwrite <id>)]\n"
    );
    process.exit(2);
  }

  if (args["find-review-id"]) {
    const id = findReviewId(args.pr, args.repo, args["find-review-id"]);
    if (id === null) {
      process.stderr.write(`no review found by ${args["find-review-id"]} on PR #${args.pr}\n`);
      process.exit(1);
    }
    process.stdout.write(String(id) + "\n");
    return;
  }

  if (!args["report-file"]) {
    process.stderr.write("--report-file is required to post\n");
    process.exit(2);
  }
  const mode = args.overwrite
    ? "overwrite"
    : args.approve
    ? "approve"
    : args["request-changes"]
    ? "request-changes"
    : args.comment
    ? "comment"
    : null;
  if (!mode) {
    process.stderr.write("one of --approve, --request-changes, --comment, --overwrite <id> is required\n");
    process.exit(2);
  }

  if (!args["footer-file"]) {
    process.stderr.write(
      "--footer-file is required — build it with scripts/build-post-footer.mjs; every posted review ends with a metadata footer, never hand-typed\n"
    );
    process.exit(2);
  }
  let footerText;
  try {
    footerText = readFileSync(args["footer-file"], "utf8").trim();
  } catch (e) {
    process.stderr.write(`--footer-file could not be read: ${e.message}\n`);
    process.exit(2);
  }
  if (!footerText) {
    process.stderr.write("--footer-file is empty — refusing to post without a footer\n");
    process.exit(2);
  }

  let verdict;
  if (mode === "overwrite") {
    // --overwrite changes the body only, never the review's state (same rule
    // documented in SKILL.md) — so the banner must reflect what GitHub
    // already has recorded for this review, not the mode string, which
    // carries none.
    const existing = JSON.parse(gh(["api", `repos/${args.repo}/pulls/${args.pr}/reviews/${args.overwrite}`]));
    const stateMap = { APPROVED: "APPROVE", CHANGES_REQUESTED: "REQUEST_CHANGES", COMMENTED: "COMMENT" };
    verdict = stateMap[existing.state];
    if (!verdict) {
      process.stderr.write(`cannot determine verdict banner: review ${args.overwrite} has state "${existing.state}"\n`);
      process.exit(1);
    }
  } else {
    verdict = mode === "approve" ? "APPROVE" : mode === "request-changes" ? "REQUEST_CHANGES" : "COMMENT";
  }

  const rawReport = readFileSync(args["report-file"], "utf8");
  const body = composeBody(rawReport, footerText, verdict);
  const bodyFile = join(tmpdir(), `post-review-body-${process.pid}.md`);
  writeFileSync(bodyFile, body);

  try {
    if (mode === "overwrite") {
      gh(["api", `repos/${args.repo}/pulls/${args.pr}/reviews/${args.overwrite}`, "-X", "PUT", "-F", `body=@${bodyFile}`]);
    } else {
      const flag =
        mode === "approve" ? "--approve" : mode === "request-changes" ? "--request-changes" : "--comment";
      gh(["pr", "review", String(args.pr), "--repo", args.repo, "--body-file", bodyFile, flag]);
    }

    // Hard leak-check: re-fetch the posted body and refuse to succeed silently
    // if the Draft comments section made it through anyway. Identify the
    // review by highest numeric id (monotonically increasing), not array
    // order, which GitHub does not guarantee.
    const reviews = JSON.parse(gh(["api", `repos/${args.repo}/pulls/${args.pr}/reviews`]));
    const target =
      mode === "overwrite"
        ? reviews.find((r) => String(r.id) === String(args.overwrite))
        : reviews.reduce((max, r) => (!max || r.id > max.id ? r : max), null);
    const postedBody = target ? target.body || "" : "";
    if (/Draft comments/.test(postedBody)) {
      process.stderr.write(
        "FATAL: posted review body still contains a 'Draft comments' section — strip step failed or was bypassed\n"
      );
      process.exit(1);
    }

    const url = gh(["pr", "view", String(args.pr), "--repo", args.repo, "--json", "url", "--jq", ".url"]).trim();
    process.stdout.write(url + "\n");
  } finally {
    try {
      unlinkSync(bodyFile);
    } catch {
      // best-effort cleanup only
    }
  }
}

if (isMainModule()) {
  main();
}

export { parseArgs, findReviewId, composeBody };
