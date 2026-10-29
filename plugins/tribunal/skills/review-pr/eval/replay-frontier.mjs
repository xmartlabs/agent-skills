#!/usr/bin/env node
// replay-frontier.mjs — the line between the plugin and its clients.
//
// A client's data does not ship in the plugin: profiles live in each repo, and
// recorded runs, fixtures, corpus, specs and the release log live in the
// documentation repo's review-harness-data/ (eval/paths.mjs). `mustNotExist`
// names the paths that used to hold them, so none comes back. Inside the plugin,
// client names and client domain terms are a ratchet: a file may keep at most
// the count recorded in `allowed`, a file not listed may not mention the client
// at all, and a count may only go down. This check is what turns "make tribunal
// project-agnostic" from an intention into a number that cannot drift back up
// while the migration is in progress.
//
// It sees EXPLICIT coupling only (names, paths, domain identifiers). Implicit
// coupling, such as a rule written for one codebase's shape, has no token to
// find; that half is tracked in the coupling inventory, not here.
//
// Usage: node eval/replay-frontier.mjs [--tighten]   (--tighten prints the new
// allowlist when counts went down)

import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { join, relative } from "node:path";
import { HARNESS_DATA, PLUGIN_ROOT } from "./paths.mjs";

const SPEC = JSON.parse(readFileSync(join(PLUGIN_ROOT, "skills/review-pr/eval/frontier.json"), "utf8"));
// The client terms are not in this repository: listing them here would name the
// client. They live in the test-data directory as frontier-tokens.json (a JSON
// array of regular expressions), or at TRIBUNAL_FRONTIER_TOKENS. Without the
// list the scan is skipped and says so. When the test-data directory exists but
// the list does not, that is a failure, so the full gate cannot skip it silently.
const TOKENS_FILE =
  process.env.TRIBUNAL_FRONTIER_TOKENS || process.env.REVIEWER_TOOLS_FRONTIER_TOKENS || join(HARNESS_DATA, "frontier-tokens.json");
const TOKENS = existsSync(TOKENS_FILE) ? JSON.parse(readFileSync(TOKENS_FILE, "utf8")) : null;
const TOKEN = TOKENS && TOKENS.length ? new RegExp(TOKENS.join("|"), "gi") : null;

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// The spec file holds the token patterns themselves, so it is not scanned.
const SELF = "skills/review-pr/eval/frontier.json";
function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const abs = join(dir, name);
    const rel = relative(PLUGIN_ROOT, abs);
    if (rel === SELF) continue;
    if (statSync(abs).isDirectory()) walk(abs, out);
    else if (!SPEC.skipExtensions.some((e) => name.endsWith(e))) out.push(rel);
  }
  return out;
}

const counts = {};
for (const rel of walk(PLUGIN_ROOT)) {
  let text;
  try {
    text = readFileSync(join(PLUGIN_ROOT, rel), "utf8");
  } catch {
    continue;
  }
  const n = TOKEN ? (text.match(TOKEN) || []).length : 0;
  if (n) counts[rel] = n;
}

console.log("\nfrontier — no client data in the plugin, and client names may only shrink");
const tokensMissing = !TOKEN;
if (tokensMissing) console.log(`        token scan skipped: no client-term list at ${TOKENS_FILE}`);
check("the client-term list is present when the test-data directory is", !(tokensMissing && existsSync(HARNESS_DATA) && !process.env.REVIEWER_TOOLS_FRONTIER_TOKENS), `${HARNESS_DATA} exists but has no frontier-tokens.json`);
const back = SPEC.mustNotExist.filter((c) => existsSync(join(PLUGIN_ROOT, c.replace(/\/$/, ""))));
check("none of the paths that held client data is back in the plugin", SPEC.mustNotExist.length > 0 && back.length === 0, back.join(", "));
check("the scan found the files it should (not vacuous)", tokensMissing || Object.keys(counts).length > 0 || Object.keys(SPEC.allowed).length === 0);

const grew = [];
const shrank = [];
for (const [rel, n] of Object.entries(counts)) {
  const allowed = SPEC.allowed[rel] ?? 0;
  if (n > allowed) grew.push(`${rel}: ${n} > ${allowed}`);
  else if (n < allowed) shrank.push(`${rel}: ${allowed} -> ${n}`);
}
if (!tokensMissing) for (const rel of Object.keys(SPEC.allowed)) if (!(rel in counts)) shrank.push(`${rel}: ${SPEC.allowed[rel]} -> 0`);

check("no file in the plugin names the client more than it already did", grew.length === 0, grew.join("; "));
const total = Object.values(counts).reduce((a, b) => a + b, 0);
const allowedTotal = Object.values(SPEC.allowed).reduce((a, b) => a + b, 0);
console.log(`        ${total} mention(s) in ${Object.keys(counts).length} file(s); allowlist ${allowedTotal} in ${Object.keys(SPEC.allowed).length}`);
if (shrank.length) {
  console.log(`        went down (lower the allowlist to lock it in): ${shrank.join("; ")}`);
  if (process.argv.includes("--tighten")) console.log(JSON.stringify(Object.fromEntries(Object.entries(counts).sort()), null, 2));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe frontier holds");
