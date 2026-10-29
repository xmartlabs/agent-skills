#!/usr/bin/env node
// replay-critic-gaps.mjs — the completeness critic's gaps reach the panel.
//
// Until v101 the critic ran after the panel and handed prose to the synthesis
// prompt, so a defect only it found could never block. In the stored
// 2026-10-21 runs it named two such defects (a socket ping missing the key its
// consumer dedupes on, and a lookup that turns a database error into "not
// found"). These checks pin where it runs and what happens to its gaps.
//
// Usage: node eval/replay-critic-gaps.mjs

import { readFileSync } from "node:fs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const src = readFileSync(WORKFLOW, "utf8");
const start = src.indexOf("// --- Completeness critic, before the panel ---");
const end = src.indexOf("const panelEligible = panelInput.filter((f) => needsPanel(f));");
const block = start !== -1 && end > start ? src.slice(start, end) : "";

console.log("\n--- where it runs ---");
check("the critic block was located, right before the panel set is taken", block.length > 0, `start=${start} end=${end}`);
check("there is exactly one critic dispatch", (src.match(/label: "completeness-critic"/g) || []).length === 1);
check("it runs in the Verify phase, before any panel", block.includes('label: "completeness-critic",\n      phase: "Verify",'));
check("it still skips PRs of two files or fewer", block.includes("if (fileCount > 2) {"));
check("it sees the lens candidates, not the panel's survivors", block.includes("Findings the lenses raised (not adjudicated yet") && !block.includes("survivingTitles"));

console.log("\n--- what it returns ---");
check("its answer is structured: a summary and findings in the lens shape", block.includes("findings: FINDINGS_SCHEMA.properties.findings,") && block.includes('required: ["summary", "findings"]') && block.includes("schema: CRITIC_SCHEMA,"));
check("something it could not check stays out of the findings", /Something you could not check is not a finding: leave it in the summary\./.test(block));
check("a caller-side break is anchored at the changed declaration", /anchor it at the changed declaration and put the caller's line in verifiedBy/.test(block));
check("the evidence rule against invented quotes is kept", block.includes("A contradiction without a quote is not reportable"));
check("the summary still reaches the synthesis prompt", block.includes('criticOutput = critic && typeof critic.summary === "string" ? critic.summary : "";') && src.includes('${criticOutput || "(not run — small PR)"}'));

console.log("\n--- what happens to its gaps ---");
check("each gap passes the same out-of-diff gate, and a dropped one is listed", block.includes("const kept = reanchorOutOfDiff(g, prContext.files, blastRadius);") && /droppedPrePanel\.push\(\{[^}]*completeness-critic gap out-of-diff/.test(block));
check("a gap at the place of a lens candidate, or of an earlier gap, is not added twice", block.includes("if (panelInput.some((f) => samePlace(f, kept)) || criticGaps.some((f) => samePlace(f, kept))) continue;"));
check("kept gaps join the panel input with their own slot ids and lens", /panelInput = panelInput\.concat\(raisedByCritic\);/.test(block) && block.includes("slotId: panelInput.length + i,") && block.includes('lens: "completeness-critic",'));
check("risk comes from the profile's riskPaths too, as for sweep findings", block.includes("riskPathTouched(g.file, riskPathRules(PROFILE))"));
check("the panel rule applies after the gaps are in", src.indexOf("panelInput = panelInput.concat(raisedByCritic);") < src.indexOf("const panelEligible = panelInput.filter((f) => needsPanel(f));"));
check("the row counts them", src.includes("completenessCriticGaps: criticGaps.length,"));

console.log("\n--- fault put back: the old position ---");
{
  const moved = src.replace('      phase: "Verify",\n      model: reviewModel,\n      agentType: LENS_AGENT,\n      schema: CRITIC_SCHEMA,', '      phase: "Synthesize",\n      model: reviewModel,\n      agentType: LENS_AGENT,\n      schema: CRITIC_SCHEMA,');
  const b = moved.slice(moved.indexOf("// --- Completeness critic, before the panel ---"), moved.indexOf("const panelEligible = panelInput.filter((f) => needsPanel(f));"));
  check("a critic moved back to Synthesize turns the phase check red", moved !== src && !b.includes('label: "completeness-critic",\n      phase: "Verify",'));
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall critic-gap expectations hold");
