#!/usr/bin/env node
// replay-load-profile.mjs — scripts/load-profile.mjs, the one place a review
// learns its project facts. Discovery order, refusal on every ambiguity, and the
// sha256 the metrics row records, driven with a fake `gh` and temp dirs.
//
// Usage: node eval/replay-load-profile.mjs

import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { loadProfile, REPO_PROFILE_PATH } from "../scripts/load-profile.mjs";
import { clientProfileByReviewer, clientProfileFiles } from "./client-profiles.mjs";
import { SKILL_DIR } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const BE = clientProfileByReviewer("backend").profile.repo;
const FE = clientProfileByReviewer("frontend").profile.repo;
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
const sha = (t) => createHash("sha256").update(t).digest("hex");
const frozenText = (repo) =>
  readFileSync(clientProfileFiles().find((f) => JSON.parse(readFileSync(f, "utf8")).repo === repo), "utf8");
const noGh = () => {
  throw new Error("gh must not be called without --ref");
};
const ghReturning = (result) => () => result;
const foreign = (over = {}) =>
  JSON.stringify({
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

console.log("\nload-profile — discovery, validation, sha256");

console.log("\n--- each repo's own profile, read at the base ref ---");
for (const repo of [BE, FE]) {
  const r = loadProfile({ repo, ref: "development", gh: ghReturning({ found: true, text: frozenText(repo) }) });
  check(`${repo} resolves to the profile committed in it`, r.ok && r.profile.repo === repo && r.source === `repo:${repo}@development:${REPO_PROFILE_PATH}`, JSON.stringify(r).slice(0, 160));
  check(`${repo}: sha256 is over the file's exact bytes`, r.ok && r.sha256 === sha(frozenText(repo)));
}
check("with no base ref and no --profile there is nothing to read, and it says so", /pass --pr <n> or --ref <branch>/.test(loadProfile({ repo: BE, gh: noGh }).error || ""));

console.log("\n--- the repo's own profile wins, read at the base ref ---");
{
  let asked = null;
  const gh = (repo, ref, path) => {
    asked = { repo, ref, path };
    return { found: true, text: foreign() };
  };
  const r = loadProfile({ repo: "acme/api", ref: "main", gh });
  check("a committed profile is used when present", r.ok && r.profile.project === "acme", JSON.stringify(r).slice(0, 160));
  check("it is read from the PR's repo, at the base ref, at the fixed path", asked && asked.repo === "acme/api" && asked.ref === "main" && asked.path === REPO_PROFILE_PATH, JSON.stringify(asked));
  check("the source names the repo and ref", r.ok && r.source === `repo:acme/api@main:${REPO_PROFILE_PATH}`, r.source);
  const be = loadProfile({ repo: BE, ref: "development", gh: ghReturning({ found: false }), defaultBranchOf: () => ({ ref: "development" }) });
  check("a 404 there is a refusal: the plugin ships no profile to fall back on", !be.ok && be.error.includes(`no profile for ${BE} at development`), JSON.stringify(be).slice(0, 160));
  const down = loadProfile({ repo: BE, ref: "development", gh: ghReturning({ error: "HTTP 502" }) });
  check("any other gh failure refuses instead of guessing", !down.ok && /Refusing to guess/.test(down.error), JSON.stringify(down));
  const onlyOnDev = (repo, at) => (at === "development" ? { found: true, text: frozenText(repo) } : { found: false });
  const old = loadProfile({ repo: BE, ref: "release-2026-07-31", gh: (repo, at) => onlyOnDev(BE, at), defaultBranchOf: () => ({ ref: "development" }) });
  check("a base branch that predates the profile reads it from the default branch", old.ok && old.source === `repo:${BE}@development:${REPO_PROFILE_PATH}` && old.sha256 === sha(frozenText(BE)), JSON.stringify(old).slice(0, 160));
  check("profileRef names the ref the profile was found at, not the base it was asked for", old.ok && old.profileRef === "development", old.profileRef);
  check("a profile found at the base ref reports that ref", r.ok && r.profileRef === "main", r.profileRef);
  const noDefault = loadProfile({ repo: BE, ref: "release-2026-07-31", gh: ghReturning({ found: false }), defaultBranchOf: () => ({ error: "HTTP 404" }) });
  check("a default branch that cannot be read refuses instead of guessing", !noDefault.ok && /default branch could not be read.*Refusing to guess/.test(noDefault.error), JSON.stringify(noDefault));
  const flaky = loadProfile({ repo: BE, ref: "release-2026-07-31", gh: (repo, at) => (at === "development" ? { error: "HTTP 502" } : { found: false }), defaultBranchOf: () => ({ ref: "development" }) });
  check("a gh failure on the default-branch read refuses too", !flaky.ok && /at development \(HTTP 502\)\. Refusing to guess/.test(flaky.error), JSON.stringify(flaky));
  const neither = loadProfile({ repo: BE, ref: "release-2026-07-31", gh: ghReturning({ found: false }), defaultBranchOf: () => ({ ref: "development" }) });
  check("missing on both branches names both", !neither.ok && neither.error.includes(`no profile for ${BE} at release-2026-07-31 or on the default branch development`), JSON.stringify(neither));
}

console.log("\n--- refusals ---");
{
  const r = loadProfile({ repo: "acme/web", ref: "main", gh: ghReturning({ found: false }), defaultBranchOf: () => ({ ref: "main" }) });
  check("a repo with no profile is refused", !r.ok && /no profile for acme\/web at main/.test(r.error), JSON.stringify(r));
  const dir = mkdtempSync(join(tmpdir(), "load-profile-"));
  const file = join(dir, "p.json");
  writeFileSync(file, frozenText(BE));
  const wrong = loadProfile({ repo: FE, profilePath: file, gh: noGh });
  check("a --profile file describing another repo is refused", !wrong.ok && new RegExp(`describes ${esc(BE)}, but this run reviews`).test(wrong.error), JSON.stringify(wrong));
  writeFileSync(file, "{not json");
  const bad = loadProfile({ repo: BE, profilePath: file, gh: noGh });
  check("an unparseable profile is refused", !bad.ok && /not valid JSON/.test(bad.error));
  const committedBad = loadProfile({ repo: "acme/api", ref: "main", gh: ghReturning({ found: true, text: foreign({ reviewer: "x" }) }) });
  check("an invalid committed profile is refused, not skipped", !committedBad.ok && /reviewer must be one of/.test(committedBad.error), JSON.stringify(committedBad));
}

console.log("\n--- the CLI prints one line and exits non-zero on refusal ---");
{
  const script = join(SKILL_DIR, "scripts", "load-profile.mjs");
  const beProfile = clientProfileFiles().find((f) => f.endsWith("_BE.json"));
  const beRules = join(dirname(beProfile), "..", "rules", basename(beProfile).replace(/\.json$/, ".md"));
  const okRun = spawnSync(process.execPath, [script, "--repo", BE, "--profile", beProfile, "--rules", beRules], { encoding: "utf8" });
  const parsed = (() => {
    try {
      return JSON.parse(okRun.stdout.trim());
    } catch {
      return null;
    }
  })();
  check("success exits 0 with one parseable line", okRun.status === 0 && parsed && parsed.ok === true, okRun.stdout.slice(0, 120));
  const noRun = spawnSync(process.execPath, [script, "--repo", "acme/none"], { encoding: "utf8" });
  check("a refusal exits 1", noRun.status === 1 && /"ok":false/.test(noRun.stdout), noRun.stdout.slice(0, 120));
}

console.log("\n--- which repo, when the caller names none ---");
{
  const { resolveRepo, readOperatorConfig, repoFromRemoteUrl } = await import("../../../lib/operator.mjs");
  const dir = mkdtempSync(join(tmpdir(), "operator-"));
  const cfg = join(dir, "review-harness.json");
  const none = readOperatorConfig(join(dir, "absent.json"));
  check("no operator file is not an error", !none.error && none.present === false);
  writeFileSync(cfg, JSON.stringify({ defaultRepo: BE }));
  const op = readOperatorConfig(cfg);
  check("an explicit --repo wins over everything", resolveRepo({ flag: FE, operator: op, cwdRepoOf: () => "acme/x" }).repo === FE);
  const viaDefault = resolveRepo({ operator: op, cwdRepoOf: () => "acme/x" });
  check("then the operator's defaultRepo, as the old hardcoded default did", viaDefault.repo === BE && viaDefault.source.startsWith("operator:"), JSON.stringify(viaDefault));
  const viaCwd = resolveRepo({ operator: none, cwdRepoOf: () => "acme/api" });
  check("then the current directory's origin", viaCwd.repo === "acme/api" && viaCwd.source === "cwd-origin");
  check("with none of the three, a refusal that names all three", /pass --repo.*defaultRepo.*origin/.test(resolveRepo({ operator: none, cwdRepoOf: () => null }).error || ""));
  writeFileSync(cfg, "{not json");
  check("an unreadable operator file refuses instead of being ignored", /not valid JSON/.test(resolveRepo({ operator: readOperatorConfig(cfg), cwdRepoOf: () => "acme/api" }).error || ""));
  writeFileSync(cfg, JSON.stringify({ defaultRepo: "not a repo" }));
  check("a malformed defaultRepo refuses", /defaultRepo must be owner\/name/.test(readOperatorConfig(cfg).error || ""));
  for (const [url, want] of [
    ["git@github.com:acme/api.git", "acme/api"],
    ["https://github.com/acme/api", "acme/api"],
    ["https://github.com/acme/api.git\n", "acme/api"],
    ["https://gitlab.com/acme/api.git", null],
    ["", null],
  ])
    check(`origin ${JSON.stringify(url.trim())} parses to ${want}`, repoFromRemoteUrl(url) === want, String(repoFromRemoteUrl(url)));
  const { resolveAndLoad } = await import("../scripts/load-profile.mjs");
  const full = resolveAndLoad({ pr: 7, operator: op.error ? none : op, cwdRepoOf: () => null, baseRefOf: () => ({ ref: "development" }), gh: () => ({ found: false }) });
  writeFileSync(cfg, JSON.stringify({ defaultRepo: BE }));
  const full2 = resolveAndLoad({ pr: 7, operator: readOperatorConfig(cfg), cwdRepoOf: () => null, baseRefOf: () => ({ ref: "development" }), gh: () => ({ found: true, text: frozenText(BE) }) });
  check("--pr alone: repo from the operator, base from the PR, profile from the repo at that base", full2.ok && full2.repo === BE && full2.baseRef === "development" && full2.source === `repo:${BE}@development:${REPO_PROFILE_PATH}`, JSON.stringify(full2).slice(0, 200));
  const noBase = resolveAndLoad({ pr: 7, operator: readOperatorConfig(cfg), cwdRepoOf: () => null, baseRefOf: () => ({ error: "HTTP 404" }) });
  check("a PR whose base cannot be read refuses", !noBase.ok && /could not read the base branch of PR #7/.test(noBase.error), JSON.stringify(noBase));
  void full;
}

console.log("\n--- one file, two plugins: tribunal reads only its fields ---");
{
  const rp = [{ focus: "security", pattern: "auth" }];
  const shared = loadProfile({
    repo: "acme/api",
    profilePath: (() => {
      const f = join(mkdtempSync(join(tmpdir(), "pr-prof-")), "p.json");
      writeFileSync(f, foreign({ riskPaths: rp, otherTool: { anything: [1, 2, 3] }, releaseReview: { globs: ["**"] }, someFutureField: true }));
      return f;
    })(),
  });
  check("other tools' sections and an unknown key do not make tribunal refuse", shared.ok, JSON.stringify(shared).slice(0, 200));
  const odd = loadProfile({
    repo: "acme/api",
    profilePath: (() => {
      const f = join(mkdtempSync(join(tmpdir(), "pr-prof-")), "p.json");
      writeFileSync(f, foreign({ riskPaths: [...rp, { focus: "accessibility", pattern: "aria" }] }));
      return f;
    })(),
  });
  check(
    "a risk path forcing a focus tribunal does not use is a warning, not a refusal",
    odd.ok && odd.warnings.some((w) => w.includes("accessibility")),
    JSON.stringify(odd).slice(0, 240),
  );
  const none = loadProfile({
    repo: "acme/api",
    profilePath: (() => {
      const f = join(mkdtempSync(join(tmpdir(), "pr-prof-")), "p.json");
      writeFileSync(f, foreign());
      return f;
    })(),
  });
  check("a profile from before riskPaths existed still loads, and says so", none.ok && none.warnings.some((w) => w.startsWith("no riskPaths")), JSON.stringify(none.warnings));
  const broken = loadProfile({
    repo: "acme/api",
    profilePath: (() => {
      const f = join(mkdtempSync(join(tmpdir(), "pr-prof-")), "p.json");
      writeFileSync(f, foreign({ riskPaths: [{ focus: "security", pattern: "(" }] }));
      return f;
    })(),
  });
  check("a risk path that does not compile is refused", !broken.ok && /does not compile/.test(broken.error), JSON.stringify(broken).slice(0, 200));
}
{
  const dir = mkdtempSync(join(tmpdir(), "pr-save-"));
  const src = join(dir, "in.json");
  const saved = join(dir, "saved.json");
  const text = foreign({ riskPaths: [{ focus: "security", pattern: "auth" }] });
  writeFileSync(src, text);
  const out = spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "load-profile.mjs"), "--repo", "acme/api", "--profile", src, "--save", saved], { encoding: "utf8" });
  const printed = JSON.parse(out.stdout || "{}");
  check(
    "--save writes the bytes the printed sha256 describes, and keeps them out of stdout",
    out.status === 0 && readFileSync(saved, "utf8") === text && printed.sha256 === sha(text) && !("text" in printed),
    out.stdout + out.stderr,
  );
}

