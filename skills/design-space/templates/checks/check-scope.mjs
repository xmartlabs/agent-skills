// Fails when a change touches anything outside the designer-owned paths.
// Usage: node checks/check-scope.mjs <base-sha> <head-sha>
// Run it from a checkout of the base commit: it reads the rules from the base checkout's
// guard/guard.config.json and applies the base checkout's guard policy, and only asks git for the
// list of changed files. It never runs anything from the head commit.
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { repoPathProblem, validateConfig } from "../guard/guard.mjs";

const SHA = /^[0-9a-f]{7,64}$/;

const [base, head] = process.argv.slice(2);
if (!base || !head || !SHA.test(base) || !SHA.test(head)) {
  console.error("usage: node checks/check-scope.mjs <base-sha> <head-sha>");
  process.exit(2);
}

const folderDir = dirname(dirname(fileURLToPath(import.meta.url)));
let config;
try {
  config = JSON.parse(readFileSync(join(folderDir, "guard", "guard.config.json"), "utf8"));
  validateConfig(config);
} catch (error) {
  console.error(`check-scope: cannot read guard/guard.config.json (${error.message})`);
  process.exit(2);
}

let changed;
try {
  const out = execFileSync("git", ["diff", "--name-only", "-z", "--no-renames", `${base}...${head}`], {
    cwd: folderDir,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  changed = out.split("\0").filter(Boolean);
} catch (error) {
  console.error(`check-scope: git diff failed (${String(error.stderr ?? error.message).trim()})`);
  process.exit(2);
}

const outside = changed.map((path) => [path, repoPathProblem(path, config)]).filter(([, why]) => why);
if (outside.length > 0) {
  console.error(`check-scope: ${outside.length} changed file(s) are not designer-owned in ${config.folder}/:`);
  for (const [path, why] of outside) console.error(`  ${path} (${why})`);
  process.exit(1);
}
console.log(`check-scope: ${changed.length} changed file(s), all designer-owned`);
