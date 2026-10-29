#!/usr/bin/env node
// blast-radius.mjs — list the UNCHANGED code that depends on what the PR changed.
//
// BE #2167 (2026-10-18): queue.ts moved an accounting-sync queue onto a
// fail-fast Redis connection. An unchanged webhook controller fires the job
// enqueue without await or catch and answers 200, so a Redis blip now drops a
// webhook the accounting provider will not resend. Lenses raised it in
// 2 of 10 runs: nothing told them that caller existed, the methodology limits
// caller checks to the same file, and the defect sits in a file the PR never
// touched. This script finds such callers deterministically, two hops out:
//
//   changed declaration -> a declaration that uses it (same file, or a file that
//   imports it) -> call sites of that declaration in files outside the PR.
//
// A HEURISTIC, NOT A COMPILER. Declarations are found by a column-0 regex, names
// by whole-word grep, imports by specifier (relative or tsconfig paths, the same
// forms import-adjacency.mjs reads). Types and interfaces are left out: the
// typecheck already sees those breaks. `awaited` means awaited, returned,
// chained or handed over as a callback, read from the call line and the line
// before it only, so it is a hint for the lens, never a verdict.
//
//   node blast-radius.mjs <diff-file> <repo-dir> [--rev <sha>]
//
// Reads the files in <repo-dir> as checked out, or at <sha> with --rev.
// stdout: relayLines({ changed: [{file, symbols}], callers: [{file, line, via,
//         call, awaited, text}], total, truncated })

import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { relayLines } from "./relay.mjs";
import { attentionFacts, loadTypescript } from "./attention-facts.mjs";

export const MAX_CALLERS = 25;
const SOURCE_RE = /\.(tsx?|jsx?|mjs|cjs)$/;
const TEST_RE = /(^|\/)(__tests__|__mocks__|tests?)\/|\.(test|spec)\.[^/]+$/;
const DECL_RE =
  /^(export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?(function\*?|const|let|var|class|enum|type|interface)\s+([A-Za-z_$][\w$]*)/;
const IMPORT_RE =
  /(?:from\s*['"]([^'"]+)['"])|require\(\s*['"]([^'"]+)['"]\s*\)|import\(\s*['"]([^'"]+)['"]\s*\)/g;
const TEXT_CHARS = 100;

export function changedLines(diff) {
  const out = new Map();
  let file = null;
  let line = 0;
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("diff --git ")) {
      file = null;
      continue;
    }
    if (raw.startsWith("+++ ")) {
      const p = raw.slice(4).trim();
      file = p === "/dev/null" ? null : p.replace(/^b\//, "");
      if (file && !out.has(file)) out.set(file, new Set());
      continue;
    }
    const h = raw.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (h) {
      line = Number(h[1]);
      continue;
    }
    if (!file || raw.startsWith("--- ")) continue;
    if (raw.startsWith("+")) {
      out.get(file).add(line);
      line++;
    } else if (raw.startsWith("-")) {
      // A pure deletion still changes the declaration around it.
      out.get(file).add(line);
    } else if (raw.startsWith(" ")) line++;
  }
  return out;
}

const COMMENT_LINE = /^\s*(\/\/|\/\*|\*)/;

// CommonJS exports, which the legacy half of a repo uses instead of `export`:
// `exports.name =` and `module.exports.name =` declare and export at once, and
// `module.exports = { a, b: c }` exports declarations made above it. Until
// 2026-10-21 none of these counted as exported, so a changed legacy helper
// listed no callers at all: `runWithConcurrencySettled` in the BE has three.
const CJS_DECL_RE = /^(?:module\.)?exports\.([A-Za-z_$][\w$]*)\s*=/;
const CJS_OBJECT_START = /^module\.exports\s*=\s*\{/;
const CJS_ENTRY = /^([A-Za-z_$][\w$]*)\s*(?::\s*([A-Za-z_$][\w$]*))?$/;

// local name -> the name an importer uses, from every `module.exports = {...}`.
export function cjsExportNames(lines) {
  const out = new Map();
  for (let i = 0; i < lines.length; i++) {
    if (!CJS_OBJECT_START.test(lines[i])) continue;
    let text = lines[i].replace(CJS_OBJECT_START, "");
    let j = i;
    while (!/^\s*\}/.test(lines[j]) && !/\}\s*;?\s*$/.test(text) && j + 1 < lines.length && j - i < 500) text += `\n${lines[++j]}`;
    text = text.replace(/\}\s*;?\s*$/, "").replace(/\/\/[^\n]*/g, "");
    for (const entry of text.split(",")) {
      const m = entry.trim().match(CJS_ENTRY);
      if (m) out.set(m[2] || m[1], m[1]);
    }
    i = j;
  }
  return out;
}

