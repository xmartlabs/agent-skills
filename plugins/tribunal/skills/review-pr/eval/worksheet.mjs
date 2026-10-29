#!/usr/bin/env node
// worksheet.mjs — prepare the unjudged findings of one PR for a human to label.
//
// Clerical only. It gathers what a person needs to answer "is this a defect?"
// — the quote, the code at the reviewed commit, what the panel concluded, what
// happened to the PR — and then asks. It **proposes no bucket**, deliberately: a
// suggested answer is an anchor, and an anchored human label is a model label
// wearing a costume, which would reintroduce the model-grading-a-model problem
// the corpus exists to escape (see specs/2026-07-28-ground-truth-labelling-spec.md).
//
// Which findings are "unjudged" comes from the SAME predicate the scorer uses
// (eval/annotation-match.mjs). A worksheet with its own matcher would ask about
// findings already labelled, or skip ones nobody has.
//
// Usage:
//   node eval/worksheet.mjs <pr> [--context <lines>]
//   REPO_DIR=/path/to/repo node eval/worksheet.mjs 1796

import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { canonicalRows } from "./rows.mjs";
import { matches } from "./annotation-match.mjs";
import { CORPUS_DIR, reviewRepo } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const pr = String(process.argv[2] || "").replace(/\D/g, "");
const ctxArg = process.argv.indexOf("--context");
const CTX = ctxArg > -1 ? Number(process.argv[ctxArg + 1]) || 12 : 12;
// Hardcoded default, and recorded as such: RELEASES.md open item 27 already says
// this harness is not org-agnostic. An env override costs one line and keeps the
// next repo from needing a patch.
const REPO_DIR =
  process.env.REPO_DIR ||
  reviewRepo();

if (!pr) {
  console.log("usage: node eval/worksheet.mjs <pr> [--context <lines>]");
  process.exit(1);
}

const entryPath = join(CORPUS_DIR, `pr-${pr}.json`);
if (!existsSync(entryPath)) {
  console.log(`No corpus entry at ${entryPath}. Create it first — see eval/README.md.`);
  process.exit(1);
}
const entry = JSON.parse(readFileSync(entryPath, "utf8"));

// Latest canonical run, the same one score.mjs scores. Anything else would ask
// about findings the scorer is not looking at.
const runs = canonicalRows().filter((r) => String(r.pr) === pr);
const run = runs[runs.length - 1];
if (!run) {
  console.log(`No canonical metrics row for PR #${pr} — run the review first (--no-post).`);
  process.exit(1);
}

const BUCKETS = ["knownReal", "knownBogus", "knownAccurateNonDefect", "undecidable"];
const labelled = (f) => BUCKETS.some((b) => (entry[b] || []).some((a) => matches(f, a)));
const queue = (run.findings || []).filter((f) => f.survived && !labelled(f));

