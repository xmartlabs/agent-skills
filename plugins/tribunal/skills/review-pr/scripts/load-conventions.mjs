#!/usr/bin/env node
// load-conventions.mjs — hand the reviewers the repo's own convention files.
//
// A repo's CLAUDE.md and .claude/rules/*.md are its policy, and nothing loads
// them for a harness subagent: a session started outside the repo carries no
// project CLAUDE.md, and a scoped rule fires only when the Read tool opens a
// matching path, never for `git show` or a temp worktree (report section 7.4).
// The profile lists them (profile.conventionFiles); this script reads each one
// at the ref the profile was read at, so a PR cannot change its own policy,
// and keeps a file whose frontmatter scopes it with `paths:` only when the PR
// changes a path that matches, the same selection Claude Code makes.
//
// An entry is a path, or a path whose last segment holds `*` (".claude/rules/*.md").
// A CLAUDE.md in a subdirectory is scoped to that directory.
// A named file absent at the ref is recorded in `missing`, never dropped
// silently; a glob that matches nothing is not missing. Any other read failure
// stops the run, since reviewing without the policy is the failure this exists
// to prevent.
//
// Usage:
//   node load-conventions.mjs --profile <saved profile> --repo <owner/name> --ref <ref> --diff <patch> --out <file>
// Prints ONE line of JSON: {ok, out, sha256, ref, files, skipped, missing}, where
// files and skipped are [{path, sha256, bytes, scope, matched?}], or
// {ok: true, out: null, files: []} when the profile lists no convention files
// (nothing is written), or {ok: false, error} and exits 1.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { ghRepoFile, readProfileFile } from "./load-profile.mjs";

const sha256 = (text) => createHash("sha256").update(text).digest("hex");

