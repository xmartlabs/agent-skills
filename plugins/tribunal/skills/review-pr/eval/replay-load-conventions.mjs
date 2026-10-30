#!/usr/bin/env node
// replay-load-conventions.mjs — scripts/load-conventions.mjs, the channel that
// hands reviewers the repo's own CLAUDE.md and scoped rules. Which files are
// read, at which ref, which scoped rules a PR's paths select, and every refusal,
// driven with a fake `gh`.
//
// Usage: node eval/replay-load-conventions.mjs

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { changedPaths, globToRegExp, loadConventions, scopeOf } from "../scripts/load-conventions.mjs";
import { loadProfile } from "../scripts/load-profile.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};
const sha = (t) => createHash("sha256").update(t).digest("hex");
const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "..", "scripts", "load-conventions.mjs");

const profile = (over = {}) => ({
  schemaVersion: 1,
  project: "acme",
  repo: "acme/api",
  reviewer: "backend",
  baseBranch: "main",
  deployBranches: [],
  foci: {
    correctness: "correctness (control flow, edge cases)",
    dataContract: "data contracts (API shapes)",
    security: "security (authz, input validation)",
    performance: "performance (queries, hot paths)",
    errorHandling: "error handling (propagation, logging)",
  },
  highRiskSurface: "billing, auth, data deletion",
  crossRepoAnchors: [],
  ...over,
});

const ROOT_MD = "# acme/api\n\nMoney is integer cents everywhere.\n";
const CONTROLLERS = '---\npaths:\n  - "src/controllers/**"\n---\n\nValidate route params with zod.\n';
const BILLING = "---\npaths:\n  - 'src/billing/*.ts'\n  - src/jobs/invoice*\n---\n\nNever retry a charge.\n";
const TESTS = '---\npaths: ["**/*.test.ts"]\n---\n\nTests sit beside the file.\n';
const NOTES = "Plain rule, no frontmatter.\n";
const TREE = {
  "CLAUDE.md": ROOT_MD,
  ".claude/rules/controllers.md": CONTROLLERS,
  ".claude/rules/billing.md": BILLING,
  ".claude/rules/testing.md": TESTS,
  ".claude/rules/notes.md": NOTES,
  ".claude/rules/README.txt": "not a rule",
  "web/CLAUDE.md": "Web app rules.\n",
};
const asked = [];
const gh = (repo, ref, path) => {
  asked.push({ repo, ref, path });
  return path in TREE ? { found: true, text: TREE[path] } : { found: false };
};
const ghDir = (repo, ref, dir) => {
  asked.push({ repo, ref, dir });
  const names = Object.keys(TREE)
    .filter((p) => p.startsWith(`${dir}/`) && !p.slice(dir.length + 1).includes("/"))
    .map((p) => p.slice(dir.length + 1))
    .reverse();
  return names.length ? { found: true, names } : { found: false };
};

console.log("\nload-conventions — the repo's own policy, read at the profile's ref");

console.log("\n--- changed paths come from the patch, both sides of a rename ---");
{
  const patch = [
    "diff --git a/src/old.ts b/src/new.ts",
    "--- a/src/old.ts",
    "+++ b/src/new.ts",
    "diff --git a/gone.ts b/gone.ts",
    "--- a/gone.ts",
    "+++ /dev/null",
    "diff --git a/added.ts b/added.ts",
    "--- /dev/null",
    "+++ b/added.ts",
    '--- "a/with space.ts"',
    "+++ b/src/x.ts\t",
    "+++ b/fake added line inside a hunk is +++ text",
  ].join("\n");
  const got = changedPaths(patch);
  check("a rename contributes both paths", got.includes("src/old.ts") && got.includes("src/new.ts"), JSON.stringify(got));
  check("/dev/null is not a path", !got.some((p) => p.includes("dev/null")), JSON.stringify(got));
  check("added and deleted files count", got.includes("added.ts") && got.includes("gone.ts"));
  check("a quoted path is read without its quotes", got.includes("with space.ts"), JSON.stringify(got));
  check("a trailing tab is not part of the path", got.includes("src/x.ts"), JSON.stringify(got));
}

