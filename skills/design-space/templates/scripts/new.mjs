// Starts a prototype: creates and switches to proto/<name>, and copies flow-template to flows/<name>.
// Usage: npm run design-space:new -- <name>   (names are lowercase words joined by dashes)
// Trusted, engineer-owned code: it runs git itself.
import { cpSync, existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBase } from "./base.mjs";

const NAME_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const folder = dirname(dirname(fileURLToPath(import.meta.url)));
const fail = (message) => { console.error(`design-space new: ${message}`); process.exit(1); };
const git = (...args) => execFileSync("git", args, { cwd: folder, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();

const name = process.argv[2];
if (process.argv.length !== 3 || !name || !NAME_RE.test(name)) {
  fail("give one name made of lowercase letters and numbers joined by dashes, for example: checkout-redesign");
}
const template = join(folder, "flow-template");
const target = join(folder, "flows", name);
if (!existsSync(template)) fail("flow-template is missing, so there is nothing to copy");
if (existsSync(target)) fail(`a prototype named "${name}" already exists`);

const branch = `proto/${name}`;
try {
  git("rev-parse", "--verify", "-q", `refs/heads/${branch}`);
  fail(`the branch ${branch} already exists`);
} catch (error) {
  if (error.status === undefined) throw error;
}

// Every prototype starts from the latest base branch, never from the previous prototype's work.
const root = git("rev-parse", "--show-toplevel");
if (execFileSync("git", ["status", "--porcelain", "--untracked-files=all"], { cwd: root, encoding: "utf8" }).trim()) {
  fail("there are unsaved changes in this project. Hand them off with the ready step or ask an engineer before starting a new prototype.");
}
const previous = git("branch", "--show-current") || git("rev-parse", "HEAD");
let startPoint;
const base = resolveBase(root);
if (base) {
  try {
    git("fetch", "origin", base);
    startPoint = `origin/${base}`;
    git("rev-parse", "--verify", "-q", `${startPoint}^{commit}`);
  } catch {
    startPoint = undefined;
  }
}
if (!startPoint) {
  console.warn("design-space new: warning: could not get the latest base branch from the remote, so this prototype starts from the branch you are on. Tell an engineer if that is not what you want.");
}
try {
  git("switch", "-c", branch, ...(startPoint ? ["--no-track", startPoint] : []));
} catch (error) {
  fail(`could not create ${branch}: ${String(error.stderr ?? error.message).trim()}`);
}
const abandon = (message) => {
  try {
    rmSync(target, { recursive: true, force: true });
    git("switch", "--force", previous);
    git("branch", "-D", branch);
  } catch { /* leave the branch if it cannot be removed */ }
  fail(message);
};
if (!existsSync(template)) abandon("flow-template is missing on the base branch, so there is nothing to copy");

cpSync(template, target, { recursive: true });
const metaFile = join(target, "meta.ts");
try {
  const match = /^export\s+default\s+([\s\S]*?);?$/.exec(readFileSync(metaFile, "utf8").trim());
  const meta = JSON.parse(match[1]);
  meta.title = `Prototype: ${name}`;
  meta.status = "in-progress";
  writeFileSync(metaFile, `export default ${JSON.stringify(meta, null, 2)}\n`);
} catch (error) {
  abandon(`could not prepare the new prototype: ${error.message}`);
}
console.log(`Started ${branch} with a new prototype in flows/${name}/.`);