// A declaration owns the comment block right above it, so a PR that only
// rewrites that comment changes the declaration below, not the one above.
// `exportAs` is the name importers use, which differs from `name` only for a
// CommonJS `module.exports = { exported: local }`.
export function declarations(content) {
  const lines = String(content || "").split("\n");
  const decls = [];
  lines.forEach((l, i) => {
    const m = l.match(DECL_RE);
    const c = m ? null : l.match(CJS_DECL_RE);
    if (!m && !c) return;
    let lead = i;
    while (lead > 0 && COMMENT_LINE.test(lines[lead - 1])) lead--;
    decls.push(
      m
        ? { name: m[3], kind: m[2], exported: Boolean(m[1]), start: lead + 1, at: i + 1 }
        : { name: c[1], kind: "cjs", exported: true, start: lead + 1, at: i + 1 },
    );
  });
  const cjs = cjsExportNames(lines);
  decls.forEach((d, i) => {
    d.end = i + 1 < decls.length ? decls[i + 1].start - 1 : lines.length;
    d.body = lines.slice(d.at - 1, d.end).join("\n");
    if (cjs.has(d.name) && !isType(d)) d.exported = true;
    d.exportAs = cjs.get(d.name) || d.name;
  });
  return decls;
}

// Lines that belong to an import or re-export statement, which can span lines.
function importLines(lines) {
  const out = new Set();
  for (let i = 0; i < lines.length; i++) {
    if (/\brequire\(\s*['"]/.test(lines[i])) {
      out.add(i + 1);
      // `const {\n  a,\n  b\n} = require('x')`: the names above are the import too.
      if (/^\s*\}\s*=\s*require\(/.test(lines[i]))
        for (let k = i - 1; k >= 0 && i - k <= 200; k--) {
          out.add(k + 1);
          if (/^\s*(const|let|var)\s*\{/.test(lines[k])) break;
        }
      continue;
    }
    if (!/^\s*(import\b|export\s+(\*|\{))/.test(lines[i])) continue;
    let j = i;
    while (j < lines.length && !/\bfrom\s*['"]|^\s*import\s*['"]|require\(/.test(lines[j]) && j - i < 200) j++;
    if (j >= lines.length) j = i;
    for (let k = i; k <= j; k++) out.add(k + 1);
    i = j;
  }
  return out;
}

const isType = (d) => d.kind === "type" || d.kind === "interface";
const wordRe = (name) => new RegExp(`(^|[^\\w$])${name.replace(/\$/g, "\\$")}([^\\w$]|$)`);
const noExt = (p) => p.replace(SOURCE_RE, "");

// tsconfig is JSONC. Comments are stripped outside strings only: the BE's own
// "include": ["src/**/*"] reads as a block comment to a plain regex.
function stripJsonc(text) {
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j;
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
      out += "\n";
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 1;
    } else out += c;
  }
  return out.replace(/,(\s*[}\]])/g, "$1");
}

export function readAliases(tsconfigText) {
  try {
    const opts = JSON.parse(stripJsonc(String(tsconfigText || ""))).compilerOptions || {};
    const base = opts.baseUrl || ".";
    const aliases = {};
    for (const [k, v] of Object.entries(opts.paths || {}))
      if (Array.isArray(v) && v[0]) aliases[k] = path.posix.normalize(path.posix.join(base, v[0]));
    return aliases;
  } catch {
    return {};
  }
}

// Does `spec`, written in `importer`, point at `target`? Relative and aliased
// specifiers only; a bare package name never matches a repo file.
export function specPointsAt(spec, importer, target, aliases) {
  let resolved = null;
  if (spec.startsWith(".")) resolved = path.posix.normalize(path.posix.join(path.posix.dirname(importer), spec));
  else
    for (const [k, v] of Object.entries(aliases)) {
      if (k.endsWith("/*") && spec.startsWith(k.slice(0, -1))) {
        resolved = path.posix.normalize(v.replace(/\*$/, spec.slice(k.length - 1)));
        break;
      }
      if (k === spec) {
        resolved = path.posix.normalize(v);
        break;
      }
    }
  if (!resolved) return false;
  const t = noExt(target);
  const r = noExt(resolved);
  return r === t || `${r}/index` === t;
}

function importsFrom(content, importer, target, aliases) {
  IMPORT_RE.lastIndex = 0;
  let m;
  while ((m = IMPORT_RE.exec(content || "")))
    if (specPointsAt(m[1] || m[2] || m[3], importer, target, aliases)) return true;
  return false;
}

function awaitedAt(lines, n) {
  const here = lines[n - 1] || "";
  const before = (lines[n - 2] || "").trimEnd();
  return /\bawait\b|\breturn\b|\byield\b|\.then\(|\.catch\(/.test(here) || /(\bawait|=>|=)$/.test(before);
}

export function makeRepo(repoDir, rev = null) {
  const git = (args) => execFileSync("git", ["-C", repoDir, ...args], { encoding: "utf8", maxBuffer: 64 << 20 });
  const cache = new Map();
  return {
    read(file) {
      if (!cache.has(file)) {
        let text = null;
        try {
          text = rev ? git(["show", `${rev}:${file}`]) : readFileSync(path.join(repoDir, file), "utf8");
        } catch {}
        cache.set(file, text);
      }
      return cache.get(file);
    },
    readBlob(sha) {
      try {
        return git(["cat-file", "-p", sha]);
      } catch {
        return null;
      }
    },
    filesMentioning(name) {
      let out = "";
      try {
        out = git(["grep", "-l", "-w", "-I", "-F", "-e", name, ...(rev ? [rev] : []), "--", "*.ts", "*.tsx", "*.js", "*.jsx", "*.mjs", "*.cjs"]);
      } catch {
        return [];
      }
      return out
        .split("\n")
        .filter(Boolean)
        .map((l) => (rev ? l.slice(rev.length + 1) : l));
    },
  };
}

// The references `importer` makes to `name` exported by `target`, found by
// binding rather than by word: only names bound to an import or require of
// `target` count, `mod.name(...)` through a namespace or module object counts as
// a call, and a file that re-exports the name is reported so its own importers
// can be followed. null when the file does not import `name` from `target` at
// all. With the reviewed repo's `typescript` parser; without one, blastRadius
// keeps the line-by-word scan below.
export function bindingRefs(ts, importer, content, target, name, aliases) {
  const kind = /\.tsx$/.test(importer) ? ts.ScriptKind.TSX : /\.jsx$/.test(importer) ? ts.ScriptKind.JSX : /\.(js|mjs|cjs)$/.test(importer) ? ts.ScriptKind.JS : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(importer, content, ts.ScriptTarget.Latest, true, kind);
  const points = (spec) => typeof spec === "string" && specPointsAt(spec, importer, target, aliases);
  const requireOf = (n) =>
    n && ts.isCallExpression(n) && ts.isIdentifier(n.expression) && n.expression.text === "require" && n.arguments[0] && ts.isStringLiteralLike(n.arguments[0]) && points(n.arguments[0].text);
  const locals = new Set();
  const modules = new Set();
  const declared = new Set();
  let reexports = false;
  const direct = [];
  const bindings = (n) => {
    if (ts.isImportDeclaration(n) && ts.isStringLiteralLike(n.moduleSpecifier) && points(n.moduleSpecifier.text) && n.importClause) {
      const nb = n.importClause.namedBindings;
      if (nb && ts.isNamespaceImport(nb)) modules.add(nb.name.text);
      if (nb && ts.isNamedImports(nb))
        for (const el of nb.elements)
          if ((el.propertyName || el.name).text === name) {
            locals.add(el.name.text);
            declared.add(el.name);
          }
    }
    if (ts.isExportDeclaration(n) && n.moduleSpecifier && ts.isStringLiteralLike(n.moduleSpecifier) && points(n.moduleSpecifier.text))
      if (!n.exportClause || (ts.isNamedExports(n.exportClause) && n.exportClause.elements.some((el) => (el.propertyName || el.name).text === name))) reexports = true;
    if (ts.isVariableDeclaration(n) && requireOf(n.initializer)) {
      if (ts.isIdentifier(n.name)) modules.add(n.name.text);
      if (ts.isObjectBindingPattern(n.name))
        for (const el of n.name.elements)
          if (ts.isIdentifier(el.name) && ((el.propertyName && ts.isIdentifier(el.propertyName) ? el.propertyName.text : el.name.text) === name)) {
            locals.add(el.name.text);
            declared.add(el.name);
          }
    }
    if (ts.isPropertyAccessExpression(n) && n.name.text === name && requireOf(n.expression)) direct.push(n);
    ts.forEachChild(n, bindings);
  };
  bindings(sf);
  if (!locals.size && !modules.size && !direct.length && !reexports) return null;
  const sites = [...direct];
  const refs = (n) => {
    if (ts.isIdentifier(n) && locals.has(n.text) && !declared.has(n) && !(n.parent && ts.isPropertyAccessExpression(n.parent) && n.parent.name === n)) sites.push(n);
    if (ts.isPropertyAccessExpression(n) && n.name.text === name && ts.isIdentifier(n.expression) && modules.has(n.expression.text)) sites.push(n);
    ts.forEachChild(n, refs);
  };
  refs(sf);
  const isFn = (m) => ts.isFunctionDeclaration(m) || ts.isFunctionExpression(m) || ts.isArrowFunction(m) || ts.isMethodDeclaration(m);
  const lines = content.split("\n");
  const out = sites.map((node) => {
    const call = Boolean(node.parent && ts.isCallExpression(node.parent) && node.parent.expression === node);
    const site = call ? node.parent : node;
    const p = site.parent;
    const awaited = call
      ? Boolean(p && (ts.isAwaitExpression(p) || ts.isReturnStatement(p) || (ts.isArrowFunction(p) && p.body === site) || (ts.isPropertyAccessExpression(p) && ["then", "catch", "finally"].includes(p.name.text)) || ts.isVariableDeclaration(p) || ts.isCallExpression(p)))
      : null;
    let inTry = false;
    for (let m = site.parent; m && !isFn(m); m = m.parent) if (ts.isTryStatement(m) && m.tryBlock.pos <= site.pos && site.end <= m.tryBlock.end) inTry = true;
    const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
    return { line, call, awaited, inTry: call ? inTry : null, text: (lines[line - 1] || "").trim().slice(0, TEXT_CHARS) };
  });
  return { refs: out, reexports };
}

// Does `name`, declared in this file, return a promise? Only the outer function
// counts: `(roles) => async (req, res) => {}` is a factory that returns a
// middleware synchronously.
export function returnsPromise(ts, file, content, name) {
  const kind = /\.tsx$/.test(file) ? ts.ScriptKind.TSX : /\.jsx$/.test(file) ? ts.ScriptKind.JSX : /\.(js|mjs|cjs)$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(file, content, ts.ScriptTarget.Latest, true, kind);
  let fn = null;
  const visit = (n) => {
    if (fn) return;
    if (ts.isFunctionDeclaration(n) && n.name && n.name.text === name) fn = n;
    else if (ts.isVariableDeclaration(n) && ts.isIdentifier(n.name) && n.name.text === name && n.initializer && (ts.isArrowFunction(n.initializer) || ts.isFunctionExpression(n.initializer))) fn = n.initializer;
    else if (ts.isBinaryExpression(n) && n.operatorToken.kind === ts.SyntaxKind.EqualsToken && /^(?:module\.)?exports\./.test(n.left.getText(sf)) && n.left.getText(sf).endsWith(`.${name}`) && (ts.isArrowFunction(n.right) || ts.isFunctionExpression(n.right))) fn = n.right;
    else ts.forEachChild(n, visit);
  };
  visit(sf);
  if (!fn) return null;
  if ((ts.getCombinedModifierFlags(fn) & ts.ModifierFlags.Async) !== 0) return true;
  if (fn.type) return /^Promise\b/.test(fn.type.getText(sf));
  // No annotation: false only when every value it returns is plainly not a
  // promise (a function, a literal, an object or array, or nothing). A returned
  // call may be a promise, as in `(input) => queue.add(name, input)` on BE #2167,
  // whose unawaited caller is the defect: that stays unknown (null).
  const plain = (e) =>
    !e ||
    ts.isArrowFunction(e) ||
    ts.isFunctionExpression(e) ||
    ts.isObjectLiteralExpression(e) ||
    ts.isArrayLiteralExpression(e) ||
    ts.isLiteralExpression(e) ||
    ts.isTemplateExpression(e) ||
    [ts.SyntaxKind.TrueKeyword, ts.SyntaxKind.FalseKeyword, ts.SyntaxKind.NullKeyword].includes(e.kind) ||
    ts.isBinaryExpression(e) ||
    ts.isPrefixUnaryExpression(e);
  if (!ts.isBlock(fn.body)) return plain(fn.body) ? false : null;
  const returned = [];
  const walk = (m) => {
    if (m !== fn.body && (ts.isArrowFunction(m) || ts.isFunctionExpression(m) || ts.isFunctionDeclaration(m))) return;
    if (ts.isReturnStatement(m)) returned.push(m.expression);
    ts.forEachChild(m, walk);
  };
  walk(fn.body);
  return returned.every(plain) ? false : null;
}

const HANDLER_PATH = /(^|\/)(controllers?|routes?|jobs?|workers?|webhooks?|middlewares?|_middleware)\//;

export function blastRadius(diff, repo, { max = MAX_CALLERS, ts = null } = {}) {
  const changed = changedLines(diff);
  const inPr = new Set(changed.keys());
  const aliases = readAliases(repo.read("tsconfig.json"));
  const out = { changed: [], callers: [], total: 0, truncated: false };
  const seen = new Set();
  const callers = [];

  // Every file that imports `name` from `file`, with its declarations.
  const importers = (name, file) =>
    repo
      .filesMentioning(name)
      .filter((g) => g !== file && SOURCE_RE.test(g) && !TEST_RE.test(g))
      .map((g) => ({ file: g, content: repo.read(g) }))
      .filter((g) => g.content && importsFrom(g.content, g.file, file, aliases));

  // With a parser: the importers that bind `name` from `file`, with their
  // reference sites; a re-exporting barrel is followed one level.
  const boundImporters = (name, file) => {
    const out = [];
    const targets = [file];
    for (let i = 0; i < targets.length && i < 4; i++) {
      for (const g of repo.filesMentioning(name)) {
        if (g === targets[i] || !SOURCE_RE.test(g) || TEST_RE.test(g)) continue;
        const content = repo.read(g);
        if (!content) continue;
        let r = null;
        try {
          r = bindingRefs(ts, g, content, targets[i], name, aliases);
        } catch {}
        if (!r) continue;
        if (r.reexports && !targets.includes(g)) targets.push(g);
        if (r.refs.length) out.push({ file: g, content, refs: r.refs });
      }
    }
    return out;
  };
  // `sync`: the changed declaration returns no promise, so whether a caller
  // awaits it says nothing (on BE #2120, 25 route registrations of a middleware
  // factory took every slot as "NOT awaited").
  const recordRefs = (g, via, hop, sync = false) => {
    if (inPr.has(g.file)) return;
    for (const r of g.refs) {
      const key = `${g.file}:${r.line}`;
      if (seen.has(key)) continue;
      seen.add(key);
      callers.push({ file: g.file, line: r.line, via, hop, call: r.call, awaited: r.call && sync ? null : r.awaited, inTry: r.inTry, text: r.text });
    }
  };
  // The exported declarations of an importer that contain one of its references.
  const usingDecls = (g) =>
    declarations(g.content).filter((u) => u.exported && !isType(u) && g.refs.some((r) => r.line >= u.at && r.line <= u.end));

  const record = (g, name, via, hop) => {
    if (inPr.has(g.file)) return;
    const lines = g.content.split("\n");
    const skip = importLines(lines);
    const re = wordRe(name);
    const callRe = new RegExp(`(^|[^\\w$.])${name.replace(/\$/g, "\\$")}\\s*(\\?\\.)?\\(`);
    lines.forEach((l, i) => {
      if (skip.has(i + 1) || !re.test(l) || COMMENT_LINE.test(l)) return;
      const key = `${g.file}:${i + 1}`;
      if (seen.has(key)) return;
      seen.add(key);
      const call = callRe.test(l);
      callers.push({
        file: g.file,
        line: i + 1,
        via,
        hop,
        call,
        awaited: call ? awaitedAt(lines, i + 1) : null,
        text: l.trim().slice(0, TEXT_CHARS),
      });
    });
  };

  const second = new Map();
  for (const [file, lines] of changed) {
    if (!SOURCE_RE.test(file) || TEST_RE.test(file)) continue;
    const content = repo.read(file);
    if (!content) continue;
    const decls = declarations(content);
    const hit = decls.filter((d) => !isType(d) && [...lines].some((n) => n >= d.start && n <= d.end));
    if (!hit.length) continue;
    out.changed.push({ file, symbols: hit.map((d) => d.name) });
    for (const d of hit) {
      const re = wordRe(d.name);
      for (const u of decls)
        if (u !== d && u.exported && !isType(u) && re.test(u.body) && !hit.includes(u))
          second.set(`${file}#${u.exportAs}`, { file, name: u.exportAs, via: `${d.name} -> ${u.exportAs}` });
      if (!d.exported) continue;
      if (ts) {
        let sync = false;
        try {
          sync = returnsPromise(ts, file, content, d.name) === false;
        } catch {}
        for (const g of boundImporters(d.exportAs, file)) {
          recordRefs(g, d.exportAs, 1, sync);
          for (const u of usingDecls(g))
            second.set(`${g.file}#${u.exportAs}`, { file: g.file, name: u.exportAs, via: `${d.exportAs} -> ${u.exportAs}` });
        }
        continue;
      }
      const usedAs = wordRe(d.exportAs);
      for (const g of importers(d.exportAs, file)) {
        record(g, d.exportAs, d.exportAs, 1);
        for (const u of declarations(g.content))
          if (u.exported && !isType(u) && usedAs.test(u.body))
            second.set(`${g.file}#${u.exportAs}`, { file: g.file, name: u.exportAs, via: `${d.exportAs} -> ${u.exportAs}` });
      }
    }
  }
  for (const s of second.values())
    if (ts) {
      let sync = false;
      try {
        sync = returnsPromise(ts, s.file, repo.read(s.file) || "", s.name) === false;
      } catch {}
      for (const g of boundImporters(s.name, s.file)) recordRefs(g, s.via, 2, sync);
    }
    else for (const g of importers(s.name, s.file)) record(g, s.name, s.via, 2);

  // A call nobody awaits is where a changed failure mode lands unhandled, so
  // those lead, and a call outside any try before one inside it; plain
  // references (a queue listed in an array) come last. Within a rank, a caller
  // in a controller, route, job, worker or middleware comes first: that is
  // where a request or a job breaks.
  const rank = (c) => (c.call ? (c.awaited === false ? 0 : 2) + (c.inTry ? 1 : 0) : 4);
  const onPath = (c) => (HANDLER_PATH.test(c.file) ? 0 : 1);
  callers.sort((a, b) => rank(a) - rank(b) || a.hop - b.hop || onPath(a) - onPath(b) || a.file.localeCompare(b.file) || a.line - b.line);
  // Within a rank, take one caller of each changed symbol before a second of
  // any, so one widely used symbol cannot fill every slot.
  const nth = new Map();
  const turn = callers.map((c) => {
    const k = `${rank(c)}|${c.via}`;
    nth.set(k, (nth.get(k) || 0) + 1);
    return nth.get(k);
  });
  const order = callers.map((c, i) => ({ c, i, t: turn[i], r: rank(c) }));
  order.sort((a, b) => a.r - b.r || a.t - b.t || a.i - b.i);
  callers.splice(0, callers.length, ...order.map((x) => x.c));
  out.total = callers.length;
  out.truncated = callers.length > max;
  out.callers = callers.slice(0, max).map(({ hop, ...c }) => c);
  return out;
}

function main() {
  const args = process.argv.slice(2);
  const revAt = args.indexOf("--rev");
  const rev = revAt >= 0 ? args.splice(revAt, 2)[1] : null;
  const [diffFile, repoDir] = args;
  if (!diffFile || !repoDir) {
    process.stderr.write("usage: blast-radius.mjs <diff-file> <repo-dir> [--rev <sha>]\n");
    process.exit(2);
  }
  let diff;
  try {
    diff = readFileSync(diffFile, "utf8");
  } catch (e) {
    process.stdout.write(`FATAL: cannot read the diff at ${diffFile}: ${e.message}\n`);
    process.exit(1);
  }
  const repo = makeRepo(repoDir, rev);
  // The pinned checkout has no node_modules; the clone the run started in does.
  const ts = loadTypescript([repoDir, process.cwd()]);
  const out = blastRadius(diff, repo, { ts });
  out.resolver = ts ? "bindings" : "words";
  out.attention = attentionFacts(diff, repo, ts);
  process.stdout.write(relayLines(out));
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();
if (isMain) main();