console.log("\n--- the repo's review rules, read where the profile was read ---");
{
  const rulesText = "<!-- slot: tribunal/intro -->\nYou review acme.\n<!-- /slot -->\n";
  const reads = [];
  const gh = (repo, at, path) => {
    reads.push(`${at}:${path}`);
    if (path === REPO_PROFILE_PATH) return at === "main" ? { found: true, text: foreign({ projectRules: ".claude/review-rules.md" }) } : { found: false };
    if (path === ".claude/review-rules.md") return at === "main" ? { found: true, text: rulesText } : { found: false };
    return { found: false };
  };
  const r = loadProfile({ repo: "acme/api", ref: "release-1", gh, defaultBranchOf: () => ({ ref: "main" }) });
  check(
    "a profile found through the default-branch fallback reads its rules on that branch too",
    r.ok && r.projectRules?.source === "repo:acme/api@main:.claude/review-rules.md" && r.projectRules.sha256 === sha(rulesText) && r.rulesText === rulesText && !reads.includes("release-1:.claude/review-rules.md"),
    JSON.stringify({ ok: r.ok, rules: r.projectRules, error: r.error, reads }),
  );
  const missing = loadProfile({
    repo: "acme/api",
    ref: "main",
    gh: (repo, at, path) => (path === REPO_PROFILE_PATH ? { found: true, text: foreign({ projectRules: ".claude/review-rules.md" }) } : { found: false }),
    defaultBranchOf: () => ({ ref: "main" }),
  });
  check("a profile naming a rules file that is not there is refused", !missing.ok && /does not exist at main/.test(missing.error), JSON.stringify(missing).slice(0, 200));
  const broken = loadProfile({
    repo: "acme/api",
    ref: "main",
    gh: (repo, at, path) => (path === REPO_PROFILE_PATH ? { found: true, text: foreign({ projectRules: ".claude/review-rules.md" }) } : { error: "HTTP 502" }),
    defaultBranchOf: () => ({ ref: "main" }),
  });
  check("a gh error reading the rules is a refusal, not a review without them", !broken.ok && /HTTP 502/.test(broken.error), JSON.stringify(broken).slice(0, 200));
  const none = loadProfile({ repo: "acme/api", ref: "main", gh: () => ({ found: true, text: foreign() }), defaultBranchOf: () => ({ ref: "main" }) });
  check("no projectRules: projectRules is null and the loader warns", none.ok && none.projectRules === null && none.warnings.some((w) => w.startsWith("no projectRules")), JSON.stringify(none.warnings));
  const dir = mkdtempSync(join(tmpdir(), "pr-rules-"));
  const prof = join(dir, "p.json");
  writeFileSync(prof, foreign({ projectRules: ".claude/review-rules.md" }));
  const fromFile = loadProfile({ repo: "acme/api", profilePath: prof });
  check("a profile read from a file with projectRules needs --rules", !fromFile.ok && /pass --rules/.test(fromFile.error), JSON.stringify(fromFile).slice(0, 200));
  const rules = join(dir, "r.md");
  writeFileSync(rules, rulesText);
  const withRules = loadProfile({ repo: "acme/api", profilePath: prof, rulesPath: rules });
  check("--rules supplies them", withRules.ok && withRules.projectRules.source === `file:${rules}` && withRules.projectRules.sha256 === sha(rulesText), JSON.stringify(withRules.projectRules));
  for (const bad of ["../x.md", "/etc/passwd", "a/../b.md"]) {
    const r2 = loadProfile({ repo: "acme/api", ref: "main", gh: () => ({ found: true, text: foreign({ projectRules: bad }) }), defaultBranchOf: () => ({ ref: "main" }) });
    check(`projectRules ${JSON.stringify(bad)} is refused`, !r2.ok && /projectRules must be a path inside the repo/.test(r2.error), r2.error);
  }
  const saved = join(dir, "saved.md");
  const cli = spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "load-profile.mjs"), "--repo", "acme/api", "--profile", prof, "--rules", rules, "--save-rules", saved], { encoding: "utf8" });
  const printed = JSON.parse(cli.stdout || "{}");
  check(
    "--save-rules writes the bytes projectRules.sha256 describes, and keeps them out of stdout",
    cli.status === 0 && readFileSync(saved, "utf8") === rulesText && printed.projectRules.sha256 === sha(rulesText) && !("rulesText" in printed),
    cli.stdout + cli.stderr,
  );
  const plain = join(dir, "plain.json");
  writeFileSync(plain, foreign());
  const noRules = spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "load-profile.mjs"), "--repo", "acme/api", "--profile", plain, "--save-rules", join(dir, "never.md")], { encoding: "utf8" });
  check("--save-rules with no rules file named writes nothing and still succeeds", noRules.status === 0 && JSON.parse(noRules.stdout).projectRules === null, noRules.stdout + noRules.stderr);
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall load-profile expectations hold");
