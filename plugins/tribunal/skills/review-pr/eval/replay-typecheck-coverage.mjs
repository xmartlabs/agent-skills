#!/usr/bin/env node
// replay-typecheck-coverage.mjs — the typecheck's coverage list, narrowed on disk.
//
// The grounded typecheck printed every file of the tsconfig program, up to 5000
// paths, and a mechanic copied that text back so the workflow could ask "did
// tsc read this PR's files?". Only the PR's files are ever asked about, so from
// v81 the bash keeps those and prints the program's size on the marker line.
// This runs the TYPECHECK-COVERAGE bash against a fake `npx` and pins both the
// narrowing and the fail-closed branch.
//
// Usage: node eval/replay-typecheck-coverage.mjs

import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { PLUGIN_ROOT, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const region = loadRegion(WORKFLOW, "TYPECHECK-COVERAGE", ["typecheckCoverageBash"], ["VERIFIED-RELAY"]);
check("the region is pure", region.violations.length === 0, JSON.stringify(region.violations));

const work = mkdtempSync(join(tmpdir(), "tsc-coverage-"));
const bin = join(work, "bin");
spawnSync("mkdir", ["-p", bin]);
const program = ["./src/a.ts", "./src/c.ts", "./src/dir/b.ts", "./src/d.tsx"];
writeFileSync(join(bin, "npx"), `#!/bin/sh\nprintf '%s' '${JSON.stringify({ files: program })}'\n`);
chmodSync(join(bin, "npx"), 0o755);
const run = (bash) =>
  spawnSync("bash", ["-c", bash.replaceAll('"$(tribunal-root)"', `"${PLUGIN_ROOT}"`)], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: work },
  });

try {
  const files = ["src/a.ts", "src/dir/b.ts", "scripts/outside.ts"];
  const bash = region.api.typecheckCoverageBash(files);
  const r = run(bash);
  const lines = r.stdout.trimEnd().split("\n");
  check("the marker carries the program's size", lines[0] === "---COVERED--- 4", r.stdout + r.stderr);
  check("and only this PR's files the program holds follow it", lines.slice(1).join() === "src/a.ts,src/dir/b.ts", lines.join(" | "));
  check("nothing else of the program is printed", !r.stdout.includes("src/c.ts") && !r.stdout.includes("src/d.tsx"));

  const altered = bash.replace("\nsrc/dir/b.ts\n", "\nsrc/dir/x.ts\n");
  const bad = run(altered);
  check("the fault applied to the heredoc only", altered !== bash && altered.includes("src/dir/x.ts"));
  check("an altered PR-file list reports coverage as unknown, not as a narrower truth", bad.stdout.trimEnd() === "---COVERED--- 0", bad.stdout + bad.stderr);

  const long = region.api.typecheckCoverageBash(Array.from({ length: 200 }, (_, i) => `src/dir${i}/file${i}.ts`), 3000);
  check("a PR-file list over the Windows budget skips the probe instead of growing the command", long.length < 200 && run(long).stdout.trimEnd() === "---COVERED--- 0", long.slice(0, 120));
  writeFileSync(join(bin, "npx"), "#!/bin/sh\nexit 1\n");
  const none = run(bash);
  check("a showConfig that prints nothing is a program of size 0", none.stdout.trimEnd() === "---COVERED--- 0", none.stdout + none.stderr);

  const src = readFileSync(WORKFLOW, "utf8");
  check("both typecheck sites use it", (src.match(/\$\{typecheckCoverageBash\(prContext\.files, coverageListLimit\)\}/g) || []).length === 2 && !src.includes("| head -5000"));
  check(
    "the parse reads the size off the marker, and an older probe without it still works",
    /text\.slice\(marker\)\.match\(\/\^---COVERED--- \(\\d\+\)\/\)/.test(src) && /const programSize = sized \? Number\(sized\[1\]\) : coveredList\.length;/.test(src) && src.includes("if (programSize === 0) {"),
  );
} finally {
  rmSync(work, { recursive: true, force: true });
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall typecheck-coverage expectations hold");