console.log("\n--- globs follow the rule files' own semantics ---");
for (const [glob, path, want] of [
  ["**/*.test.ts", "a.test.ts", true],
  ["**/*.test.ts", "src/x/a.test.ts", true],
  ["**/*.test.ts", "src/x/a.test.tsx", false],
  ["src/controllers/**", "src/controllers/a/b.ts", true],
  ["src/controllers/**", "src/controllersX/b.ts", false],
  ["src/jobs/services/ledger*", "src/jobs/services/ledgerSync.ts", true],
  ["src/jobs/services/ledger*", "src/jobs/services/nested/ledger.ts", false],
  ["src/jobs/services/*edger*", "src/jobs/services/generalLedgerPush.ts", true],
  ["**/*Partner*", "src/legacy/getPartnerLink.js", true],
  ["src/**/*.{ts,tsx}", "src/a/b.tsx", true],
  ["src/**/*.{ts,tsx}", "src/a/b.js", false],
  ["a.b/c", "aXb/c", false],
]) check(`${glob} ${want ? "matches" : "does not match"} ${path}`, globToRegExp(glob).test(path) === want);

console.log("\n--- scope comes from the paths: frontmatter ---");
check("no frontmatter: always applies", scopeOf(NOTES) === null && scopeOf(ROOT_MD) === null);
check("a frontmatter with no paths: always applies", scopeOf("---\ndescription: x\n---\nbody") === null);
check("a block list, double-quoted", JSON.stringify(scopeOf(CONTROLLERS)) === '["src/controllers/**"]');
check("a block list, single-quoted and bare", JSON.stringify(scopeOf(BILLING)) === '["src/billing/*.ts","src/jobs/invoice*"]');
check("an inline list", JSON.stringify(scopeOf(TESTS)) === '["**/*.test.ts"]');
let threw = "";
try {
  scopeOf("---\npaths:\n  src/a\n---\n");
} catch (err) {
  threw = err.message;
}
check("an unreadable paths: block throws rather than guessing a scope", /cannot read/.test(threw), threw);
threw = "";
try {
  scopeOf("---\npaths:\n---\n");
} catch (err) {
  threw = err.message;
}
check("an empty paths: block throws", /lists no path/.test(threw), threw);

