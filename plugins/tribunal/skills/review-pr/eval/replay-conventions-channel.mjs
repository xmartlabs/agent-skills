#!/usr/bin/env node
// replay-conventions-channel.mjs — the CONVENTIONS-CHANNEL region of
// workflows/tribunal.js, executed: which line each dispatch carries, what the metrics row records, which args are refused, and
// whether the bash byte check stops a run on a file that is not the one
// SKILL.md loaded. The region is sliced out of the shipped workflow, not
// reimplemented.
//
// Usage: node eval/replay-conventions-channel.mjs

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};
const sha = (t) => createHash("sha256").update(t).digest("hex");

const { api, violations } = loadRegion(WORKFLOW, "CONVENTIONS-CHANNEL", ["conventionsChannel"]);
const METHODOLOGY_LINE = "METHODOLOGY FILE: /tmp/tribunal-methodology-acme-api-12.md";
const load = (parsedArgs, profile = { conventionFiles: ["CLAUDE.md"] }) => api.conventionsChannel(parsedArgs, profile);
const refused = (parsedArgs, profile) => {
  try {
    load(parsedArgs, profile);
    return "";
  } catch (err) {
    return err.message;
  }
};

const dir = mkdtempSync(join(tmpdir(), "conventions-channel-"));
const path = join(dir, "tribunal-conventions-acme-api-12.md");
const text = "# Repository conventions: acme/api at main\n\nMoney is integer cents.\n";
writeFileSync(path, text);
const manifest = {
  ok: true,
  out: path,
  sha256: sha(text),
  ref: "main",
  files: [
    { path: "CLAUDE.md", sha256: "a".repeat(64), bytes: 10, scope: "always" },
    { path: ".claude/rules/controllers.md", sha256: "b".repeat(64), bytes: 20, scope: "paths", matched: "src/controllers/x.ts" },
  ],
  skipped: [{ path: ".claude/rules/testing.md", sha256: "c".repeat(64), bytes: 30, scope: "paths" }],
  missing: ["AGENTS.md"],
};
const ARGS = { conventionsPath: path, conventionsSha256: sha(text), conventions: manifest };

console.log("\nconventions channel — the shipped region, executed");

check("the region is pure", violations.length === 0, violations.join(" | "));

console.log("\n--- with conventions loaded ---");
{
  const w = load(ARGS);
  const lens = w.reviewerPrompt(METHODOLOGY_LINE, "Review PR #12.");
  check("a reviewer prompt starts with the methodology line, then the conventions line, then a blank line", lens === `${METHODOLOGY_LINE}\nCONVENTIONS FILE: ${path}\n\nReview PR #12.`, JSON.stringify(lens));
  check("a refuter prompt starts with the conventions line and a blank line", w.withConventions("REFUTE — x") === `CONVENTIONS FILE: ${path}\n\nREFUTE — x`);
  check(
    "the row records the manifest without file text or byte counts",
    JSON.stringify(w.manifest) ===
      JSON.stringify({
        sha256: sha(text),
        ref: "main",
        files: [
          { path: "CLAUDE.md", sha256: "a".repeat(64), scope: "always" },
          { path: ".claude/rules/controllers.md", sha256: "b".repeat(64), scope: "paths", matched: "src/controllers/x.ts" },
        ],
        skipped: [".claude/rules/testing.md"],
        missing: ["AGENTS.md"],
      }),
    JSON.stringify(w.manifest),
  );
  const ok = spawnSync("bash", ["-c", `set -e${w.checkBash}\necho PASSED`], { encoding: "utf8" });
  check("the bash byte check passes on the file SKILL.md loaded", ok.status === 0 && ok.stdout.includes("PASSED"), ok.stdout + ok.stderr);
  writeFileSync(path, text + "edited\n");
  const bad = spawnSync("bash", ["-c", `${w.checkBash}\necho PASSED`], { encoding: "utf8" });
  check("and stops the run with a FATAL when the bytes changed", bad.status === 1 && bad.stdout.includes("FATAL") && !bad.stdout.includes("PASSED"), bad.stdout);
  writeFileSync(path, text);
  const partial = load({ ...ARGS, conventions: undefined });
  check("a manifest that did not arrive is recorded as unknown, not as empty", partial.manifest.files === null && partial.manifest.sha256 === sha(text));
}

