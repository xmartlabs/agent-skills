#!/usr/bin/env node
// build-lens-prompts.mjs — the Workflow's lens prompts, outside the Workflow.
//
// It prints the Workflow's prompt for every planned lens: lensPrompt from the
// LENS-PROMPT region, the foci selectFoci picks from the profile for the
// Workflow's own lens budget, the pinned-checkout block from HEAD-WORKTREE, all
// cut from the shipped workflow by their markers. eval/lens-replay.mjs builds
// its prompts with the same functions, so what the replays measure is what the
// Workflow ships. (Until v101 it also fed the in-context path, now removed.)
//
// stdin (JSON): {"files": [...], "additions": N, "deletions": N}
// argv: --pr <n> --repo <owner/name> --head-sha <sha> --diff <patch path>
//       --profile <profile json> --methodology <path> --out-dir <dir>
//       [--conventions <path>] [--worktree <pinned checkout>]
//       [--dependents <file with render-dependents stdout>] [--jira <KEY-1>]
//       [--context <file with the decisions block>]
// stdout: one JSON line {lensCount, plannedLensCount, foci,
//         prompts: [paths], markerPresent}
// Each prompt file is the whole dispatch prompt: METHODOLOGY FILE line,
// CONVENTIONS FILE line when given, the Workflow prompt, and the return format.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { extractRegions } from "../eval/extract-pure-region.mjs";
import { readProfileFile, riskPathRules } from "./load-profile.mjs";

const WORKFLOW = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "workflows", "tribunal.js");
export const MARKER = "===== YOUR PASS =====";

// A Workflow lens returns through StructuredOutput, which `agent({schema})`
// adds to its tools. An Agent-tool dispatch has no schema, so the lens is asked
// for the same object as text, below its own section.
export const RETURN_SUFFIX = (schema) => `

StructuredOutput is not available in this session. Return the object you would have submitted through it as one JSON code block, matching this schema exactly, and nothing after it:
${JSON.stringify(schema)}`;

export function loadWorkflowPieces(src = readFileSync(WORKFLOW, "utf8")) {
  const fn = (name, returns) => new Function(`${extractRegions(src, name).join("\n")}\nreturn ${returns};`)();
  const { lensPrompt } = fn("LENS-PROMPT", "{ lensPrompt }");
  const { selectFoci, highRiskSurfaceFor } = fn("LENS-FOCI", "{ selectFoci, highRiskSurfaceFor }");
  const { dependentsBlock } = fn("DEPENDENTS-BLOCK", "{ dependentsBlock }");
  const { pinnedCheckoutBlock } = fn("HEAD-WORKTREE", "{ pinnedCheckoutBlock }");
  return {
    lensPrompt,
    selectFoci,
    highRiskSurfaceFor,
    dependentsBlock,
    pinnedCheckoutBlock,
    findingsSchema: findingsSchemaOf(src),
  };
}

// FINDINGS_SCHEMA is a plain object literal outside any region; cut it by
// brace depth so the lens is asked for the same shape a Workflow lens returns.
export function findingsSchemaOf(src, preamble = "") {
  const start = src.indexOf("const FINDINGS_SCHEMA = {");
  if (start === -1) throw new Error("FINDINGS_SCHEMA not found in the workflow source");
  const open = src.indexOf("{", start);
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return new Function(`${preamble}\nreturn ${src.slice(open, i + 1)};`)();
  }
  throw new Error("FINDINGS_SCHEMA is not terminated");
}

export function planLenses({ files, additions, deletions, profile, plan, lenses, foci: fociKeys, selectFoci }) {
  if (fociKeys) {
    const picked = fociKeys.map((k) => {
      if (!profile.foci[k]) throw new Error(`profile has no focus "${k}"`);
      return profile.foci[k];
    });
    return { mode: "foci", lensCount: picked.length, foci: picked };
  }
  if (lenses) {
    const foci = selectFoci(files, lenses, profile.foci, riskPathRules(profile));
    return { mode: "lenses", lensCount: lenses, foci: foci.slice(0, lenses) };
  }
  if (plan !== "workflow") throw new Error(`unknown --plan ${plan}`);
  const budget = workflowLensBudget(files.length, (Number(additions) || 0) + (Number(deletions) || 0));
  // selectFoci keeps every forced focus even past the budget, as the Workflow does.
  const foci = selectFoci(files, budget, profile.foci, riskPathRules(profile));
  return { mode: "workflow", lensCount: foci.length, plannedLensCount: budget, foci };
}

