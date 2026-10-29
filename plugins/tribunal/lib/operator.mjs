// operator.mjs — per-machine settings: never shipped with a plugin, never part
// of a project profile. One JSON file, which other review tools may share:
//
//   ${REVIEW_HARNESS_CONFIG:-~/.claude/review-harness.json}
//   { "defaultRepo": "owner/name" }
//
// `defaultRepo` is the repo a review runs against when the caller names none,
// the role a hardcoded default used to play.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const REPO_RE = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export function operatorConfigPath(env = process.env) {
  return env.REVIEW_HARNESS_CONFIG || join(homedir(), ".claude", "review-harness.json");
}

export function readOperatorConfig(path = operatorConfigPath()) {
  if (!existsSync(path)) return { config: {}, path, present: false };
  let config;
  try {
    config = JSON.parse(readFileSync(path, "utf8"));
  } catch (err) {
    return { error: `${path} is not valid JSON: ${err.message}`, path };
  }
  if (!config || typeof config !== "object" || Array.isArray(config))
    return { error: `${path} must hold a JSON object`, path };
  if (config.defaultRepo !== undefined && !REPO_RE.test(String(config.defaultRepo)))
    return { error: `${path}: defaultRepo must be owner/name, got ${JSON.stringify(config.defaultRepo)}`, path };
  return { config, path, present: true };
}

export function repoFromRemoteUrl(url) {
  const m = String(url || "")
    .trim()
    .match(/github\.com[:/]([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\.git)?\/?$/);
  return m ? m[1] : null;
}

export function cwdRepo(cwd = process.cwd()) {
  try {
    const url = execFileSync("git", ["-C", cwd, "remote", "get-url", "origin"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return repoFromRemoteUrl(url);
  } catch {
    return null;
  }
}

// --repo, else the operator's defaultRepo, else the current directory's origin.
// The operator default comes before the cwd so a machine that always reviewed
// one repo by default keeps doing so wherever the session was started.
export function resolveRepo({ flag = "", operator = readOperatorConfig(), cwd = process.cwd(), cwdRepoOf = cwdRepo } = {}) {
  if (flag)
    return REPO_RE.test(flag) ? { repo: flag, source: "flag" } : { error: `--repo must be owner/name, got ${JSON.stringify(flag)}` };
  if (operator.error) return { error: operator.error };
  if (operator.config.defaultRepo) return { repo: operator.config.defaultRepo, source: `operator:${operator.path}` };
  const fromCwd = cwdRepoOf(cwd);
  if (fromCwd) return { repo: fromCwd, source: "cwd-origin" };
  return {
    error: `no repo to review: pass --repo <owner/name>, set "defaultRepo" in ${operator.path}, or run from a checkout whose origin is on GitHub`,
  };
}
