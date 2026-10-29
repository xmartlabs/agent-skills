#!/usr/bin/env node
// replay-lens-replay.mjs — zero-token checks for eval/lens-replay.mjs.
//
// The runner's value is that its prompt, foci, dependents block and schema
// come from the shipping workflow rather than from a copy. These checks pin
// that, plus the two offline gates its score rests on (out-of-diff and nit)
// and the raw-decode parser, which once gave false zeros on nested verifiedBy.
//
// Usage: node eval/replay-lens-replay.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { findingsFromText, gate, loadWorkflowPieces, planLenses, scoreReplicates } from "./lens-replay.mjs";
import { loadRegion } from "./extract-pure-region.mjs";
import { RETURN_SUFFIX, lensDiffBlock, lensPromptBodies } from "../scripts/build-lens-prompts.mjs";
import { WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = readFileSync(resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js"), "utf8");
const LR = readFileSync(resolve(HERE, "..", "scripts", "build-lens-prompts.mjs"), "utf8");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log("\nlens-replay — eval/lens-replay.mjs");
// Both regions load standalone, the way the runner cuts them.
const { lensPrompt: regionPrompt } = loadRegion(WORKFLOW, "LENS-PROMPT", ["lensPrompt"]).api;
check("LENS-PROMPT loads by its markers", typeof regionPrompt === "function");
const pieces = loadWorkflowPieces(SRC);

console.log("\n--- the prompt is the workflow's ---");
{
  const p = pieces.lensPrompt({
    prNumber: 1, repo: "o/r", diffBlock: "DIFF", jiraTicket: null, contextBlock: "",
    dependents: "<dependents>\nDEP\n</dependents>", highRiskSurface: "HRS", focus: "FOCUS-SENTENCE", files: null,
  });
  const marker = p.indexOf("===== YOUR PASS =====");
  check("marker present", marker > 0);
  check("dependents and field list sit above the marker", p.indexOf("<dependents>") < marker && p.indexOf("verifiedBy: REQUIRED") < marker);
  check("focus sits below the marker", p.indexOf("Restrict this pass to ONLY: FOCUS-SENTENCE") > marker);
  check("high-risk surface is the profile's", p.includes("touchesHighRiskSurface: true if it involves HRS"));
  check("the severity field lists the three levels", p.includes('- severity: "critical" | "important" | "nit"\n'));
  check("with other lenses, the rest is theirs", p.includes("Ignore everything else — other lenses cover the rest."));
}

console.log("\n--- the diff block is textually the workflow's ---");
{
  const read = "read that file instead of re-running gh pr view / gh pr diff. It is the authoritative copy of this PR's changes.";
  const tail = "Any read BEYOND the diff — surrounding code, call sites, sibling surfaces — belongs there.";
  for (const [name, s] of [["READ_PATH_INSTRUCTION", read], ["LENS_DIFF_TAIL", tail]])
    check(`${name} text matches the workflow`, SRC.includes(s) && LR.includes(s), s.slice(0, 50));
  check(
    "the pinned-checkout sentence is the HEAD-WORKTREE region's, not a copy",
    LR.includes('fn("HEAD-WORKTREE", "{ pinnedCheckoutBlock }")') && !LR.includes("Read source THERE"),
  );
  const pinned = lensDiffBlock("/p.patch", "/wt", "abc123", pieces.pinnedCheckoutBlock);
  const bare = lensDiffBlock("/p.patch", null, "abc123", pieces.pinnedCheckoutBlock);
  check("with a checkout, the lens is pointed at it", pinned.includes("is already prepared at /wt") && pinned.endsWith(tail));
  check("without one, it is told no pinned checkout exists", bare.includes("NO PINNED CHECKOUT IS AVAILABLE") && bare.endsWith(tail));
}

console.log("\n--- the shipped CLI writes the same prompts ---");
{
  let cp = null;
  try {
    cp = (await import("./client-profiles.mjs")).clientProfileByReviewer("backend");
  } catch {}
  if (!cp) console.log("  skip  no client profile in TRIBUNAL_HARNESS_DATA");
  else {
    const out = mkdtempSync(join(tmpdir(), "lens-prompts-"));
    const input = { files: ["src/services/v2/payments.ts", "src/services/v2/notes.ts"], additions: 40, deletions: 2 };
    const res = JSON.parse(
      execFileSync("node", [
        resolve(HERE, "..", "scripts", "build-lens-prompts.mjs"),
        "--pr", "7", "--repo", "o/r", "--head-sha", "abc1234", "--diff", "/d.patch", "--profile", cp.file,
        "--methodology", "/m.md", "--worktree", "/wt", "--jira", "ABC-1", "--out-dir", out,
      ], { input: JSON.stringify(input), stdio: ["pipe", "pipe", "ignore"] }).toString(),
    );
    const built = loadWorkflowPieces(readFileSync(resolve(HERE, "..", "..", "..", "workflows", "tribunal.js"), "utf8"));
    const plan = planLenses({ ...input, profile: cp.profile, plan: "workflow", selectFoci: built.selectFoci });
    const bodies = lensPromptBodies({ pieces: built, foci: plan.foci, prNumber: 7, repo: "o/r", diffPath: "/d.patch", worktree: "/wt", headSha: "abc1234", profile: cp.profile, jiraTicket: "ABC-1" });
    const files = res.prompts.map((p) => readFileSync(p, "utf8"));
    check("the CLI plans what planLenses plans", res.lensCount === plan.lensCount && res.lensCount === files.length, JSON.stringify(res.foci));
    check(
      "each file is the header, the Workflow prompt and the return format",
      files.every((f, i) => f === `METHODOLOGY FILE: /m.md\n\n${bodies[i]}${RETURN_SUFFIX(built.findingsSchema)}`),
    );
    check("the marker is in every prompt", res.markerPresent === true);
  }
}

console.log("\n--- schema ---");
{
  const s = pieces.findingsSchema;
  check("FINDINGS_SCHEMA is cut whole", s && s.type === "object" && s.properties && s.properties.findings, JSON.stringify(s).slice(0, 120));
  const item = s.properties.findings.items || {};
  check("...with the severity enum", JSON.stringify(item).includes('"nit"'));
}

console.log("\n--- foci come from the profile through selectFoci ---");
{
  const profile = {
    reviewer: "backend",
    foci: { correctness: "C", dataContract: "D", security: "S", performance: "P", errorHandling: "E" },
    riskPaths: [{ pattern: "auth", focus: "security" }],
  };
  const tiny = planLenses({ files: ["src/a.ts"], additions: 10, deletions: 1, profile, plan: "workflow", selectFoci: pieces.selectFoci });
  check("1-file PR gets one lens, correctness", tiny.lensCount === 1 && tiny.foci[0] === "C", JSON.stringify(tiny));
  const auth = planLenses({ files: ["src/auth.ts"], additions: 10, deletions: 1, profile, plan: "workflow", selectFoci: pieces.selectFoci });
  check("a forced lens takes the single slot", auth.lensCount === 1 && auth.foci[0] === "S", JSON.stringify(auth));
  const mid = planLenses({ files: ["a.ts", "b.ts", "c.ts", "d.ts", "e.ts"], additions: 300, deletions: 10, profile, plan: "workflow", selectFoci: pieces.selectFoci });
  check("5 files and 310 lines of churn plan the Workflow's 2 lenses", mid.lensCount === 2 && mid.plannedLensCount === 2, JSON.stringify(mid));
  const wf = readFileSync(WORKFLOW, "utf8");
  const bl = readFileSync(resolve(HERE, "..", "scripts", "build-lens-prompts.mjs"), "utf8");
  const exprs = [
    "fileCount <= 2 ? 1 : fileCount <= 10 ? 2 : 5;",
    "churn < 200 ? 0 : churn < 500 ? 2 : churn < 2000 ? 4 : churn < 5000 ? 6 : 8;",
    "fileCount > 12 ? Math.ceil(fileCount / FILES_PER_GROUP) : 1;",
    "const FILES_PER_GROUP = 6;",
    "Math.max(fileTier, churnTier, groupCount)",
  ];
  const missing = exprs.filter((e) => !wf.includes(e) || !bl.includes(e));
  check("the budget's expressions are the Workflow's, word for word", missing.length === 0, missing.join(" | "));
  const keys = planLenses({ files: ["src/a.ts"], profile, foci: ["errorHandling", "dataContract"], selectFoci: pieces.selectFoci });
  check("--foci picks the profile sentences verbatim", keys.foci.join() === "E,D", JSON.stringify(keys));
}

console.log("\n--- raw-decode parser ---");
{
  const nested = 'Notes first.\n```json\n{"findings":[{"title":"t","severity":"important","verifiedBy":[{"file":"a","line":1,"quote":"}"}]}],"notReached":["x"]}\n```';
  const r = findingsFromText(nested);
  check("nested verifiedBy with a brace inside a string", r && r.findings.length === 1 && r.notReached[0] === "x", JSON.stringify(r));
  const bare = 'see [1] then [{"title":"a","severity":"nit"}]';
  check("a bare array after a prose bracket", findingsFromText(bare)?.findings.length === 1);
  check("no JSON is null, not an empty list", findingsFromText("nothing here") === null);
}

console.log("\n--- offline gates ---");
{
  const files = ["src/a.ts", "src/b.test.ts"];
  check("important in the diff can block", gate({ file: "src/a.ts", severity: "important" }, files).canBlock);
  check("important outside the diff cannot", !gate({ file: "src/other.ts", severity: "important" }, files).canBlock);
  check("nit in the diff cannot", !gate({ file: "src/a.ts", severity: "nit" }, files).canBlock);
}

console.log("\n--- scoring ---");
{
  const entry = { knownReal: [{ id: "alpha-defect", file: "src/a.ts", description: "alpha defect drops rows" }] };
  const reps = [
    { ok: true, findings: [{ title: "alpha defect drops rows", file: "src/a.ts", severity: "important" }] },
    { ok: true, findings: [{ title: "alpha defect drops rows", file: "src/a.ts", severity: "nit" }] },
    { ok: true, findings: [] },
    { ok: false, findings: [] },
  ];
  const s = scoreReplicates(reps, entry, ["src/a.ts"]);
  const a = s.annotations[0];
  check("seen / raised / can-block are separate counts", a.seen === 2 && a.raised === 1 && a.canBlock === 1 && a.n === 3, JSON.stringify(a));
  check("a failed replicate is reported, not scored as a miss", s.failed === 1 && s.n === 3);
}

console.log("\n--- real outputs from the 2026-10-20 replays parse ---");
{
  const dir = process.env.TRIBUNAL_HARNESS_DATA && resolve(process.env.TRIBUNAL_HARNESS_DATA, "tribunal", "lens-replays", "2026-10-20");
  let manifest = null;
  try {
    manifest = JSON.parse(readFileSync(resolve(dir, "manifest.json"), "utf8"));
  } catch {}
  if (!manifest) console.log("  skip  TRIBUNAL_HARNESS_DATA has no lens-replays/2026-10-20");
  else {
    let agree = 0, total = 0;
    const off = [];
    for (const a of manifest.arms)
      for (const r of a.runs) {
        total++;
        const out = JSON.parse(readFileSync(resolve(dir, r.file), "utf8"));
        const got = findingsFromText(out.result);
        const n = got ? got.findings.length : null;
        if (n === r.findingCount) agree++;
        else off.push(`${r.file}: ${n} vs ${r.findingCount}`);
      }
    check(`finding counts match the archived manifest on ${total} runs`, agree === total, off.slice(0, 3).join("; "));
  }
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall lens-replay expectations hold");