console.log("\n--- selection by the PR's paths ---");
{
  asked.length = 0;
  const r = loadConventions({
    profile: profile({ conventionFiles: ["CLAUDE.md", ".claude/rules/*.md", "CLAUDE.md", "AGENTS.md"] }),
    repo: "acme/api",
    ref: "main",
    changed: ["src/controllers/users.ts", "README.md"],
    gh,
    ghDir,
  });
  const paths = r.files.map((f) => f.path);
  check("reads succeed", r.ok, JSON.stringify(r).slice(0, 200));
  check("every read is at the given ref, in the PR's repo", asked.length > 0 && asked.every((a) => a.ref === "main" && a.repo === "acme/api"), JSON.stringify(asked));
  check("an unscoped file always applies", r.files.some((f) => f.path === "CLAUDE.md" && f.scope === "always"));
  check("a rule with no frontmatter always applies", r.files.some((f) => f.path === ".claude/rules/notes.md" && f.scope === "always"));
  check("a scoped rule applies when a changed path matches, and names it", r.files.some((f) => f.path === ".claude/rules/controllers.md" && f.scope === "paths" && f.matched === "src/controllers/users.ts"));
  check("scoped rules no changed path matches are skipped, and listed", JSON.stringify(r.skipped.map((s) => s.path)) === '[".claude/rules/billing.md",".claude/rules/testing.md"]', JSON.stringify(r.skipped));
  check("a glob takes only files its last segment matches", !paths.includes(".claude/rules/README.txt"));
  check("a glob's files come in sorted order, whatever order the listing gave", JSON.stringify(paths) === '["CLAUDE.md",".claude/rules/controllers.md",".claude/rules/notes.md"]', JSON.stringify(paths));
  check("a file listed twice is read once", asked.filter((a) => a.path === "CLAUDE.md").length === 1);
  check("a named file absent at the ref is recorded as missing", JSON.stringify(r.missing) === '["AGENTS.md"]', JSON.stringify(r.missing));
  check("each file's sha256 is over its exact bytes", r.files.find((f) => f.path === "CLAUDE.md").sha256 === sha(ROOT_MD) && r.skipped[0].sha256 === sha(BILLING));
  check("the manifest carries no file text", r.files.every((f) => !("text" in f)));
  check("the text holds each applied file byte for byte", r.text.includes(ROOT_MD.replace(/\n$/, "")) && r.text.includes(CONTROLLERS.replace(/\n$/, "")) && r.text.includes(NOTES.replace(/\n$/, "")));
  check("the text holds no skipped rule", !r.text.includes("Never retry a charge") && !r.text.includes("Tests sit beside"));
  check("the text says why each file applies", r.text.includes("applies because this PR changes src/controllers/users.ts") && r.text.includes("(always applies)"));
  check("the text names what was left out and what was missing", r.text.includes("billing.md, .claude/rules/testing.md") && r.text.includes("absent at main: AGENTS.md"));
  check("the text names the repo and ref it was read at", r.text.startsWith("# Repository conventions: acme/api at main"));

  const both = loadConventions({ profile: profile({ conventionFiles: [".claude/rules/*.md"] }), repo: "acme/api", ref: "main", changed: ["src/jobs/invoiceRun.ts", "src/billing/a.test.ts"], gh, ghDir });
  const nested = loadConventions({ profile: profile({ conventionFiles: ["web/CLAUDE.md"] }), repo: "acme/api", ref: "main", changed: ["webapp/x.ts"], gh, ghDir });
  check("a CLAUDE.md in a subdirectory is skipped when nothing under it changes", nested.ok && nested.files.length === 0 && nested.skipped[0].path === "web/CLAUDE.md", JSON.stringify(nested));
  const nestedHit = loadConventions({ profile: profile({ conventionFiles: ["web/CLAUDE.md"] }), repo: "acme/api", ref: "main", changed: ["web/a/b.tsx"], gh, ghDir });
  check("and applies when a path under its directory changes", nestedHit.ok && nestedHit.files[0].matched === "web/a/b.tsx", JSON.stringify(nestedHit));
  check("any one changed path is enough, and the first match is named", both.files.some((f) => f.path === ".claude/rules/billing.md" && f.matched === "src/jobs/invoiceRun.ts") && both.files.some((f) => f.path === ".claude/rules/testing.md" && f.matched === "src/billing/a.test.ts"), JSON.stringify(both.files));
}

console.log("\n--- nothing listed, nothing found, and refusals ---");
{
  const none = loadConventions({ profile: profile(), repo: "acme/api", ref: "main", changed: [], gh: () => { throw new Error("no read expected"); }, ghDir });
  check("a profile with no conventionFiles reads nothing and renders nothing", none.ok && none.text === null && none.files.length === 0);
  const noRef = loadConventions({ profile: profile({ conventionFiles: ["CLAUDE.md"] }), repo: "acme/api", ref: "", changed: [], gh, ghDir });
  check("conventionFiles with no ref is refused", !noRef.ok && /--ref/.test(noRef.error), JSON.stringify(noRef));
  const emptyDir = loadConventions({ profile: profile({ conventionFiles: ["docs/rules/*.md"] }), repo: "acme/api", ref: "main", changed: [], gh, ghDir });
  check("a glob over a directory that does not exist is neither an error nor missing", emptyDir.ok && emptyDir.missing.length === 0 && emptyDir.files.length === 0, JSON.stringify(emptyDir));
  const down = loadConventions({ profile: profile({ conventionFiles: ["CLAUDE.md"] }), repo: "acme/api", ref: "main", changed: [], gh: () => ({ error: "HTTP 502" }), ghDir });
  check("a failed read refuses instead of reviewing without the policy", !down.ok && /HTTP 502.*Refusing/.test(down.error), JSON.stringify(down));
  const listDown = loadConventions({ profile: profile({ conventionFiles: [".claude/rules/*.md"] }), repo: "acme/api", ref: "main", changed: [], gh, ghDir: () => ({ error: "HTTP 502" }) });
  check("a failed listing refuses too", !listDown.ok && /could not list/.test(listDown.error), JSON.stringify(listDown));
  const badScope = loadConventions({ profile: profile({ conventionFiles: ["x.md"] }), repo: "acme/api", ref: "main", changed: [], gh: () => ({ found: true, text: "---\npaths:\n  nope\n---\n" }), ghDir });
  check("an unreadable scope refuses, naming the file", !badScope.ok && /acme\/api:x\.md at main/.test(badScope.error), JSON.stringify(badScope));
}

