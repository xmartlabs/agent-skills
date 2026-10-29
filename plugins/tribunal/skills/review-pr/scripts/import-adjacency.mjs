#!/usr/bin/env node
// import-adjacency.mjs — given file contents at HEAD for a small candidate
// set, decide which "carried forward" (unchanged-file) findings have an
// import/require relationship with a file this PR DID change.
//
// Usage (stdin, one JSON line in, one JSON line out):
//   echo '{"files": {...}, "carried": [...], "changed": [...], "aliases": {...}}' \
//     | node scripts/import-adjacency.mjs
//
// Prints ONE line of JSON:
//   {relatedFiles: {<carriedFile>: [{file, matchConfidence}, ...]}, scanned, aliasResolutionAttempted}
//
// WHY THIS EXISTS. classifyPriorFindings (workflows/tribunal.js, BEGIN
// CARRY-FORWARD region) auto-carries forward any prior finding whose file is
// in scope.unchanged, with no re-check by any agent. That's correct when the
// file truly wasn't touched by anything in this PR — wrong when the actual
// fix landed in a DIFFERENT file this PR DID change, one the unchanged file
// imports (or is imported by). PR #2012: a finding citing
// auth.controller.ts stayed "unchanged" while its fix landed in
// accounts.ts, which the controller calls into — carried forward as an open
// blocker anyway, because nothing ever crossed the two files.
//
// THIS IS A HEURISTIC, NOT A COMPILER. It resolves relative and
// tsconfig-path-aliased specifiers by SUFFIX/STEM matching against the
// small candidate set it was handed — never full Node module resolution,
// never a real dependency graph. Explicitly out of scope, by design:
//   - bare package specifiers ("lodash", "react") — no attempt to resolve
//   - tsconfig `extends` chains — the caller passes at most one level of
//     `compilerOptions.paths`, already resolved by the caller
//   - `require(someVariable)` — only string-literal specifiers are seen
//   - non-JS/TS files — the regex below only recognizes JS/TS import forms
// A relationship this can't see (a fix reached only through a bare-package
// re-export, or landing in a non-JS/TS file) still falls through to plain
// carry-forward — this narrows that failure class, it does not close it.
//
// Every match is tagged matchConfidence: "path" (a specifier that actually
// resolved — relative or alias) or "stem" (same filename, no resolvable
// path — the weaker fallback, kept distinguishable rather than silently
// trusted the same as a resolved match).

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

// Same symlink-safety concern as incremental-scope.mjs: this plugin is
// reached through ~/.claude/skills/tribunal, a symlink into a version-keyed
// cache dir, so a strict argv[1]-vs-import.meta.url comparison would never
// match and main() would silently never run.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

const IMPORT_RE =
  /(?:from\s*['"]([^'"]+)['"])|require\(\s*['"]([^'"]+)['"]\s*\)|import\(\s*['"]([^'"]+)['"]\s*\)/g;

export function extractSpecifiers(content) {
  const specs = [];
  IMPORT_RE.lastIndex = 0;
  let m;
  while ((m = IMPORT_RE.exec(content || ""))) specs.push(m[1] || m[2] || m[3]);
  return specs;
}

