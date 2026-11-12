// Finds what a repository already has that looks like a design space or style guide.
// Pure reads, Node built-ins only. Evidence only: bootstrap decides what to do with it.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const SKIPPED = new Set(["node_modules", ".git", "dist", "build", "out", ".next", "coverage"]);
const NAMED = /^(space|design-space|design-system|styleguide)$/i;
const SKILL_NAME = /design.*space/i;
const SCRIPT_NAME = /space|storybook/i;
const ROUTE_ROOTS = ["src/app", "app", "pages", "src"];
const MAX_DEPTH = 6;

const entries = (dir) => {
  try { return readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1)); } catch { return []; }
};

/** @returns {string[]} repository-relative evidence, sorted and unique */
export function detectExistingSpace(root, folder) {
  const found = new Set();
  const skipFolder = folder.replace(/\/+$/, "");

  const visit = (rel, depth) => {
    for (const e of entries(join(root, rel))) {
      if (!e.isDirectory() || SKIPPED.has(e.name)) continue;
      const path = `${rel}/${e.name}`;
      if (path === skipFolder) continue;
      // A route group such as "(internal)" is not part of the name.
      if (NAMED.test(e.name.replace(/^\((.*)\)$/, "$1"))) found.add(path);
      if (depth < MAX_DEPTH) visit(path, depth + 1);
    }
  };
  for (const base of ROUTE_ROOTS) visit(base, 0);

  if (entries(root).some((e) => e.isDirectory() && e.name === ".storybook")) found.add(".storybook");

  const skillDirs = [".claude/skills"];
  for (const e of entries(root)) {
    if (e.isDirectory() && !SKIPPED.has(e.name) && !e.name.startsWith(".")) skillDirs.push(`${e.name}/.claude/skills`);
  }
  for (const dir of skillDirs.filter((d) => !d.startsWith(`${skipFolder}/`))) {
    for (const e of entries(join(root, dir))) {
      if (SKILL_NAME.test(e.name)) found.add(`${dir}/${e.name}`);
    }
  }

  let scripts = {};
  try { scripts = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).scripts ?? {}; } catch { /* no package.json */ }
  for (const name of Object.keys(scripts)) {
    if (SCRIPT_NAME.test(name) && !name.startsWith("design-space:")) found.add(`package.json#scripts.${name}`);
  }
  return [...found].sort();
}
