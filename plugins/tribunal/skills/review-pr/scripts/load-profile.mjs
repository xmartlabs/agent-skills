#!/usr/bin/env node
// load-profile.mjs — find, validate and print the project profile of ONE repo.
//
// Every project fact a review uses (which reviewer, which foci, what counts as a
// high-risk surface, deploy branches, sibling repos) lives in that repo's
// profile. The Workflow cannot read files, so SKILL.md runs this and passes the
// result through args; the in-context path runs it and reads the same fields.
//
// Discovery, first hit wins. There is no fallback to another repo's profile:
// borrowing another repo's reviewer and foci is exactly the silent failure FE
// PRs #1516 and #1554 measured.
//   1. --profile <path>                              (testing, or a one-off)
//   2. <repo>:.claude/review-profile.json at --ref    (the repo owns its facts;
//      read at the BASE ref so a PR cannot change its own review rules)
//      or, when the base branch predates the file, at the repo's default branch
// The plugin ships no profiles: a client's facts stay in the client's repo.
//
// Validation is the workflow's own PROFILE-LOOKUP region, sliced out of
// workflows/tribunal.js and run here, so the two cannot disagree.
//
// Usage:
//   node load-profile.mjs --pr <n> [--repo <owner/name>] [--ref <base ref>] [--profile <path> [--rules <path>]] [--save <path>] [--save-rules <path>]
//   (no --repo: the operator's defaultRepo, else the cwd's origin; see lib/operator.mjs)
// Prints ONE line of JSON: {ok: true, profile, sha256, source, profileRef, warnings,
// projectRules} on success, or {ok: false, error} and exits 1. --save writes
// the profile's text, byte for byte as read, so the scripts that take --profile
// later read the same bytes this sha256 describes; --save-rules does the same
// for the review rules file.
//
// profileRef is the ref the profile was read at (null for --profile), the ref
// load-conventions.mjs reads the convention files at.
//
// projectRules is {path, sha256, source} when the profile names a rules file,
// else null. The file is read at the same ref the profile was, for the same
// reason. A profile read from --profile has no ref, so its rules come from
// --rules; a profile that names a rules file nobody can find is refused.

import { resolveRepo } from "../../../lib/operator.mjs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_PROFILE_PATH = ".claude/review-profile.json";

// The workflow this script ships beside, not whatever CLAUDE_PLUGIN_ROOT names.
const WORKFLOW = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "workflows", "tribunal.js");

function profileLookupRegion() {
  const src = readFileSync(WORKFLOW, "utf8");
  const BEGIN = "// ===== BEGIN PROFILE-LOOKUP =====";
  const END = "// ===== END PROFILE-LOOKUP =====";
  const b = src.indexOf(BEGIN);
  const e = src.indexOf(END, b);
  if (b === -1 || e === -1) throw new Error(`no PROFILE-LOOKUP region in ${WORKFLOW}`);
  return new Function(
    `${src.slice(b + BEGIN.length, e)}\nreturn { validateProfile, riskPathRules, inContextRiskPathRules, profileWarnings };`,
  )();
}

const region = profileLookupRegion();
const { validateProfile } = region;
export const { riskPathRules, inContextRiskPathRules, profileWarnings } = region;

const sha256 = (text) => createHash("sha256").update(text).digest("hex");

