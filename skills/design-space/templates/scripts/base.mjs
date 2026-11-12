// Finds the branch that prototypes start from and open their pull request against: the profile's
// baseBranch first, then the remote's default branch. Returns undefined when neither is
// known. Trusted, engineer-owned code: it runs git itself.
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";

const SAFE_BRANCH = /^[A-Za-z0-9._\/-]+$/;

export function resolveBase(root) {
  let base;
  try {
    base = JSON.parse(readFileSync(join(root, ".design-space.json"), "utf8"))?.baseBranch;
  } catch { /* no profile, or no baseBranch in it */ }
  if (!base) {
    try {
      const out = execFileSync("git", ["ls-remote", "--symref", "origin", "HEAD"], {
        cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
      });
      base = /^ref: refs\/heads\/(\S+)\s+HEAD/m.exec(out)?.[1];
    } catch { /* handled by the caller */ }
  }
  return typeof base === "string" && SAFE_BRANCH.test(base) && !base.startsWith("-") ? base : undefined;
}