// The Workflow's lens budget on a full review (its fileTier, churnTier and
// groupCountBase lines). The Workflow cannot export it, so
// eval/replay-lens-replay.mjs checks these expressions against its source text.
export function workflowLensBudget(fileCount, churn) {
  const fileTier = fileCount <= 2 ? 1 : fileCount <= 10 ? 2 : 5;
  const churnTier =
    churn < 200 ? 0 : churn < 500 ? 2 : churn < 2000 ? 4 : churn < 5000 ? 6 : 8;
  const groupCount = fileCount > 12 ? Math.ceil(fileCount / FILES_PER_GROUP) : 1;
  return Math.max(fileTier, churnTier, groupCount);
}
const FILES_PER_GROUP = 6;

// The Workflow's diff body in path mode (READ_PATH_INSTRUCTION + LENS_DIFF_TAIL),
// with or without a pinned checkout.
export function lensDiffBlock(diffPath, worktree, headSha, pinnedCheckoutBlock) {
  const pin = worktree ? { ok: true, path: worktree } : null;
  return `The full PR diff has already been fetched to ${diffPath} — read that file instead of re-running gh pr view / gh pr diff. It is the authoritative copy of this PR's changes.
${pinnedCheckoutBlock(pin, headSha)} Any read BEYOND the diff — surrounding code, call sites, sibling surfaces — belongs there.`;
}

export function lensPromptBodies({ pieces, foci, prNumber, repo, diffPath, worktree, headSha, profile, dependents = "", jiraTicket = null, contextBlock = "" }) {
  const diffBlock = lensDiffBlock(diffPath, worktree, headSha, pieces.pinnedCheckoutBlock);
  return foci.map((focus) =>
    pieces.lensPrompt({
      prNumber,
      repo,
      diffBlock,
      jiraTicket,
      contextBlock,
      dependents,
      highRiskSurface: pieces.highRiskSurfaceFor(profile),
      focus,
      files: null,
    }),
  );
}

export const dispatchHeader = (methodology, conventions) =>
  `METHODOLOGY FILE: ${methodology}${conventions ? `\nCONVENTIONS FILE: ${conventions}` : ""}\n\n`;

const invokedDirectly = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

if (invokedDirectly) {
  const args = process.argv.slice(2);
  const opt = (k) => {
    const i = args.indexOf(k);
    return i >= 0 ? args[i + 1] : null;
  };
  const required = ["--pr", "--repo", "--head-sha", "--diff", "--profile", "--methodology", "--out-dir"];
  const missing = required.filter((k) => !opt(k));
  if (missing.length) {
    console.error(`build-lens-prompts: missing ${missing.join(", ")}`);
    process.exit(2);
  }
  let input;
  try {
    input = JSON.parse(readFileSync(0, "utf8"));
  } catch (e) {
    console.error(`build-lens-prompts: stdin is not valid JSON: ${e.message}`);
    process.exit(2);
  }
  if (!Array.isArray(input.files)) {
    console.error('build-lens-prompts: stdin must carry a "files" array');
    process.exit(2);
  }
  const loaded = readProfileFile(opt("--profile"));
  if (loaded.error) {
    console.error(`build-lens-prompts: ${loaded.error}`);
    process.exit(2);
  }
  const profile = loaded.profile;
  const pieces = loadWorkflowPieces();
  const plan = planLenses({ ...input, profile, plan: "workflow", selectFoci: pieces.selectFoci });
  const read = (k) => (opt(k) ? readFileSync(opt(k), "utf8").trim() : "");
  const bodies = lensPromptBodies({
    pieces,
    foci: plan.foci,
    prNumber: Number(opt("--pr")),
    repo: opt("--repo"),
    diffPath: opt("--diff"),
    worktree: opt("--worktree"),
    headSha: opt("--head-sha"),
    profile,
    dependents: read("--dependents"),
    jiraTicket: opt("--jira"),
    contextBlock: read("--context"),
  });
  const header = dispatchHeader(opt("--methodology"), opt("--conventions"));
  const outDir = opt("--out-dir");
  mkdirSync(outDir, { recursive: true });
  const prompts = bodies.map((b, i) => {
    const path = join(outDir, `lens-${i + 1}.txt`);
    writeFileSync(path, header + b + RETURN_SUFFIX(pieces.findingsSchema));
    return path;
  });
  process.stdout.write(
    `${JSON.stringify({
      lensCount: plan.lensCount,
      plannedLensCount: plan.plannedLensCount,
      foci: plan.foci,
      prompts,
      promptSha256: bodies.map((b) => createHash("sha256").update(b).digest("hex")),
      markerPresent: bodies.every((b) => b.includes(MARKER)),
    })}\n`,
  );
  console.error(`lens prompts: ${prompts.length} written to ${outDir}`);
}
