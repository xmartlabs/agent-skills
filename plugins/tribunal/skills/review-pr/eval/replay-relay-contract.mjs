#!/usr/bin/env node
// replay-relay-contract.mjs — no new unverified relay, by construction.
//
// A Workflow script has no filesystem, so everything that crosses between it
// and disk is text a mechanic agent copies, and that copy is a lossy channel
// (agent-harness-design rule 16). v80 and v81 moved every relay that carried
// real data onto the VERIFIED-RELAY path: a checksummed line from the script, a
// heredoc checked on disk, one retry on sonnet, an explicit degrade. This file
// keeps it that way. It fails when:
//
//   1. a dispatch spreads MECHANICAL outside relayAgent and is not on the
//      allowlist below, which names why each remaining one is safe as it is;
//   2. an allowlisted dispatch no longer exists, so the list cannot rot into
//      blanket permission;
//   3. a relayAgent call does not hand the agent RELAY_RULES (the file rule
//      plus RELAY_RETURN_RULES), or its script output does not go through
//      relayFilePipe, so the relay would be copied by a model again;
//   4. a heredoc's whole body is one interpolated value and it does not go
//      through verifiedHeredoc, unless its tag is allowlisted with a reason;
//   5. diff text or bot comment bodies are inlined into a prompt again.
//
// Usage: node eval/replay-relay-contract.mjs

import { readFileSync } from "node:fs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const MECHANIC_ALLOWLIST = {
  relayAgent: "the verified relay itself: checksum, one sonnet retry, an explicit degrade",
  "context-bookkeeping":
    "a small schema'd lookup (prior review, scope lists, import adjacency) that fails open: any doubt leaves incrementalScope null and the run reads every file",
  "setup-head-worktree": "returns one fixed marker line (CREATED_WORKTREE, CLONED or WORKTREE_FAILED) that parseHeadWorktree parses",
  "check-external-heads":
    "fixed markers plus tsc diagnostics; the coverage list is narrowed to the PR's files on disk (TYPECHECK-COVERAGE) before it is copied",
  "grounded-typecheck": "the same typecheck text as check-external-heads, on the path where that probe did not run",
  "heads:": "external-head drivers: the votes they return are reconciled against the CLI artifacts on disk by lib/reconcile-votes.mjs",
  classifyArtifacts: "one JSON line carrying its own expected total, which the caller checks, so an abridged copy is a detected event",
  "heads:codex:sweep": "launches the sweep CLI in the background and returns launch markers only",
  "read:sweep-artifact": "copies the sweep's answer; a copy the parser cannot read is recorded as an unparseable sweep, which downgrades a clean verdict",
  "ref-evidence-check": "prints MISSING lines for cited identifiers absent at the pinned head; a lost line under-reports a suspect vote in shadow evidence, it decides nothing",
  "persist-parts": "the persist part writer: lib/persist-parts.mjs re-checks every part on disk before anything is joined, and the writer's own answer is never read",
  persistRelayOpts: "copies back persist-run.sh's contract line only; the payload travels in verified parts since v81",
  "teardown-and-verify-metrics-row": "a read-back of the row just written, compared with the findings count the run expects",
  "verify-row-before-retry": "a read-back of the last row when the persist relay returned something nobody can read; the retry runs only when that row is not this run's, so it cannot write a row twice",
};

const HEREDOC_ALLOWLIST = {
  __PR_FILES__: "checked inline with relay.mjs check; an altered copy reports coverage as unknown (TYPECHECK-COVERAGE)",
};

const src = readFileSync(WORKFLOW, "utf8");
const lines = src.split("\n");

