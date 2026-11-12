// Import boundaries, no ESLint needed. Usage: node checks/check-boundaries.mjs
//  - product code (git-tracked source files outside this folder) must not import from this folder;
//  - library/ must not import from flows/ (flows may import library/ and product modules).
// Relative imports are resolved; bare specifiers are packages. Writes nothing.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const folder = dirname(dirname(fileURLToPath(import.meta.url)));
const root = execFileSync("git", ["rev-parse", "--show-toplevel"], { cwd: folder, encoding: "utf8" }).trim();
const SOURCE = /\.(?:[cm]?[jt]s|[jt]sx)$/;

const inside = (parent, path) => path === parent || path.startsWith(parent + sep);
const importsOf = (text) => {
  const found = [];
  const patterns = [
    /\b(?:import|export)\s[^;'"]*?\sfrom\s*["']([^"']+)["']/g,
    /\bimport\s*["']([^"']+)["']/g,
    /\b(?:import|require)\s*\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const pattern of patterns) for (const match of text.matchAll(pattern)) found.push(match[1]);
  return found;
};

function* walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "node_modules" || entry.name === "dist" || entry.name === ".git") continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}

const problems = [];
const report = (file, spec, why) => problems.push(`${relative(root, file)}: imports "${spec}", ${why}`);
const check = (file, rule) => {
  for (const spec of importsOf(readFileSync(file, "utf8"))) {
    if (!spec.startsWith(".")) continue;
    const reason = rule(resolve(dirname(file), spec));
    if (reason) report(file, spec, reason);
  }
};

const tracked = execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" }).split("\0").filter(Boolean);
for (const path of tracked) {
  const file = join(root, path);
  if (!SOURCE.test(path) || inside(folder, file) || !existsSync(file)) continue;
  check(file, (target) => (inside(folder, target) ? "but product code must not depend on the design space" : ""));
}

const library = join(folder, "library");
const flows = join(folder, "flows");
if (existsSync(library)) {
  for (const file of walk(library)) {
    if (!SOURCE.test(file)) continue;
    check(file, (target) => (inside(flows, target) ? "but library/ must not depend on flows/" : ""));
  }
}

if (problems.length) {
  for (const problem of problems) console.error(problem);
  process.exit(1);
}
console.log("check-boundaries: ok");
