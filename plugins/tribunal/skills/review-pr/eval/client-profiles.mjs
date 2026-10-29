// The client profiles the replays test against, frozen: inputs come from here,
// goldens stay literal in each replay, since a golden read from the profile it
// checks is a tautology. They live with the rest of the client's harness data,
// outside the plugin; the copies reviews actually read are each repo's
// .claude/review-profile.json.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { PROFILES_DIR } from "./paths.mjs";

export function clientProfileFiles(dir = PROFILES_DIR) {
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((f) => join(dir, f));
}

export function loadClientProfiles(dir = PROFILES_DIR) {
  const out = {};
  for (const file of clientProfileFiles(dir)) {
    const profile = JSON.parse(readFileSync(file, "utf8"));
    if (out[profile.repo]) throw new Error(`two frozen profiles describe ${profile.repo}`);
    out[profile.repo] = profile;
  }
  return out;
}

// By reviewer kind, so a replay can name "the backend profile" without naming
// the client.
export function clientProfileByReviewer(kind, dir = PROFILES_DIR) {
  for (const file of clientProfileFiles(dir)) {
    const profile = JSON.parse(readFileSync(file, "utf8"));
    if (profile.reviewer === kind) return { profile, file };
  }
  throw new Error(`no frozen profile with reviewer ${kind} in ${dir}`);
}