console.log("\n--- 1-2. every mechanic dispatch is a verified relay or named here ---");
const found = new Map();
lines.forEach((l, i) => {
  if (!l.includes("...MECHANICAL")) return;
  let key = null;
  let dist = Infinity;
  for (let k = Math.max(0, i - 14); k <= Math.min(lines.length - 1, i + 6); k++) {
    const m = lines[k].match(/label:\s*"([^"]+)"|label:\s*`([^`$]*)/);
    const param = /\blabel,|\blabel: l\b/.test(lines[k]);
    if ((m || param) && Math.abs(k - i) < dist) {
      dist = Math.abs(k - i);
      key = m ? m[1] ?? m[2] : null;
    }
  }
  if (!key)
    for (let k = i; k >= 0; k--) {
      const m = lines[k].match(/^(?:async function|function|const) (\w+)/);
      if (m) {
        key = m[1];
        break;
      }
    }
  found.set(key, [...(found.get(key) || []), i + 1]);
});
const unlisted = [...found.keys()].filter((k) => !(k in MECHANIC_ALLOWLIST));
check("every MECHANICAL dispatch is relayAgent or allowlisted with a reason", unlisted.length === 0, unlisted.map((k) => `${k} (line ${found.get(k).join(",")})`).join("; "));
const stale = Object.keys(MECHANIC_ALLOWLIST).filter((k) => !found.has(k));
check("every allowlisted dispatch still exists", stale.length === 0, stale.join(", "));
check("the scan is not vacuous", found.size >= 10 && found.has("relayAgent"), `${found.size}`);

console.log("\n--- 3. every relayAgent call hands over the return rules and pipes its relay to a file ---");
const calls = [...src.matchAll(/relayAgent\(\s*(?:noteBashSize\([^,]+,\s*)?`/g)].map((m) => m.index);
const lineOf = (at) => `line ${src.slice(0, at).split("\n").length}`;
const callText = (at) => {
  const end = src.indexOf("{ label:", at);
  return end === -1 ? "" : src.slice(at, end);
};
const missingRules = calls.filter((at) => !callText(at).includes("${RELAY_RULES}"));
check(`all ${calls.length} relayAgent calls carry RELAY_RULES`, calls.length >= 5 && missingRules.length === 0, missingRules.map(lineOf).join(", "));
const fetchBash = src.slice(src.indexOf("const FETCH_PR_BASH"), src.indexOf("`;", src.indexOf("const FETCH_PR_BASH")) + 2);
const missingPipe = calls.filter((at) => !callText(at).includes("relayFilePipe(") && !(callText(at).includes("${FETCH_PR_BASH}") && fetchBash.includes("relayFilePipe(")));
check(`all ${calls.length} relayAgent calls pipe their relay through relay-file.mjs`, missingPipe.length === 0, missingPipe.map(lineOf).join(", "));

console.log("\n--- 4. a data heredoc goes through verifiedHeredoc ---");
const raw = [...src.matchAll(/<<'([A-Za-z0-9_]+)'\n\$\{[^\n]*\}\n\1\b/g)].map((m) => ({ tag: m[1], line: src.slice(0, m.index).split("\n").length }));
const rawUnlisted = raw.filter((h) => !(h.tag in HEREDOC_ALLOWLIST));
check("no heredoc whose body is one interpolated value bypasses verifiedHeredoc", rawUnlisted.length === 0, rawUnlisted.map((h) => `${h.tag} (line ${h.line})`).join(", "));
const staleTags = Object.keys(HEREDOC_ALLOWLIST).filter((t) => !raw.some((h) => h.tag === t));
check("every allowlisted heredoc still exists", staleTags.length === 0, staleTags.join(", "));
check("the resolver payloads go down through verifiedWrite (chunked) or verifiedHeredoc", (src.match(/\$\{verifiedHeredoc\(|\.\.\.verifiedWrite\(/g) || []).length >= 2);

console.log("\n--- 5. no text that lives on disk is inlined into a prompt ---");
check("no prompt inlines prContext.diff", !/\$\{prContext\.diff\}/.test(src));
check("no prompt inlines the bot comment bodies", !/JSON\.stringify\(prContext\.botComments/.test(src));

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall relay-contract expectations hold");
