// build-tier1-prompt.mjs — assemble one Tier-1 arm: a recorded refuter's exact
// prompt, with its "go read the code" instruction replaced by the code itself.
//
// The experiment this feeds asks one question the offline coverage scorer
// structurally cannot: does a refuter handed the package reach the SAME VERDICT,
// and does it still take 23-40 turns? Everything else about the prompt is held
// constant on purpose — same finding, same lens, same diff, same pinned SHA — so
// that the package is the only variable.
//
// Tools stay available in the experimental arm. That is the shipping design, not
// a concession: the package is an accelerator, so the worst case has to be
// today's cost rather than a worse review. `filesOpenedOutsidePackage` is how the
// arm reports whether the fallback fired.
//
// Usage:
//   node eval/build-tier1-prompt.mjs "<title prefix>" <outdir>
//
// Prints the baseline the arm will be compared against, and writes
//   <outdir>/tier1-<slug>-prompt.txt      the prompt to dispatch
//   <outdir>/tier1-<slug>-baseline.json   turns, input, and the recorded vote

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { findRunDirs, parseTranscript } from "./agent-cost.mjs";
import { readRun, parseSingleRefuterCase, votesFromResult } from "./refine-attribution.mjs";
import { assemblePackage, extractReads, resolveToken } from "./refuter-package.mjs";
import { projectDir, reviewRepo } from "./paths.mjs";

const PROJECT = projectDir();
const REPO = reviewRepo();

const [titlePrefix, outDir] = process.argv.slice(2);
if (!titlePrefix || !outDir) {
  console.error('usage: node eval/build-tier1-prompt.mjs "<title prefix>" <outdir>');
  process.exit(2);
}

const slug = titlePrefix
  .toLowerCase()
  .replace(/[^a-z0-9]+/g, "-")
  .replace(/^-|-$/g, "")
  .slice(0, 40);

for (const dir of findRunDirs(PROJECT)) {
  const run = readRun(dir);
  if (!run || !run.embeddedRow) continue;
  const row = run.embeddedRow;
  const results = new Map();
  for (const l of readFileSync(join(dir, "journal.jsonl"), "utf8").split("\n")) {
    if (!l.trim()) continue;
    try {
      const r = JSON.parse(l);
      if (r.type === "result") results.set(r.agentId, r.result);
    } catch {
      /* a truncated journal line is skipped, not fatal */
    }
  }

  for (const f of readdirSync(dir)) {
    const m = /^agent-(\w+)\.jsonl$/.exec(f);
    if (!m) continue;
    const text = readFileSync(join(dir, f), "utf8");
    const parsed = parseTranscript(text);
    if (!/^REFUTE — do not confirm — this code review finding/.test(parsed.promptHead)) continue;
    const first = text
      .split("\n")
      .filter(Boolean)
      .map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return null;
        }
      })
      .find((r) => r && r.type === "user");
    const prompt = first && typeof first.message.content === "string" ? first.message.content : "";
    const c = parseSingleRefuterCase(prompt);
    if (!c || !c.title.startsWith(titlePrefix)) continue;
    const vote = (votesFromResult(results.get(m[1]))[0] || {}).vote;
    if (!vote) continue;

    const repoFiles = execFileSync("git", ["-C", REPO, "ls-tree", "-r", "--name-only", row.headSha], {
      encoding: "utf8",
      maxBuffer: 1 << 26
    })
      .split("\n")
      .filter(Boolean);
    const pkg = assemblePackage(
      { ...c, description: prompt.slice(prompt.indexOf(c.title)) },
      { prFiles: run.prFiles, repoFiles, variant: "cited-in-pr" }
    );
    const reads = [...new Set(extractReads(text).map((r) => resolveToken(r.path, repoFiles)).filter(Boolean))];
    const missed = reads.filter((r) => !pkg.paths.includes(r));

    const blocks = pkg.paths.map((p) => {
      let body = "";
      try {
        body = execFileSync("git", ["-C", REPO, "show", `${row.headSha}:${p}`], {
          encoding: "utf8",
          maxBuffer: 1 << 26,
          stdio: ["ignore", "pipe", "ignore"]
        });
      } catch {
        body = "(file not present at this commit)";
      }
      return `----- ${p} (at HEAD ${row.headSha.slice(0, 8)}) -----\n${body}`;
    });

    const readAtHead = new RegExp(
      `Read the surrounding code and call sites at HEAD \\(${row.headSha}\\)[^\\n]*`
    );
    if (!readAtHead.test(prompt)) {
      console.error(`  ! the "read at HEAD" line was not found — prompt shape changed; skipping`);
      continue;
    }
    const replacement =
      `The files you need are INLINED BELOW, read verbatim from the repo at HEAD (${row.headSha}). ` +
      `They are the finding's own file plus every file its description cites. Use these instead of fetching them.\n\n` +
      `You still have your tools. If something you need is NOT in this package, go and read it at HEAD as usual — ` +
      `the package is an accelerator, not a restriction. When you are done, say in \`reason\` whether you had to open ` +
      `anything outside it, and which.\n\n` +
      `===== PRE-ASSEMBLED CONTEXT =====\n${blocks.join("\n\n")}\n===== END PRE-ASSEMBLED CONTEXT =====`;

    let out = prompt.replace(readAtHead, replacement);
    // The recorded prompt points at a diff in ANOTHER session's scratchpad, which
    // no longer exists. Repoint it, and fetch the diff if it is not there — the
    // baseline arm had a diff, so an arm without one is not the same experiment.
    // Keyed by pr AND sha. Two arms of the same PR at DIFFERENT commits collided
    // under a pr-only name, so the second arm would have been handed the first
    // arm's diff — a different tree, silently, in the one experiment whose whole
    // design is "hold everything constant but the package".
    const diffPath = join(outDir, `tier1-diff-${row.pr}-${row.headSha.slice(0, 8)}.patch`);
    out = out.replace(/\/private\/tmp\/[^\s]*peer-review-diff-\d+\.patch/g, diffPath);

    writeFileSync(join(outDir, `tier1-${slug}-prompt.txt`), out);
    writeFileSync(
      join(outDir, `tier1-${slug}-baseline.json`),
      JSON.stringify(
        {
          pr: row.pr,
          headSha: row.headSha,
          runId: run.runId,
          agentId: m[1],
          title: c.title,
          file: c.file,
          baseline: {
            turns: parsed.turns,
            inputTokens: parsed.inputTokens,
            verdict: vote.verdict,
            confidence: vote.confidence,
            coreStands: !!vote.coreStands
          },
          package: pkg.paths,
          historicallyRead: reads,
          uncoveredByPackage: missed,
          diffPath
        },
        null,
        1
      )
    );
    console.log(`pr${row.pr} ${row.headSha.slice(0, 8)}  ${c.title}`);
    console.log(
      `  baseline: ${parsed.turns} turns, ${(parsed.inputTokens / 1e6).toFixed(2)}M input, ` +
        `${vote.verdict} @ ${vote.confidence}, coreStands=${!!vote.coreStands}`
    );
    console.log(`  package (${pkg.paths.length}): ${pkg.paths.join(", ")}`);
    console.log(`  historically read but NOT in package (${missed.length}): ${missed.join(", ") || "none"}`);
    console.log(`  prompt: ${join(outDir, `tier1-${slug}-prompt.txt`)}`);
    console.log(`  diff expected at: ${diffPath}`);
    process.exit(0);
  }
}
console.error(`no single refuter found with a title starting "${titlePrefix}"`);
process.exit(1);