console.log("\n--- the profile validates conventionFiles ---");
for (const [list, ok] of [
  [["CLAUDE.md", ".claude/rules/*.md"], true],
  [["AGENTS.md", "docs/*-rules.md"], true],
  [["../other/CLAUDE.md"], false],
  [["a/*/b.md"], false],
  [["/etc/passwd"], false],
  [[".claude/rules/"], false],
  ["CLAUDE.md", false],
  [[7], false],
]) {
  const r = loadProfile({ repo: "acme/api", ref: "main", gh: () => ({ found: true, text: JSON.stringify(profile({ conventionFiles: list })) }) });
  check(`conventionFiles ${JSON.stringify(list)} is ${ok ? "accepted" : "refused"}`, ok ? r.ok : !r.ok && /conventionFiles must be/.test(r.error), r.error);
}

console.log("\n--- the profile validates conventionSources ---");
for (const [list, ok, why] of [
  [undefined, true],
  [[], true],
  [[{ repo: "acme/handbook", ref: "submodule:docs" }], true],
  [[{ repo: "acme/handbook", ref: "v2", select: "all", include: ["convention/*.md"], exclude: ["**/ui-*.md"], consumer: "server", budgetBytes: 30000, later: 1 }], true],
  [[{ root: "docs/", select: "manifest", manifest: "docs/m.json" }], true],
  [[{ repo: "acme/handbook", ref: "main", select: "manifest", manifest: "m.json" }], true],
  ["acme/handbook", false, /conventionSources must be an array/],
  [[{ ref: "main" }], false, /needs a repo/],
  [[{ root: "docs" }], false, /\.root must be/],
  [[{ select: "manifest" }], false, /\.manifest must be a path/],
  [[{ root: "docs/", select: "manifest", manifest: "m.json" }], false, /must lie under root docs\//],
  [[null], false, /conventionSources\[0\] must be an object/],
  [[{ repo: "acme", ref: "main" }], false, /conventionSources\[0\]\.repo/],
  [[{ repo: "acme/handbook" }], false, /\.ref must be a git ref/],
  [[{ repo: "acme/handbook", ref: "submodule:../x" }], false, /\.ref must be a git ref/],
  [[{ repo: "acme/handbook", ref: "submodule:docs/" }], false, /\.ref must be a git ref/],
  [[{ repo: "acme/handbook", ref: "main", select: "some" }], false, /\.select must be "okf-code", "all" or "manifest"/],
  [[{ repo: "acme/handbook", ref: "main", include: ["../x.md"] }], false, /\.include must be/],
  [[{ repo: "acme/handbook", ref: "main", exclude: "x.md" }], false, /\.exclude must be/],
  [[{ repo: "acme/handbook", ref: "main", consumer: "a/b" }], false, /\.consumer must be/],
  [[{ repo: "acme/handbook", ref: "main", budgetBytes: 0 }], false, /\.budgetBytes must be/],
  [[{ repo: "acme/handbook", ref: "main", budgetBytes: 2.5 }], false, /\.budgetBytes must be/],
]) {
  const r = loadProfile({ repo: "acme/api", ref: "main", gh: () => ({ found: true, text: JSON.stringify(profile(list === undefined ? {} : { conventionSources: list })) }) });
  check(`conventionSources ${JSON.stringify(list)} is ${ok ? "accepted" : "refused"}`, ok ? r.ok : !r.ok && why.test(r.error), r.error);
}

console.log("\n--- the command line, end to end through a fake gh ---");
{
  const dir = mkdtempSync(join(tmpdir(), "load-conventions-"));
  const bin = join(dir, "bin");
  spawnSync("mkdir", ["-p", bin]);
  writeFileSync(join(dir, "tree.json"), JSON.stringify(TREE));
  writeFileSync(
    join(bin, "gh"),
    `#!/usr/bin/env node
const tree = require(${JSON.stringify(join(dir, "tree.json"))});
const fs = require("fs");
fs.appendFileSync(${JSON.stringify(join(dir, "calls.log"))}, process.argv.slice(2).join(" ") + "\\n");
const m = process.argv[3].match(/^repos\\/acme\\/api\\/contents\\/(.*)\\?ref=(.*)$/);
if (!m || m[2] !== "rel%2F1") { process.stderr.write("HTTP 404: Not Found"); process.exit(1); }
const p = m[1];
if (process.argv.includes("Accept: application/vnd.github.raw")) {
  if (p in tree) { process.stdout.write(tree[p]); process.exit(0); }
  process.stderr.write("HTTP 404: Not Found"); process.exit(1);
}
const names = Object.keys(tree).filter((k) => k.startsWith(p + "/") && !k.slice(p.length + 1).includes("/"));
if (!names.length) { process.stderr.write("HTTP 404: Not Found"); process.exit(1); }
process.stdout.write(JSON.stringify(names.map((k) => ({ name: k.slice(p.length + 1), type: "file" }))));
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  const profFile = join(dir, "profile.json");
  writeFileSync(profFile, JSON.stringify(profile({ conventionFiles: ["CLAUDE.md", ".claude/rules/*.md"] })));
  const patch = join(dir, "pr.patch");
  writeFileSync(patch, "diff --git a/src/controllers/a.ts b/src/controllers/a.ts\n--- a/src/controllers/a.ts\n+++ b/src/controllers/a.ts\n");
  const out = join(dir, "conventions.md");
  const run = (args) => {
    const p = spawnSync("node", [SCRIPT, ...args], { encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}` } });
    let json = null;
    try {
      json = JSON.parse(p.stdout);
    } catch {}
    return { code: p.status, json, stdout: p.stdout, stderr: p.stderr };
  };
  const r = run(["--profile", profFile, "--ref", "rel/1", "--diff", patch, "--out", out]);
  check("it exits 0 and prints one JSON line", r.code === 0 && r.json && r.json.ok, r.stdout + r.stderr);
  const written = (() => {
    try {
      return readFileSync(out, "utf8");
    } catch {
      return "";
    }
  })();
  check("the sha256 it prints is over the file it wrote", r.json && r.json.sha256 === sha(written) && written.length > 0);
  check("it reads at the ref it was given, URL-encoded", r.json && r.json.ref === "rel/1" && readFileSync(join(dir, "calls.log"), "utf8").split("\n").filter(Boolean).every((l) => l.includes("ref=rel%2F1")));
  check("the manifest selects by the patch's paths", r.json && JSON.stringify(r.json.files.map((f) => f.path)) === '["CLAUDE.md",".claude/rules/controllers.md",".claude/rules/notes.md"]', JSON.stringify(r.json && r.json.files));
  const noDiff = run(["--profile", profFile, "--ref", "rel/1", "--out", out]);
  check("no --diff is refused: scoped rules need the PR's paths", noDiff.code === 1 && /--diff/.test(noDiff.json && noDiff.json.error));
  const wrongRepo = run(["--profile", profFile, "--repo", "acme/web", "--ref", "rel/1", "--diff", patch, "--out", out]);
  check("a --repo the profile does not describe is refused", wrongRepo.code === 1 && /describes acme\/api/.test(wrongRepo.json && wrongRepo.json.error));
  const plain = join(dir, "plain.json");
  writeFileSync(plain, JSON.stringify(profile()));
  const empty = run(["--profile", plain, "--diff", patch, "--out", join(dir, "never.md")]);
  check("a profile with no conventionFiles succeeds and writes nothing", empty.code === 0 && empty.json.out === null && spawnSync("test", ["-e", join(dir, "never.md")]).status !== 0, empty.stdout);
}

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all load-conventions expectations hold"}`);
process.exit(failures.length ? 1 : 0);