const EXTS = ["", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"];
const INDEX_SUFFIXES = ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"];

const stem = (p) => path.posix.basename(p).replace(/\.(tsx?|jsx?|mjs|cjs)$/, "");

export function computeAdjacency({ files, carried, changed, aliases }) {
  files = files || {};
  carried = [...new Set(carried || [])];
  changed = [...new Set(changed || [])];
  aliases = aliases || {};

  const knownPaths = new Set([...carried, ...changed]);
  const stemIndex = new Map();
  for (const p of knownPaths) {
    const s = stem(p);
    if (!stemIndex.has(s)) stemIndex.set(s, []);
    stemIndex.get(s).push(p);
  }

  // tsconfig `compilerOptions.paths` values are arrays ("@services/*":
  // ["src/services/*"]) — only the first target is used, matching this
  // script's "heuristic, not a compiler" scope.
  const aliasPrefixes = Object.entries(aliases)
    .filter(([, targets]) => (Array.isArray(targets) ? targets.length : targets))
    .map(([key, targets]) => ({
      keyPrefix: key.replace(/\*$/, ""),
      targetPrefix: (Array.isArray(targets) ? targets[0] : targets).replace(/\*$/, ""),
    }));

  function resolveSpecifier(spec, fromFile) {
    let resolvedNoExt = null;
    if (spec.startsWith(".")) {
      resolvedNoExt = path.posix.normalize(path.posix.join(path.posix.dirname(fromFile), spec));
    } else {
      const alias = aliasPrefixes.find(({ keyPrefix }) => spec.startsWith(keyPrefix));
      if (alias) resolvedNoExt = alias.targetPrefix + spec.slice(alias.keyPrefix.length);
    }
    if (resolvedNoExt === null) return null; // bare specifier — out of scope

    for (const ext of EXTS) {
      if (knownPaths.has(resolvedNoExt + ext)) return { file: resolvedNoExt + ext, matchConfidence: "path" };
    }
    for (const suf of INDEX_SUFFIXES) {
      if (knownPaths.has(resolvedNoExt + suf)) return { file: resolvedNoExt + suf, matchConfidence: "path" };
    }
    // Excludes fromFile itself — a file cannot have an import relationship
    // with itself, and without this a carried file whose own stem happens
    // to match the unresolved specifier (same basename, wrong directory)
    // would win the fallback ahead of the actual changed-file target.
    const stemMatches = (stemIndex.get(stem(resolvedNoExt)) || []).filter((p) => p !== fromFile);
    if (stemMatches.length) return { file: stemMatches[0], matchConfidence: "stem" };
    return null;
  }

  // Precompute each changed file's resolved import targets once, so the
  // reverse-direction check below (does a changed file import the carried
  // file?) doesn't re-extract/re-resolve per carried file.
  const changedFileTargets = new Map();
  for (const changedFile of changed) {
    const content = files[changedFile];
    if (content === undefined) continue;
    const targets = new Set();
    for (const spec of extractSpecifiers(content)) {
      const resolved = resolveSpecifier(spec, changedFile);
      if (resolved) targets.add(resolved.file);
    }
    changedFileTargets.set(changedFile, targets);
  }

  const relatedFiles = {};
  let scanned = 0;
  for (const carriedFile of carried) {
    const content = files[carriedFile];
    if (content === undefined) continue;
    scanned++;

    const found = new Map(); // file -> matchConfidence ("path" wins over "stem")
    // Forward: does the carried file import a changed file?
    for (const spec of extractSpecifiers(content)) {
      const resolved = resolveSpecifier(spec, carriedFile);
      if (resolved && changed.includes(resolved.file)) {
        const prev = found.get(resolved.file);
        if (!prev || (prev === "stem" && resolved.matchConfidence === "path"))
          found.set(resolved.file, resolved.matchConfidence);
      }
    }
    // Reverse: does a changed file import the carried file?
    for (const [changedFile, targets] of changedFileTargets) {
      if (targets.has(carriedFile) && !found.has(changedFile)) found.set(changedFile, "path");
    }

    if (found.size)
      relatedFiles[carriedFile] = [...found.entries()].map(([file, matchConfidence]) => ({
        file,
        matchConfidence,
      }));
  }

  return {
    relatedFiles,
    scanned,
    aliasResolutionAttempted: aliasPrefixes.length > 0,
  };
}

if (isMainModule()) {
  let input = "";
  try {
    input = readFileSync(0, "utf8");
  } catch {
    input = "";
  }
  let manifest;
  try {
    manifest = JSON.parse(input || "{}");
  } catch {
    manifest = {};
  }
  console.log(JSON.stringify(computeAdjacency(manifest)));
}