const sh = (cmd, args, cwd) => {
  try {
    return execFileSync(cmd, args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  } catch {
    return null;
  }
};

const slug = (s) =>
  String(s || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .split("-")
    .slice(0, 5)
    .join("-");

console.log(`# Labelling worksheet — PR #${pr}`);
console.log(`\n${entry.title || ""}`);
console.log(`commit reviewed: ${run.headSha}`);
console.log(`run: ${run.ts}  |  harness: ${run.harnessVersion || "(pre-versioning)"}`);
const prView = sh("gh", ["pr", "view", pr, "--json", "state,mergedAt,url", "--jq", ".state + \" \" + (.mergedAt // \"-\") + \" \" + .url"], REPO_DIR);
console.log(`PR outcome: ${prView ? prView.trim() : "(gh unavailable)"}`);
console.log(`\n${queue.length} finding(s) still unjudged, of ${(run.findings || []).filter((f) => f.survived).length} that survived the panel.`);
console.log(
  `\nAnswer each with ONE bucket: real | bogus | accurate | undecidable.` +
    `\n  real        a defect — the harness was right to raise it` +
    `\n  bogus       the claim is FALSE (wrong premise; the code does not do that)` +
    `\n  accurate    true, but a nit/preference, not a defect` +
    `\n  undecidable cannot be settled from the code — say what WOULD settle it`,
);

queue.forEach((f, i) => {
  console.log(`\n${"=".repeat(78)}`);
  console.log(`## ${i + 1}/${queue.length}  ${f.title}`);
  console.log(`${"=".repeat(78)}`);
  console.log(`file: ${f.file || "(none recorded)"}${f.line ? `:${f.line}` : ""}`);
  console.log(`severity: ${f.severity}   highRisk: ${f.touchesHighRiskSurface}   behaviourChanging: ${f.isBehaviorChanging}`);
  console.log(`lens: ${f.lens}`);

  console.log(`\n--- the finding's own evidence, verbatim ---`);
  // `findings[].evidence` is required of every lens and is what eval/README.md
  // calls "the corpus" — and the metrics row does not persist it (1 of 284
  // recorded findings has it). Until that is fixed, the resolver's own record is
  // the only surviving pointer: it kept the line it matched, just not the text.
  console.log(f.evidence ? String(f.evidence).split("\n").map((l) => `  ${l}`).join("\n") : "  (not persisted on the row — see RELEASES.md open item 29)");
  const res = f.evidenceResolution || {};
  if (res.resolvedLine)
    console.log(`  resolver matched line ${res.resolvedLine} (${res.matchKind}${res.lowConfidence ? ", LOW CONFIDENCE" : ""}${res.claimedLine && res.claimedLine !== res.resolvedLine ? `; lens claimed ${res.claimedLine}` : ""})`);

  // Pinned to the reviewed commit. The working tree sits on `development` and
  // would answer a different question — post-merge code, not what was reviewed.
  if (f.file && run.headSha) {
    const blob = sh("git", ["show", `${run.headSha}:${f.file}`], REPO_DIR);
    console.log(`\n--- ${f.file} at ${String(run.headSha).slice(0, 8)} ---`);
    if (!blob) {
      console.log(`  (could not read; commit may not be fetched locally: git -C ${REPO_DIR} fetch origin ${run.headSha})`);
    } else {
      const lines = blob.split("\n");
      // `f.line` is not persisted either; the resolver's `resolvedLine` is, and
      // it is the better anchor anyway — it is where the quote was actually
      // found, not where the lens said it was.
      const anchor = Number(f.line) || Number((f.evidenceResolution || {}).resolvedLine) || 0;
      const from = anchor ? Math.max(0, anchor - 1 - CTX) : 0;
      const to = anchor ? Math.min(lines.length, anchor + CTX) : Math.min(lines.length, CTX * 2);
      for (let n = from; n < to; n++)
        console.log(`  ${String(n + 1).padStart(5)}${n + 1 === anchor ? " >" : "  "} ${lines[n]}`);
      if (!anchor) console.log(`  … (no line recorded; showing the head of the file)`);
    }
  }

  console.log(`\n--- what the run concluded ---`);
  console.log(`  survived: ${f.survived}   adjudicated: ${f.adjudicated}   refuteFraction: ${f.refuteFraction ?? "n/a"} (kill at ${f.threshold ?? "?"})`);
  for (const v of f.votes || [])
    console.log(`  vote ${v.source}: ${v.verdict} @${v.confidence} coreStands=${v.coreStands} type=${v.refutationType ?? "-"} killReason=${v.killReason ?? "-"}`);
  if (!(f.votes || []).length) console.log(`  no votes recorded — this finding faced no panel`);
  if (f.debate && f.debate.fired) console.log(`  debate: ${f.debate.outcome} — ${f.debate.rationale}`);

  console.log(`\n--- your call ---`);
  console.log(`  bucket: ______________   (real | bogus | accurate | undecidable)`);
  console.log(`  why:    ______________   (required; for undecidable, what WOULD settle it)`);
  console.log(`  suggested id: ${slug(f.title)}`);
  console.log(
    `  NOTE: the id and description are a MATCHING KEY, not prose — keep the distinctive`,
  );
  console.log(
    `  nouns (function, field, table names). Generic words match the neighbours instead.`,
  );
});

if (!queue.length) {
  console.log(`\nNothing to judge: every surviving finding for PR #${pr} is already labelled.`);
  console.log(`Once that is true for a PR, its entry can move to status "annotated".`);
}
