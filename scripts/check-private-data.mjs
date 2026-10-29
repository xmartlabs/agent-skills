#!/usr/bin/env node
// check-private-data.mjs — nothing from a private repository ships in this
// public one.
//
//   node scripts/check-private-data.mjs --repo <private checkout> [--repo ...] [--tokens <file>] [--exclude <path prefix> ...]
//
// For every file git tracks here (SVG and golden files included), it reports:
//   path      a path that exists, or ever existed, in one of the --repo checkouts
//   basename  a distinctive file name from those checkouts
//   sha       a hex string that is a commit id, or a prefix of one, of those checkouts
//   token     a match of the --tokens list (a JSON array of regular expressions)
//
// The private inputs come from the operator and are never written here. A match
// that is generic enough to keep (prisma/schema.prisma exists in every Prisma
// project) is accepted by listing the sha256 of the matched text, first 16 hex
// characters, in scripts/check-private-data.allow, so the allowlist names
// nothing either. --exclude drops a subtree of the private checkouts from the
// path and name lists, for directories that hold this repository's own files,
// such as a plugin's test data. Exits 1 on any match that is not allowed.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const values = (flag) => args.flatMap((a, i) => (a === flag ? [args[i + 1]] : []));
const repos = values("--repo");
const tokensFile = values("--tokens")[0];
const excluded = values("--exclude");
if (!repos.length && !tokensFile) {
  console.error("usage: check-private-data.mjs --repo <private checkout> [--repo ...] [--tokens <file>]");
  process.exit(2);
}

const hash = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);
const git = (dir, ...a) => execFileSync("git", ["-C", dir, ...a], { encoding: "utf8", maxBuffer: 1 << 28 });

// Basenames this short or this common say nothing about a codebase.
const GENERIC_STEM = /^(index|package|package-lock|tsconfig|readme|changelog|license|schema|migration|app|server|main|config|utils?|helpers?|types|constants|setup|settings|routes|marketplace|docker-compose|dockerfile|makefile)$/i;

// The private checkouts may hold copies of this repository's own files (a
// plugin can start life inside the repository it reviews), and those are not
// private data.
const ours = new Set(git(ROOT, "ls-files").split("\n").filter(Boolean).map((p) => p.slice(p.lastIndexOf("/") + 1)));

const paths = new Set();
const basenames = new Set();
const commits = [];
for (const dir of repos) {
  for (const p of git(dir, "log", "--all", "--name-only", "--format=").split("\n")) {
    if (!p || !p.includes("/") || excluded.some((e) => p.startsWith(e))) continue;
    const base = p.slice(p.lastIndexOf("/") + 1);
    if (ours.has(base)) continue;
    paths.add(p);
    const stem = base.split(".")[0];
    if (stem.length >= 9 && !GENERIC_STEM.test(stem)) basenames.add(base);
  }
  commits.push(...git(dir, "rev-list", "--all").split("\n").filter(Boolean));
}
const byPrefix = new Map();
for (const c of commits) {
  const k = c.slice(0, 7);
  if (!byPrefix.has(k)) byPrefix.set(k, []);
  byPrefix.get(k).push(c);
}
const isCommit = (h) => (byPrefix.get(h.slice(0, 7)) || []).some((c) => c.startsWith(h));
const tokens = tokensFile ? new RegExp(JSON.parse(readFileSync(tokensFile, "utf8")).join("|"), "gi") : null;

const allowFile = join(ROOT, "scripts", "check-private-data.allow");
const allow = new Set(
  existsSync(allowFile)
    ? readFileSync(allowFile, "utf8")
        .split("\n")
        .map((l) => l.replace(/#.*/, "").trim())
        .filter(Boolean)
    : [],
);

const found = [];
const report = (file, line, kind, text) => {
  if (!allow.has(hash(text))) found.push({ file, line, kind, text });
};
for (const file of git(ROOT, "ls-files").split("\n").filter(Boolean)) {
  let content;
  try {
    content = readFileSync(join(ROOT, file), "utf8");
  } catch {
    continue;
  }
  content.split("\n").forEach((line, i) => {
    // Base64 payloads (fonts inside the SVG and HTML diagrams) are not text.
    if (line.length > 4000 && /[A-Za-z0-9+/]{400}/.test(line)) return;
    for (const m of line.matchAll(/(?<![0-9a-f])[0-9a-f]{7,40}(?![0-9a-f])/g)) if (isCommit(m[0])) report(file, i + 1, "sha", m[0]);
    for (const m of line.matchAll(/[A-Za-z0-9_.@[\]-]+(?:\/[A-Za-z0-9_.@[\]-]+)+/g)) {
      const t = m[0].replace(/^\.\/|\.$/g, "");
      if (paths.has(t)) report(file, i + 1, "path", t);
    }
    for (const m of line.matchAll(/[A-Za-z0-9_-]+(?:\.[A-Za-z0-9]+)+/g)) if (basenames.has(m[0])) report(file, i + 1, "basename", m[0]);
    if (tokens) for (const m of line.matchAll(tokens)) report(file, i + 1, "token", m[0]);
  });
}

for (const f of found) console.log(`${f.file}:${f.line}  ${f.kind}  ${f.text}  (allow: ${hash(f.text)})`);
console.log(`${found.length} match(es) from private data across ${paths.size} paths and ${commits.length} commits`);
process.exit(found.length ? 1 : 0);