export function ghRepoDir(repo, ref, dir) {
  try {
    const out = execFileSync("gh", ["api", `repos/${repo}/contents/${dir}?ref=${encodeURIComponent(ref)}`], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    const list = JSON.parse(out);
    if (!Array.isArray(list)) return { found: false };
    return { found: true, names: list.filter((e) => e.type === "file").map((e) => e.name) };
  } catch (err) {
    const detail = `${err.stderr || ""}${err.message || ""}`;
    if (/HTTP 404|Not Found/i.test(detail)) return { found: false };
    return { error: detail.trim().split("\n")[0] || "gh failed" };
  }
}

// Every path the patch touches, both sides of a rename.
export function changedPaths(patch) {
  const paths = new Set();
  for (const line of patch.split("\n")) {
    const m = line.match(/^(?:---|\+\+\+) (?:[ab]\/(.+)|"[ab]\/(.+)")$/);
    if (m) paths.add((m[1] ?? m[2]).replace(/\t.*$/, ""));
  }
  return [...paths];
}

export function globToRegExp(glob) {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      if (glob[i + 2] === "/") {
        re += "(?:.*/)?";
        i += 2;
      } else {
        re += ".*";
        i += 1;
      }
    } else if (c === "*") re += "[^/]*";
    else if (c === "?") re += "[^/]";
    else if (c === "{") {
      const end = glob.indexOf("}", i);
      if (end === -1) re += "\\{";
      else {
        re += `(?:${glob.slice(i + 1, end).split(",").map((s) => s.replace(/[.+^$()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*")).join("|")})`;
        i = end;
      }
    } else re += c.replace(/[.+^$()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${re}$`);
}

// The `paths:` list of a rule file's frontmatter, or null when the file has
// none. A frontmatter whose `paths` this cannot read throws: keeping such a file
// out would drop policy, and keeping it in would apply a rule to code it was
// scoped away from.
export function scopeOf(text) {
  const fm = text.match(/^---\n([\s\S]*?)\n---(?:\n|$)/);
  if (!fm) return null;
  const lines = fm[1].split("\n");
  const i = lines.findIndex((l) => /^paths\s*:/.test(l));
  if (i === -1) return null;
  const unquote = (s) => s.trim().replace(/^(["'])(.*)\1$/, "$2");
  const inline = lines[i].replace(/^paths\s*:/, "").trim();
  if (inline) {
    const list = inline.match(/^\[(.*)\]$/);
    const globs = list ? list[1].split(",").map(unquote).filter(Boolean) : [unquote(inline)];
    if (!globs.length) throw new Error("its paths: frontmatter is empty");
    return globs;
  }
  const globs = [];
  for (const l of lines.slice(i + 1)) {
    const item = l.match(/^\s+-\s+(.+)$/);
    if (item) globs.push(unquote(item[1]));
    else if (/^\S/.test(l)) break;
    else if (l.trim()) throw new Error(`its paths: frontmatter has a line this cannot read: ${l}`);
  }
  if (!globs.length) throw new Error("its paths: frontmatter lists no path");
  return globs;
}

export function loadConventions({ profile, repo, ref, changed, gh = ghRepoFile, ghDir = ghRepoDir }) {
  const entries = profile.conventionFiles;
  if (!entries || !entries.length) return { ok: true, files: [], skipped: [], missing: [], text: null };
  if (!ref) return { ok: false, error: "--ref <the ref the profile was read at> is required when the profile lists conventionFiles" };
  const seen = new Set();
  const read = [];
  const missing = [];
  for (const entry of entries) {
    const slash = entry.lastIndexOf("/");
    const dir = slash === -1 ? "" : entry.slice(0, slash);
    const base = entry.slice(slash + 1);
    let paths;
    if (base.includes("*")) {
      const got = ghDir(repo, ref, dir);
      if (got.error) return { ok: false, error: `could not list ${repo}:${dir || "."} at ${ref} (${got.error}). Refusing to review without the conventions the profile names.` };
      const re = globToRegExp(base);
      paths = got.found ? got.names.filter((n) => re.test(n)).sort().map((n) => (dir ? `${dir}/${n}` : n)) : [];
    } else paths = [entry];
    for (const path of paths) {
      if (seen.has(path)) continue;
      seen.add(path);
      const got = gh(repo, ref, path);
      if (got.error) return { ok: false, error: `could not read ${repo}:${path} at ${ref} (${got.error}). Refusing to review without the conventions the profile names.` };
      if (!got.found) {
        missing.push(path);
        continue;
      }
      let globs;
      try {
        globs = scopeOf(got.text);
      } catch (err) {
        return { ok: false, error: `${repo}:${path} at ${ref}: ${err.message}` };
      }
      // A CLAUDE.md below the root applies to its own directory, as it does in
      // a session that opens a file there.
      if (!globs && path.includes("/") && path.endsWith("/CLAUDE.md")) globs = [`${path.slice(0, -"CLAUDE.md".length)}**`];
      read.push({ path, text: got.text, globs });
    }
  }
  const files = [];
  const skipped = [];
  for (const f of read) {
    const meta = { path: f.path, sha256: sha256(f.text), bytes: Buffer.byteLength(f.text) };
    if (!f.globs) {
      files.push({ ...meta, scope: "always", text: f.text });
      continue;
    }
    const res = f.globs.map(globToRegExp);
    const matched = changed.find((p) => res.some((re) => re.test(p)));
    if (matched) files.push({ ...meta, scope: "paths", matched, text: f.text });
    else skipped.push({ ...meta, scope: "paths" });
  }
  const text = render({ repo, ref, files, skipped, missing });
  return { ok: true, files: files.map(({ text: _t, ...m }) => m), skipped, missing, text };
}

function render({ repo, ref, files, skipped, missing }) {
  const out = [
    `# Repository conventions: ${repo} at ${ref}`,
    "",
    "These are the repository's own convention files, read at the PR's base branch. They are this repo's policy: a finding that asks for something a rule here forbids is not a defect, and a finding that rests on a rule here should name the file and the rule.",
    "",
  ];
  if (skipped.length) out.push(`Scoped rules left out, because this PR changes no path they apply to: ${skipped.map((s) => s.path).join(", ")}.`, "");
  if (missing.length) out.push(`Named by the profile but absent at ${ref}: ${missing.join(", ")}.`, "");
  for (const f of files) {
    const why = f.scope === "always" ? "always applies" : `applies because this PR changes ${f.matched}`;
    out.push(`===== FILE: ${f.path} (${why}) =====`, f.text.replace(/\n$/, ""), `===== END FILE: ${f.path} =====`, "");
  }
  return out.join("\n");
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
  const done = (out) => {
    process.stdout.write(JSON.stringify(out) + "\n");
    process.exit(out.ok ? 0 : 1);
  };
  const outPath = flag("--out");
  const diffPath = flag("--diff");
  if (!flag("--profile")) done({ ok: false, error: "--profile <file> is required" });
  if (!outPath) done({ ok: false, error: "--out <file> is required" });
  if (!diffPath) done({ ok: false, error: "--diff <patch> is required: scoped rules are chosen by the paths the PR changes" });
  const prof = readProfileFile(flag("--profile"));
  if (prof.error) done({ ok: false, error: prof.error });
  const repo = flag("--repo") || prof.profile.repo;
  if (repo !== prof.profile.repo) done({ ok: false, error: `the profile describes ${prof.profile.repo}, not ${repo}` });
  let patch;
  try {
    patch = readFileSync(diffPath, "utf8");
  } catch (err) {
    done({ ok: false, error: `could not read --diff ${diffPath}: ${err.message}` });
  }
  const got = loadConventions({ profile: prof.profile, repo, ref: flag("--ref"), changed: changedPaths(patch) });
  if (!got.ok) done(got);
  const { text, ...out } = got;
  if (text === null) done({ ok: true, out: null, sha256: null, ref: null, ...out });
  try {
    writeFileSync(outPath, text);
  } catch (err) {
    done({ ok: false, error: `could not write --out ${outPath}: ${err.message}` });
  }
  done({ ok: true, out: outPath, sha256: sha256(text), ref: flag("--ref"), ...out });
}