console.log("\n--- without conventions, nothing changes ---");
{
  const w = load({}, { conventionFiles: undefined });
  check("a reviewer prompt is the methodology line, a blank line, the prompt", w.reviewerPrompt(METHODOLOGY_LINE, "Review PR #12.") === `${METHODOLOGY_LINE}\n\nReview PR #12.`);
  check("a refuter prompt is unchanged", w.withConventions("REFUTE — x") === "REFUTE — x");
  check("the bash adds no check", w.checkBash === "");
  check("the row records null", w.manifest === null);
  check("an empty conventionFiles list needs no file either", load({}, { conventionFiles: [] }).manifest === null);
}

console.log("\n--- refusals ---");
check("a profile that lists conventionFiles with no conventionsPath is refused", /lists conventionFiles, but args carry no conventionsPath/.test(refused({})));
check("a profile that lists only conventionSources with no conventionsPath is refused too", /lists conventionSources, but args carry no conventionsPath/.test(refused({}, { conventionSources: [{ repo: "acme/handbook", ref: "main" }] })));
{
  const source = { repo: "acme/handbook", ref: "main", resolvedSha: "1".repeat(40), pointerFrom: null, loaded: [{ path: "convention/a.md", bytes: 10, reason: "include" }], offered: [], bytesLoaded: 10, budgetBytes: 30000, missing: [] };
  const w = load({ ...ARGS, conventions: { ...ARGS.conventions, sources: [source] } });
  check("the row records each documentation source as load-conventions reported it", JSON.stringify(w.manifest.sources) === JSON.stringify([source]));
  check("a run without sources records no sources key", !("sources" in load(ARGS).manifest));
  const inRepo = { repo: null, ref: "main", resolvedSha: "2".repeat(40), pointerFrom: null, select: "manifest", root: "docs/", manifest: "docs/m.json", loaded: [{ path: "docs/a.md", bytes: 10, reason: "manifest" }], offered: ["docs/b.md"], bytesLoaded: 10, budgetBytes: 30000, missing: [] };
  const w2 = load({ ...ARGS, conventions: { ...ARGS.conventions, sources: [inRepo] } });
  check("an in-repo manifest source (repo null, root, manifest) reaches the row unchanged", JSON.stringify(w2.manifest.sources) === JSON.stringify([inRepo]), JSON.stringify(w2.manifest.sources));
}
check("a relative conventionsPath is refused", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "conv.md" })));
check("a conventionsPath with no sha256 is refused", /must be an absolute path with its conventionsSha256/.test(refused({ ...ARGS, conventionsSha256: undefined })));
check("a path with shell metacharacters is refused", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "/tmp/x$(id).md" })));

console.log("\n--- Windows paths ---");
check("a drive path with forward slashes is absolute", refused({ ...ARGS, conventionsPath: "C:/Users/dev/AppData/Local/Temp/c.md" }) === "");
check("a drive path with backslashes is absolute", refused({ ...ARGS, conventionsPath: "C:\\Users\\dev\\AppData\\Local\\Temp\\c.md" }) === "");
check("a backslash path without a drive is refused, it is relative on Unix", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "\\tmp\\c.md" })));
check("a drive with no separator is refused, it is relative to that drive", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "C:c.md" })));
check("a drive path with shell metacharacters is refused", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "C:/tmp/x$(id).md" })));
check("a temp dir under an 8.3 short name is absolute", refused({ ...ARGS, conventionsPath: "C:\\Users\\RUNNER~1\\AppData\\Local\\Temp/tribunal-conventions-a-b-1.md" }) === "");
check("a ~ in a Unix path is still refused", /must be an absolute path/.test(refused({ ...ARGS, conventionsPath: "/tmp/~x/c.md" })));
{
  // projectRulesPath is checked outside any region, so this reads the source.
  const src = readFileSync(WORKFLOW, "utf8");
  const rules = /if \(PROJECT_RULES_PATH && !(\/.+?\/)\.test\(PROJECT_RULES_PATH\)\)/.exec(src)?.[1];
  const conv = /if \(path && \(!(\/.+?\/)\.test\(path\)/.exec(src)?.[1];
  check("projectRulesPath is held to the same rule as conventionsPath", rules && rules === conv, `${rules} vs ${conv}`);
}

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all conventions-channel expectations hold"}`);
process.exit(failures.length ? 1 : 0);
