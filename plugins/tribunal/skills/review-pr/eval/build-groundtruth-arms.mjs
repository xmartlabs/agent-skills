// build-groundtruth-arms.mjs — the experiment the corpus makes possible.
//
// The Tier-1 rig (eval/build-tier1-prompt.mjs) compares a packaged refuter
// against the RECORDED baseline for the same finding. That answers "does the
// package change the answer", which is the cost question. It cannot answer "is
// the answer right", because the baseline is another model.
//
// This builds the other experiment: take a finding whose truth is KNOWN from
// outside the harness — a later fix commit, a production incident — and refute
// it twice, once with a package and once without. Correct verdict for a
// knownReal finding is NOT_REFUTED. Now a disagreement between the arms has a
// direction: one of them is wrong, and the corpus says which.
//
// Two rules this file exists to enforce, both learned by breaking them:
//
//   1. BOTH ARMS GET THE IDENTICAL CLAIM. The only variable is the package. The
//      claim text is written here, once, and used by both.
//   2. THE CLAIM MUST NOT LEAK THE FIX. A corpus `description` is written after
//      the fact and often names the countermeasure ("the BE CLAUDE.md records
//      this PR as the reason the X skill exists"). Feeding that to a refuter
//      hands it the answer. Descriptions here are stated as a lens at the time
//      would have stated them: the mechanism and its consequence, nothing about
//      what happened afterwards. Where a corpus description could not be
//      stripped without inventing detail, the case is left OUT rather than
//      paraphrased into something the corpus does not support — PR #1938 is
//      excluded for exactly that reason.
//
// Usage: node eval/build-groundtruth-arms.mjs <outdir>

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { CORPUS_DIR, reviewRepo } from "./paths.mjs";

const REPO = reviewRepo();
const outDir = process.argv[2];
if (!outDir) {
  console.error("usage: node eval/build-groundtruth-arms.mjs <outdir>");
  process.exit(2);
}

const has = (sha) => {
  try {
    execFileSync("git", ["-C", REPO, "cat-file", "-e", `${sha}^{commit}`], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
};
const show = (sha, p) =>
  execFileSync("git", ["-C", REPO, "show", `${sha}:${p}`], {
    encoding: "utf8",
    maxBuffer: 1 << 26,
    stdio: ["ignore", "pipe", "ignore"]
  });

// The cases are real bugs from the repository the corpus was recorded on, so
// they live with the corpus in the private test data, as groundtruth-cases.json
// next to the pr-<n>.json files: an array of {id, pr, sha, truth, corpusId,
// title, file, severity, description, package}. Every entry is knownReal in the
// corpus, so the correct verdict is NOT_REFUTED for all of them.
const CASES_FILE = join(CORPUS_DIR, "groundtruth-cases.json");
let CASES;
try {
  CASES = JSON.parse(readFileSync(CASES_FILE, "utf8"));
} catch (e) {
  console.error(`needs the private test data: cannot read ${CASES_FILE} (${e.message})`);
  process.exit(1);
}

const RETURN_SPEC = `
Return: verdict REFUTED or NOT_REFUTED, refutationType, confidence 0..1, coreStands, reason.`;

const corpusRepo = (pr) => JSON.parse(readFileSync(join(CORPUS_DIR, `pr-${pr}.json`), "utf8")).repo;

let built = 0;
for (const c of CASES) {
  if (!has(c.sha)) {
    console.log(`SKIP ${c.id}: commit ${c.sha.slice(0, 8)} not present locally`);
    continue;
  }
  let ghRepo;
  try {
    ghRepo = corpusRepo(c.pr);
  } catch (e) {
    console.log(`SKIP ${c.id}: no readable corpus entry for PR #${c.pr} in ${CORPUS_DIR} (${e.message})`);
    continue;
  }
  if (!ghRepo) {
    console.log(`SKIP ${c.id}: corpus entry for PR #${c.pr} names no repo`);
    continue;
  }
  const head = `REFUTE — do not confirm — this code review finding on PR #${c.pr} in repo ${ghRepo}.
Lens: correctness/falsification — find the input or code path that proves this is NOT a real problem.

Finding: ${c.title}
File: ${c.file}
Severity: ${c.severity}
Description: ${c.description}
`;

  writeFileSync(
    `${outDir}/gt2-${c.id}-control.txt`,
    head +
      `
Read the surrounding code and call sites at HEAD (${c.sha}) to check whether the claim actually bites.` +
      RETURN_SPEC
  );

  let blocks;
  try {
    blocks = c.package.map((p) => `----- ${p} (at HEAD ${c.sha.slice(0, 8)}) -----\n${show(c.sha, p)}`);
  } catch (e) {
    console.log(`SKIP ${c.id}: could not read a package file — ${e.message}`);
    continue;
  }
  writeFileSync(
    `${outDir}/gt2-${c.id}-package.txt`,
    head +
      `
The file(s) this finding concerns are INLINED BELOW, read verbatim from the repo at HEAD (${c.sha}). Use these instead of fetching them. You still have your tools: if you need something NOT in this package, read it at HEAD as usual.

===== PRE-ASSEMBLED CONTEXT =====
${blocks.join("\n\n")}
===== END PRE-ASSEMBLED CONTEXT =====` +
      RETURN_SPEC
  );

  built++;
  console.log(
    `${c.id}  pr#${c.pr}  truth=${c.truth}  package ~${Math.round(blocks.join("").length / 4)} tokens  ` +
      `-> gt2-${c.id}-{control,package}.txt`
  );
}
console.log(`\n${built} case(s) built, ${built * 2} arms.`);
console.log(`Correct verdict is NOT_REFUTED for every case here — all are knownReal in eval/corpus/.`);
