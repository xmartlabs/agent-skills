#!/usr/bin/env node
// replay-harness-data-paths.mjs — the harness data committed next to the
// plugins names nobody's machine.
//
// Recorded runs, CLI outputs and notes carry the paths of the machine they were
// taken on: a home directory, a per-user temp directory where a run kept its
// artifacts. In a shared repo those say whose machine it was and nothing a
// replay needs. refresh-ledger-snapshot.mjs rewrites them on the way in; this
// fails when one is there anyway, checked against the machine running it plus
// the shape macOS gives every user's temp directory.
//
// Usage: node eval/replay-harness-data-paths.mjs

import { readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, relative } from "node:path";
import { HARNESS_DATA, projectSlug } from "./paths.mjs";
import { encodedHome, scrubMachinePaths } from "./refresh-ledger-snapshot.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    else files.push(p);
  }
};
walk(HARNESS_DATA);

const needles = new Set([`${homedir().replace(/\/+$/, "")}/`, encodedHome()]);
for (const t of [tmpdir(), (() => { try { return realpathSync(tmpdir()); } catch { return tmpdir(); } })()])
  if (t.replace(/\/+$/, "") !== "/tmp") needles.add(t.replace(/\/+$/, ""));
const MAC_USER_TMP = /\/var\/folders\/[A-Za-z0-9_]{2}\/[A-Za-z0-9_]+\/T/;

console.log(`\nharness data paths — ${files.length} files under ${relative(process.cwd(), HARNESS_DATA) || HARNESS_DATA}`);
check("the scan found the data it should (not vacuous)", files.length > 50, String(files.length));
const hits = [];
for (const f of files) {
  let text;
  try {
    text = readFileSync(f, "utf8");
  } catch {
    continue;
  }
  for (const n of needles) if (text.includes(n)) hits.push(`${relative(HARNESS_DATA, f)}: ${n}`);
  if (MAC_USER_TMP.test(text)) hits.push(`${relative(HARNESS_DATA, f)}: a per-user /var/folders temp dir`);
}
check("no file names this machine's home or temp directory", hits.length === 0, hits.slice(0, 8).join("; ") + (hits.length > 8 ? ` (+${hits.length - 8})` : ""));

const sample = '{"artifactDir":"/var/folders/ab/xyz123abc/T//peer-review-votes-1-A","note":"/home/op/.claude/x"}';
const scrubbed = scrubMachinePaths(sample, { home: "/home/op", tmp: "/var/folders/ab/xyz123abc/T/" });
check(
  "the refresh rewrites a home and a per-user temp dir",
  scrubbed === '{"artifactDir":"/tmp//peer-review-votes-1-A","note":"~/.claude/x"}',
  scrubbed,
);
{
  const session = scrubMachinePaths(
    "cd /private/tmp/claude-501/-home-op-work-repo/4f2a/scratchpad/lr && ls ~/.claude/projects/-home-op-work-repo/x.jsonl",
    { home: "/home/op", tmp: "/tmp" },
  );
  check(
    "the refresh rewrites a Claude Code session dir and a projects slug that encode the home",
    session === "cd /tmp/4f2a/scratchpad/lr && ls ~/.claude/projects/-~-work-repo/x.jsonl",
    session,
  );
  check("the needle for this machine is its encoded home", encodedHome("/home/op") === "-home-op");
}
check("a Windows cwd becomes Claude Code's projects slug, not the cwd itself", projectSlug("C:\\GIT\\FIVE-STONE") === "C--GIT-FIVE-STONE");
check("a macOS cwd keeps the slug it always had", projectSlug("/Users/op/work/A1_api.v2") === "-Users-op-work-A1-api-v2");
check("the projects slug and the encoded home follow one rule", projectSlug("/home/op") === encodedHome("/home/op"));
check("and leaves a path that names no machine alone", scrubMachinePaths("/tmp/review-1.patch src/a.ts", { home: "/home/op", tmp: "/tmp" }) === "/tmp/review-1.patch src/a.ts");

console.log(`\n${failures.length ? `${failures.length} FAILED` : "the harness data names no machine"}`);
process.exit(failures.length ? 1 : 0);
