// Hands a prototype to engineers: marks it ready, commits, pushes the current proto/* branch
// and opens a pull request.
// Usage: npm run design-space:ready -- <flow>
// Trusted, engineer-owned code: it runs git and gh itself. It only ever pushes proto/*.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBase } from "./base.mjs";
import { repoPathProblem, validateConfig } from "../guard/guard.mjs";

const NAME_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const BRANCH_RE = /^proto\/[a-z0-9]+(-[a-z0-9]+)*$/;
const folder = dirname(dirname(fileURLToPath(import.meta.url)));
const fail = (message) => { console.error(`design-space ready: ${message}`); process.exit(1); };
const run = (cmd, args, cwd = folder) => execFileSync(cmd, args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
const git = (...args) => run("git", args).trim();
const gitRoot = (...args) => run("git", args, root).trim();
const detail = (error) => String(error.stderr || error.message).trim();

const flow = process.argv[2];
if (process.argv.length !== 3 || !flow || !NAME_RE.test(flow)) fail("give the name of the prototype to hand off");

const root = git("rev-parse", "--show-toplevel");
let config;
try {
  config = JSON.parse(readFileSync(join(folder, "guard", "guard.config.json"), "utf8"));
  validateConfig(config);
} catch (error) {
  fail(`guard/guard.config.json is not usable (${error.message}); ask an engineer`);
}

const branch = git("branch", "--show-current");
if (!BRANCH_RE.test(branch)) fail(`you are on "${branch || "no branch"}"; hand-off only works on a proto/ branch. Start one with the new command.`);
const metaFile = join(folder, "flows", flow, "meta.ts");
if (!existsSync(metaFile)) fail(`there is no prototype named "${flow}"`);

// Everything that changed must be designer-owned, staged or not, untracked included.
const status = run("git", ["status", "--porcelain=v1", "-z", "--untracked-files=all"], root).split("\0").filter(Boolean);
const dirty = [];
for (let i = 0; i < status.length; i += 1) {
  const code = status[i].slice(0, 2);
  dirty.push(status[i].slice(3));
  if (code.includes("R") || code.includes("C")) { i += 1; dirty.push(status[i]); }
}
const outside = dirty.map((path) => [path, repoPathProblem(path, config)]).filter(([, why]) => why);
if (outside.length > 0) {
  const list = outside.map(([path, why]) => `${path} (${why})`).join("\n  ");
  fail(`these changed files cannot be handed off by a designer; remove them or ask an engineer:\n  ${list}`);
}

const original = readFileSync(metaFile, "utf8");
let title;
try {
  const meta = JSON.parse(/^export\s+default\s+([\s\S]*?);?$/.exec(original.trim())[1]);
  title = typeof meta.title === "string" && meta.title ? meta.title : flow;
  meta.status = "ready-for-dev";
  writeFileSync(metaFile, `export default ${JSON.stringify(meta, null, 2)}\n`);
} catch (error) {
  fail(`flows/${flow}/meta.ts could not be read: ${error.message}`);
}
try {
  run("node", [join(folder, "checks", "check-meta.mjs"), "--for-ready", flow]);
} catch (error) {
  writeFileSync(metaFile, original);
  fail(`the prototype is not ready yet:\n${String(error.stderr || error.stdout).trim()}`);
}

// Base branch: the profile first, then the remote's default branch.
const base = resolveBase(root);
if (!base) {
  writeFileSync(metaFile, original);
  fail("could not find the branch to open the pull request against; ask an engineer to set baseBranch in .design-space.json");
}

try {
  const owned = config.designerOwned
    .map((pattern) => `${config.folder}/${pattern.replace(/\/\*\*$/, "")}`)
    .filter((path) => existsSync(join(root, path)));
  if (owned.length > 0) gitRoot("add", "-A", "--", ...owned);
  if (gitRoot("diff", "--cached", "--name-only").length > 0) {
    gitRoot("commit", "-m", `feat(design-space): hand off ${flow}`);
  }
  gitRoot("push", "-u", "origin", branch);
  const existing = run("gh", ["pr", "list", "--head", branch, "--state", "open", "--json", "url", "--jq", ".[0].url"], root).trim();
  if (existing) {
    console.log(`updated the existing pull request: ${existing}`);
    process.exit(0);
  }
  const out = run("gh", [
    "pr", "create", "--draft=false", "--base", base, "--head", branch,
    "--title", `Design space: ${title}`,
    "--body", `Prototype \`${flow}\` is ready for engineering.`,
  ], root);
  console.log(out.trim() || `Pushed ${branch} and opened a pull request.`);
} catch (error) {
  fail(detail(error));
}
