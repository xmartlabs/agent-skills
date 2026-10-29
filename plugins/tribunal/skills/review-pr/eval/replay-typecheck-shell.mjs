#!/usr/bin/env node
// replay-typecheck-shell.mjs — Tier 0, 0 tokens. The grounded typecheck's shell
// tail has to survive the shell the Bash tool actually runs, which is zsh on
// macOS.
//
// The failure it guards: the tail stored tsc's error lines in `LINES=$(...)`.
// In zsh, LINES is a special integer parameter, so the assignment is evaluated
// as arithmetic. A diagnostic like `src/a.ts(12,3): error TS2339` divides by
// zero and aborts the script, and a clean run stores 0, so `[ -z "$LINES" ]` is
// false and the script prints `0` instead of NO_DIAGNOSTICS. Either way
// classifyTypecheckText returned "degraded" and the grounded check never ran.
// replay-embedded-bash runs every snippet under bash, where LINES is an
// ordinary variable, so it could not see this.
//
// Three checks:
//   1. No bash snippet in the workflow assigns a zsh special parameter.
//   2. The real tail lines, cut from the workflow source, run under zsh and
//      bash with one diagnostic and with none, and the real classifier (the
//      TSC-ORACLE region) reads "diagnostics" and "clean".
//   3. The schema-drift guard skips the check exactly when the ambient
//      checkout's Prisma schema differs from the head's.
//
// Usage: node eval/replay-typecheck-shell.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

const src = readFileSync(WORKFLOW, "utf8");

// zsh parameters whose assignment does not store the string as written.
// SECONDS is left out on purpose: `SECONDS=0` is the intended timer reset and
// behaves the same in bash and zsh.
const ZSH_SPECIAL = [
  "LINES",
  "COLUMNS",
  "RANDOM",
  "LINENO",
  "HISTCMD",
  "PPID",
  "status",
  "pipestatus",
  "path",
  "argv",
  "options",
  "EPOCHSECONDS",
];
const assignRe = new RegExp(`(^|[\\s;(&|])(${ZSH_SPECIAL.join("|")})=`, "");
const offending = [];
src.split("\n").forEach((line, i) => {
  if (/^\s*\/\//.test(line)) return;
  const m = line.match(assignRe);
  if (m) offending.push(`${i + 1}: ${line.trim()}`);
});
console.log(`\nzsh special parameters assigned in ${WORKFLOW}`);
check("no snippet assigns a zsh special parameter", offending.length === 0, offending.join(" | "));

// The tail: the assignment that keeps tsc's error lines and the line that
// prints them or NO_DIAGNOSTICS. Each copy is evaluated as the template literal
// it lives in, so the shell text is exactly what reaches the agent.
const lines = src.split("\n");
const tails = [];
lines.forEach((line, i) => {
  if (!/^\s*\w+=\$\(printf .*error TS.*head -500\)\s*$/.test(line)) return;
  const next = lines[i + 1] || "";
  if (!/NO_DIAGNOSTICS/.test(next)) return;
  const asShell = (s) => new Function(`return \`${s}\`;`)();
  tails.push({ at: i + 1, text: `${asShell(line.trim())}\n${asShell(next.trim())}` });
});
console.log(`\ntypecheck tails found: ${tails.length}`);
check("both typecheck tails are found (fused probe and standalone agent)", tails.length === 2, `found ${tails.length}`);

const { api } = loadRegion(WORKFLOW, "TSC-ORACLE", ["classifyTypecheckText"]);
const { classifyTypecheckText } = api;

const DIAG = "src/jobs/a.ts(12,3): error TS2339: Property 'x' does not exist on type 'Y'.";
const hasShell = (sh) => spawnSync(sh, ["-c", "true"]).status === 0;
const shells = ["bash", "zsh"].filter((sh) => {
  if (hasShell(sh)) return true;
  console.log(`  skip  ${sh} is not installed here`);
  return false;
});
check("zsh is available to run the tails", shells.includes("zsh"), "the shell this check exists for is missing");

for (const tail of tails)
  for (const sh of shells)
    for (const [label, out, want] of [
      ["one diagnostic", DIAG, "diagnostics"],
      ["no diagnostics", "", "clean"],
    ]) {
      let text = "";
      try {
        text = execFileSync(sh, ["-c", tail.text], {
          env: { ...process.env, OUT: out },
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        });
      } catch (err) {
        text = `${err.stdout || ""}${err.stderr || ""}`;
      }
      const got = classifyTypecheckText(text);
      check(
        `line ${tail.at} under ${sh}, ${label}: classified "${want}"`,
        got === want,
        `got "${got}" from ${JSON.stringify(text.trim().slice(0, 120))}`,
      );
    }

// 3. The schema-drift guard. The worktree borrows the ambient node_modules, so
//    tsc only describes the head when both checkouts carry the same schema.
const { api: driftApi } = loadRegion(WORKFLOW, "SCHEMA-DRIFT-GUARD", ["schemaDriftCondition"]);
const cond = driftApi.schemaDriftCondition();
check(
  "both typecheck sites run the schema-drift guard",
  src.split("${schemaDriftCondition()}").length - 1 === 2,
  `found ${src.split("${schemaDriftCondition()}").length - 1}`,
);
const tmp = mkdtempSync(join(tmpdir(), "tribunal-drift-"));
const mk = (name, schema) => {
  const dir = join(tmp, name);
  mkdirSync(join(dir, "prisma"), { recursive: true });
  if (schema !== null) writeFileSync(join(dir, "prisma", "schema.prisma"), schema);
  return dir;
};
const SAME = "model A {\n  id Int @id\n  taxYear String\n}\n";
const OTHER = "model A {\n  id Int @id\n  taxYear String?\n}\n";
for (const [label, repo, wt, wantDrift] of [
  ["same schema", mk("r1", SAME), mk("w1", SAME), false],
  ["one field differs", mk("r2", SAME), mk("w2", OTHER), true],
  ["no schema on either side", mk("r3", null), mk("w3", null), false],
  ["schema only at the head", mk("r4", null), mk("w4", SAME), true],
])
  for (const sh of shells) {
    const r = spawnSync(sh, ["-c", `if ${cond}; then echo DRIFT; else echo SAME; fi`], {
      env: { ...process.env, REPO: repo, WT: wt },
      encoding: "utf8",
    });
    const got = (r.stdout || "").trim();
    check(
      `schema guard under ${sh}, ${label}: ${wantDrift ? "skips" : "runs"} the check`,
      got === (wantDrift ? "DRIFT" : "SAME"),
      `got ${JSON.stringify(got)} ${r.stderr || ""}`,
    );
  }
rmSync(tmp, { recursive: true, force: true });
check(
  "a skipped check is classified degraded, never clean",
  classifyTypecheckText("CREATED_WORKTREE /tmp/x\nDEGRADED_SCHEMA_DRIFT\n") === "degraded",
);

if (failures.length) {
  console.log(`\n${failures.length} failing`);
  process.exit(1);
}
console.log("\nthe grounded typecheck tail reports the same result under zsh and bash");
