#!/usr/bin/env node
// replay-external-scope.mjs — Tier 0, 0 tokens. What an external refuter head
// (Codex, Gemini) is allowed to read when it judges one finding.
//
// The failure it guards: an external head scoped to the diff cannot check a
// cross-file claim, so it abstains as `confined`. From 2026-09-12 to 2026-10-22
// that was every Codex panel failure on record. On PR #2185 the sweep raised
// "the write helpers have no production caller" with verifiedBy anchors in
// src/jobs/sync_worker.ts and src/jobs/services/remote.job.ts, neither in the
// diff, and the refuter prompt Codex got named neither and allowed no search.
// Codex answered CANNOT-ASSESS. This runs the real verifiedFiles and
// externalScopeLines, cut out of the workflow by the DIFF-SECTIONING markers,
// over that finding.
//
// Usage: node eval/replay-external-scope.mjs

import { readFileSync } from "node:fs";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { api, violations } = loadRegion(WORKFLOW, "DIFF-SECTIONING", [
  "verifiedFiles",
  "externalScopeLines",
]);
const { verifiedFiles, externalScopeLines } = api;

console.log(`\nDIFF-SECTIONING region extracted from ${WORKFLOW}`);
check("region references no run state", violations.length === 0, violations.join(" | "));

const PR_FILES = [
  "src/controllers/v2/syncResults.ts",
  "src/controllers/v2/syncResults.types.ts",
  "src/routes/v2/index.ts",
  "src/routes/v2/syncResults.routes.ts",
  "src/services/v2/syncResults.ts",
  "src/services/v2/syncResults.test.ts",
  "src/tests/smoke/sync-results.test.ts",
];

const PR2185 = {
  file: "src/services/v2/syncResults.ts",
  line: 429,
  title: "Audit endpoint reads a feed that production jobs never populate",
  description:
    "After a normal CREATE_REMOTE_RECORD job, GET /sync-results remains empty: the repository has no production caller of the result write helpers, and the worker calls the remote API directly.",
  evidence: "prisma.syncResults.findMany({",
  verifiedBy: [
    { file: "src/jobs/sync_worker.ts", line: 65, quote: "case SyncJobNames.CREATE_REMOTE_RECORD:" },
    { file: "src/jobs/services/remote.job.ts", line: 18, quote: ") => await remoteService.createRecord(input);" },
  ],
};

console.log(`\n--- verifiedFiles: anchors outside the diff ---`);
{
  const files = verifiedFiles(PR2185);
  check(
    "the #2185 anchors reach the scope although neither file is in the diff",
    files.length === 2 && files.includes("src/jobs/sync_worker.ts") && files.includes("src/jobs/services/remote.job.ts"),
    JSON.stringify(files),
  );
}
{
  const f = { file: "a.ts", verifiedBy: [{ file: "a.ts", line: 1 }, { file: "./b.ts", line: 2 }, { file: "b.ts", line: 9 }] };
  const files = verifiedFiles(f);
  check("the finding's own file is not repeated", !files.includes("a.ts"), JSON.stringify(files));
  check("a ./ prefix is dropped and duplicates collapse", files.length === 1 && files[0] === "b.ts", JSON.stringify(files));
}
{
  const f = {
    file: "a.ts",
    verifiedBy: [
      { file: "/etc/passwd", line: 1 },
      { file: "../outside/repo.ts", line: 1 },
      { file: "src/../../up.ts", line: 1 },
      { file: "src/has space.ts", line: 1 },
      { file: "src/x.ts; rm -rf /", line: 1 },
      { file: "", line: 1 },
      { line: 3 },
      null,
    ],
  };
  check("absolute, parent, spaced, shell-metachar and empty paths widen nothing", verifiedFiles(f).length === 0, JSON.stringify(verifiedFiles(f)));
}
{
  const f = { file: "a.ts", verifiedBy: ["b.ts", "c.ts", "d.ts", "e.ts", "f.ts", "g.ts"].map((file) => ({ file, line: 1 })) };
  check("the scope is capped at 4 anchor files", verifiedFiles(f).length === 4, JSON.stringify(verifiedFiles(f)));
}
{
  check("no verifiedBy yields nothing", verifiedFiles({ file: "a.ts" }).length === 0);
  check("a non-array verifiedBy does not throw", verifiedFiles({ file: "a.ts", verifiedBy: "x.ts" }).length === 0);
  check("a null finding does not throw", verifiedFiles(null).length === 0);
}

console.log(`\n--- externalScopeLines: what the prompt says ---`);
{
  const lines = externalScopeLines(PR2185, PR_FILES);
  const text = lines.join("\n");
  check("the #2185 prompt names sync_worker.ts", text.includes("src/jobs/sync_worker.ts"), text);
  check("and remote.job.ts", text.includes("src/jobs/services/remote.job.ts"));
  check("and says files outside the PR are in scope", /including any this PR did not touch/.test(text));
  check("an absence claim may be settled with a search", /ABSENT/.test(text) && /git grep -n/.test(text) && /up to 3 searches/.test(text));
}
{
  // A file both cited in prose and anchored is listed once, under the cited line.
  const f = {
    file: "src/controllers/v2/syncResults.ts",
    description: "the route in syncResults.routes.ts skips the role check",
    evidence: "",
    verifiedBy: [{ file: "src/routes/v2/syncResults.routes.ts", line: 14, quote: "x" }],
  };
  const lines = externalScopeLines(f, PR_FILES);
  const mentions = lines.join("\n").split("src/routes/v2/syncResults.routes.ts").length - 1;
  check("a file both cited and anchored is listed once", mentions === 1, JSON.stringify(lines));
  check("and no anchor line is emitted for it", !lines.some((l) => l.includes("evidence anchors")), JSON.stringify(lines));
}
{
  const lines = externalScopeLines({ file: "a.ts", description: "", evidence: "" }, []);
  check("a finding with no citations and no anchors gets only the search line", lines.length === 1 && /ABSENT/.test(lines[0]), JSON.stringify(lines));
}

console.log(`\n--- wiring ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  const start = src.indexOf("function externalPromptText(finding)");
  const body = src.slice(start, src.indexOf("\n}\n", start));
  check("externalPromptText builds its scope from externalScopeLines", /\.\.\.externalScopeLines\(finding, prContext\.files\)/.test(body));
  check("and no longer builds the cited line inline", !/const cited = citedFiles\(/.test(body));
  check(
    "CANNOT-ASSESS no longer covers a call-site the anchors or a search reach",
    /a sibling call-site that neither the finding's files nor the searches above reach/.test(body),
  );
}

console.log(`
What this does NOT establish:
  · That Codex votes instead of abstaining once the prompt widens. Only a live
    run shows that, and #2185 is the vehicle: re-run it pinned and read the
    codex slot's artifact.
  · That the anchors exist. The resolver checks them after the panel; here a
    path only has to be a plain repo-relative path.
`);

if (failures.length) {
  console.log(`${failures.length} check(s) FAILED: ${failures.join(", ")}\n`);
  process.exit(1);
}
console.log(`All assertions passed (0 tokens).\n`);
