#!/usr/bin/env node
// replay-review-repo.mjs — the Tier-1 rigs find the checkout under review by
// its origin, never by one machine's directory layout.
//
// reviewRepo() used to return a hardcoded path under the operator's home. It
// now takes the repo (given, else the operator's defaultRepo, else the cwd's
// origin) and returns the first of the cwd, ./<name> and ../<name> whose origin
// is exactly that repo, or null.
//
// Usage: node eval/replay-review-repo.mjs

import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { reviewRepo } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const root = realpathSync(mkdtempSync(join(tmpdir(), "review-repo-")));
const work = join(root, "work");
const child = join(work, "api");
const sibling = join(root, "web");
const decoy = join(work, "web");
for (const d of [work, child, sibling, decoy]) mkdirSync(d, { recursive: true });
const origins = new Map([
  [child, "acme/api"],
  [sibling, "acme/web"],
  [decoy, "someone-else/web"],
]);
const originOf = (dir) => origins.get(dir) ?? null;

const config = join(root, "review-harness.json");
const savedConfig = process.env.REVIEW_HARNESS_CONFIG;
const savedDir = process.env.TRIBUNAL_REPO_DIR;
delete process.env.TRIBUNAL_REPO_DIR;
process.env.REVIEW_HARNESS_CONFIG = config;

console.log("\nreviewRepo — the checkout is found by origin");

writeFileSync(config, "{}");
check("a named repo resolves to ./<name> when its origin matches", reviewRepo("acme/api", { cwd: work, originOf }) === child);
check("a named repo resolves to ../<name> when its origin matches", reviewRepo("acme/web", { cwd: work, originOf }) === sibling);
check(
  "a ./<name> whose origin is another repo is passed over",
  reviewRepo("acme/web", { cwd: work, originOf }) !== decoy,
);
check("the cwd itself counts when its origin is the repo", reviewRepo("acme/api", { cwd: child, originOf }) === child);
check("no matching checkout gives null, not a guess", reviewRepo("acme/billing", { cwd: work, originOf }) === null);
check("no repo named and nothing to infer gives null", reviewRepo("", { cwd: work, originOf }) === null);

writeFileSync(config, JSON.stringify({ defaultRepo: "acme/web" }));
check("with no repo named, the operator's defaultRepo is used", reviewRepo("", { cwd: work, originOf }) === sibling);

writeFileSync(config, "{}");
check("with no default, the cwd's origin is the repo", reviewRepo("", { cwd: child, originOf }) === child);

process.env.TRIBUNAL_REPO_DIR = "/somewhere/explicit";
check("TRIBUNAL_REPO_DIR wins over everything", reviewRepo("acme/api", { cwd: work, originOf }) === "/somewhere/explicit");

if (savedDir === undefined) delete process.env.TRIBUNAL_REPO_DIR;
else process.env.TRIBUNAL_REPO_DIR = savedDir;
if (savedConfig === undefined) delete process.env.REVIEW_HARNESS_CONFIG;
else process.env.REVIEW_HARNESS_CONFIG = savedConfig;

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all review-repo expectations hold"}`);
process.exit(failures.length ? 1 : 0);
