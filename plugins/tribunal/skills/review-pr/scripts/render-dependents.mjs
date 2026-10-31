#!/usr/bin/env node
// render-dependents.mjs — the Workflow's <dependents> block for the in-context
// path.
//
// The Workflow lists the unchanged callers of what a PR changed in every lens
// prompt; it was the strongest lever measured on a cross-file miss (2 of 10
// runs raised it without the block, 6 of 6 with it). The in-context path never
// had it, and that is the path small PRs take. This prints the same block: the
// callers come from lib/blast-radius.mjs and the text from the shipped
// workflow's DEPENDENTS-BLOCK region, cut out by its markers, so the two paths
// cannot word it differently.
//
// Usage: node render-dependents.mjs <diff-file> <repo-dir> --rev <HEAD_SHA>
// stdout: the block, or nothing when no unchanged caller was found.
// stderr: one line saying how many callers were listed, or why none could be.
// Exits 0 either way: a missing block degrades the review, it does not stop it.

import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { blastRadius, makeRepo } from "../../../rust/crates/tribunal-context/src/js/blast-radius.mjs";
import { attentionFacts, loadTypescript } from "../../../rust/crates/tribunal-context/src/js/attention-facts.mjs";

const WORKFLOW = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "workflows", "tribunal.js");

export function dependentsBlockFromWorkflow(src = readFileSync(WORKFLOW, "utf8")) {
  const BEGIN = "// ===== BEGIN DEPENDENTS-BLOCK =====";
  const END = "// ===== END DEPENDENTS-BLOCK =====";
  const b = src.indexOf(BEGIN);
  const e = src.indexOf(END, b);
  if (b === -1 || e === -1) throw new Error(`no DEPENDENTS-BLOCK region in ${WORKFLOW}`);
  return new Function(`${src.slice(b + BEGIN.length, e)}\nreturn dependentsBlock;`)();
}

export function renderDependents(diff, repoDir, rev) {
  // An unreadable commit would list zero callers and read as a PR nobody calls.
  try {
    execFileSync("git", ["-C", repoDir, "cat-file", "-e", `${rev}^{commit}`], { stdio: "ignore" });
  } catch {
    throw new Error(`${rev} is not in ${repoDir}; run git fetch origin ${rev} first`);
  }
  const repo = makeRepo(repoDir, rev);
  const ts = loadTypescript([repoDir, process.cwd()]);
  const br = blastRadius(diff, repo, { ts });
  br.attention = attentionFacts(diff, repo, ts);
  return { block: dependentsBlockFromWorkflow()(br), listed: br.callers.length, total: br.total, facts: br.attention.facts.length, parser: br.attention.parser };
}

const invokedDirectly = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

if (invokedDirectly) {
  const [diffFile, repoDir] = process.argv.slice(2);
  const at = process.argv.indexOf("--rev");
  const rev = at > 0 ? process.argv[at + 1] : null;
  if (!diffFile || !repoDir || !rev) {
    console.error("usage: render-dependents.mjs <diff-file> <repo-dir> --rev <HEAD_SHA>");
    process.exit(2);
  }
  try {
    const r = renderDependents(readFileSync(diffFile, "utf8"), repoDir, rev);
    if (r.block) process.stdout.write(`${r.block}\n`);
    console.error(`dependents: listed ${r.listed} of ${r.total}; attention facts ${r.facts}${r.parser ? "" : " (no typescript package to parse with)"}`);
  } catch (err) {
    console.error(`dependents: none, ${err.message}`);
  }
}