export function ghRepoFile(repo, ref, path) {
  try {
    const text = execFileSync(
      "gh",
      ["api", `repos/${repo}/contents/${path}?ref=${encodeURIComponent(ref)}`, "-H", "Accept: application/vnd.github.raw"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    return { found: true, text };
  } catch (err) {
    const detail = `${err.stderr || ""}${err.message || ""}`;
    if (/HTTP 404|Not Found/i.test(detail)) return { found: false };
    return { error: detail.trim().split("\n")[0] || "gh failed" };
  }
}

export function ghDefaultBranch(repo) {
  try {
    const ref = execFileSync("gh", ["api", `repos/${repo}`, "--jq", ".default_branch"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
    return ref ? { ref } : { error: "gh returned no default branch" };
  } catch (err) {
    return { error: `${err.stderr || err.message || "gh failed"}`.trim().split("\n")[0] };
  }
}

export function loadProfile({ repo, ref = "", profilePath = "", rulesPath = "", gh = ghRepoFile, defaultBranchOf = ghDefaultBranch } = {}) {
  if (!repo) return { ok: false, error: "--repo <owner/name> is required" };
  const withRules = (accepted, at) => {
    if (!accepted.ok) return accepted;
    const out = { ...accepted, profileRef: at };
    const named = out.profile.projectRules;
    if (rulesPath) {
      let rulesText;
      try {
        rulesText = readFileSync(rulesPath, "utf8");
      } catch (err) {
        return { ok: false, error: `could not read --rules ${rulesPath}: ${err.message}` };
      }
      return { ...out, projectRules: { path: named || null, sha256: sha256(rulesText), source: `file:${rulesPath}` }, rulesText };
    }
    if (!named) return { ...out, projectRules: null };
    if (!at)
      return { ok: false, error: `${out.source} names projectRules ${named}, and a profile read from a file has no ref to read it at: pass --rules <path>` };
    const got = gh(repo, at, named);
    if (got.error) return { ok: false, error: `could not read ${repo}:${named} at ${at} (${got.error}). Refusing to review without the rules the profile names.` };
    if (!got.found) return { ok: false, error: `${out.source} names projectRules ${named}, which does not exist at ${at}` };
    return { ...out, projectRules: { path: named, sha256: sha256(got.text), source: `repo:${repo}@${at}:${named}` }, rulesText: got.text };
  };
  const accept = (text, source) => {
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch (err) {
      return { ok: false, error: `${source} is not valid JSON: ${err.message}` };
    }
    try {
      validateProfile(parsed, repo);
    } catch (err) {
      return { ok: false, error: `${source}: ${err.message}` };
    }
    return { ok: true, profile: parsed, sha256: sha256(text), source, warnings: profileWarnings(parsed), text };
  };

  if (profilePath) {
    let text;
    try {
      text = readFileSync(profilePath, "utf8");
    } catch (err) {
      return { ok: false, error: `could not read --profile ${profilePath}: ${err.message}` };
    }
    return withRules(accept(text, `file:${profilePath}`), null);
  }

  if (!ref) return { ok: false, error: `no base branch to read ${repo}:${REPO_PROFILE_PATH} at: pass --pr <n> or --ref <branch>, or --profile <path>` };
  const readAt = (at) => {
    const got = gh(repo, at, REPO_PROFILE_PATH);
    if (got.error)
      return { ok: false, error: `could not check ${repo}:${REPO_PROFILE_PATH} at ${at} (${got.error}). Refusing to guess which profile applies.` };
    return got.found ? withRules(accept(got.text, `repo:${repo}@${at}:${REPO_PROFILE_PATH}`), at) : null;
  };
  const atBase = readAt(ref);
  if (atBase) return atBase;
  // A base branch cut before the profile was committed (a release branch, a
  // stacked feature branch, a deploy branch) has no copy. The default branch's
  // copy is still the repo's own, and a PR cannot change it without merging.
  const def = defaultBranchOf(repo);
  if (def.error)
    return { ok: false, error: `no ${REPO_PROFILE_PATH} in ${repo} at ${ref}, and its default branch could not be read (${def.error}). Refusing to guess which profile applies.` };
  if (def.ref !== ref) {
    const atDefault = readAt(def.ref);
    if (atDefault) return atDefault;
  }
  const where = def.ref === ref ? ref : `${ref} or on the default branch ${def.ref}`;
  return {
    ok: false,
    error: `no profile for ${repo} at ${where}. Commit ${REPO_PROFILE_PATH} to the default branch, or pass --profile <path>. Tribunal does not review a repo with another repo's profile.`,
  };
}

// A profile file another script was handed with --profile: the bytes
// load-profile.mjs --save wrote. Validated again, since a file on disk can be
// anything by the time it is read.
export function readProfileFile(path) {
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch (err) {
    return { error: `could not read the profile at ${path}: ${err.message}` };
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    return { error: `${path} is not valid JSON: ${err.message}` };
  }
  try {
    validateProfile(parsed, null);
  } catch (err) {
    return { error: `${path}: ${err.message}` };
  }
  return { profile: parsed, sha256: sha256(text) };
}

export function ghBaseRef(repo, pr) {
  try {
    const ref = execFileSync("gh", ["pr", "view", String(pr), "--repo", repo, "--json", "baseRefName", "--jq", ".baseRefName"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
    return ref ? { ref } : { error: "gh returned no base branch" };
  } catch (err) {
    return { error: `${err.stderr || err.message || "gh failed"}`.trim().split("\n")[0] };
  }
}

// The whole lookup the skill needs in one call: which repo (--repo, else the
// operator's defaultRepo, else the cwd's origin), the PR's base branch when
// --pr is given, then the profile. The answer says where the repo came from.
export function resolveAndLoad({ repoFlag = "", pr = "", ref = "", profilePath = "", rulesPath = "", operator, cwd, cwdRepoOf, gh, baseRefOf = ghBaseRef, defaultBranchOf } = {}) {
  const resolved = resolveRepo({ flag: repoFlag, ...(operator ? { operator } : {}), ...(cwd ? { cwd } : {}), ...(cwdRepoOf ? { cwdRepoOf } : {}) });
  if (resolved.error) return { ok: false, error: resolved.error };
  let baseRef = ref;
  if (!baseRef && pr) {
    const got = baseRefOf(resolved.repo, pr);
    if (got.error) return { ok: false, error: `could not read the base branch of PR #${pr} in ${resolved.repo} (${got.error})` };
    baseRef = got.ref;
  }
  const out = loadProfile({ repo: resolved.repo, ref: baseRef, profilePath, rulesPath, ...(gh ? { gh } : {}), ...(defaultBranchOf ? { defaultBranchOf } : {}) });
  return { ...out, repo: resolved.repo, repoSource: resolved.source, baseRef: baseRef || null };
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

if (isMain) {
  const argv = process.argv.slice(2);
  const flag = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : "";
  };
  const { text, rulesText, ...out } = resolveAndLoad({
    repoFlag: flag("--repo"),
    pr: flag("--pr"),
    ref: flag("--ref"),
    profilePath: flag("--profile"),
    rulesPath: flag("--rules"),
  });
  for (const [savePath, bytes, what] of [
    [flag("--save"), text, "--save"],
    [flag("--save-rules"), rulesText, "--save-rules"],
  ]) {
    // No rules file named: --save-rules writes nothing, and projectRules: null
    // in the output says so.
    if (!out.ok || !savePath || bytes === undefined) continue;
    try {
      writeFileSync(savePath, bytes);
    } catch (err) {
      out.ok = false;
      out.error = `loaded the profile but could not write ${what} ${savePath}: ${err.message}`;
    }
  }
  process.stdout.write(JSON.stringify(out) + "\n");
  process.exit(out.ok ? 0 : 1);
}
