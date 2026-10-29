// attention-facts.mjs — lines a PR ADDS that match a defect class a lens has
// missed before, found by parsing, not by reading.
//
// Each fact is a question for the lens, never a finding. The classes are the
// ones the measured misses fell into, and each needed a reading the lens did
// not do on its own:
//
//   exit-before-finally   a return or throw added inside a try that has a
//                         finally: the finally may read state the new exit
//                         skipped (a backend upload handler, 2026-10: a 403 left the upload on
//                         disk).
//   unawaited-enqueue     an add*Job / enqueue call whose promise nobody
//                         awaits, returns or catches: a failing queue drops the
//                         job silently (BE #2167).
//   sentinel-from-catch   a function that returns the same null / false / []
//                         from a catch and from another branch: the caller
//                         cannot tell an error from an empty answer (BE #1996).
//   new-emit              a socket emit added: every consumer keys on fields
//                         the payload has to carry (BE #2119).
//
// And, comparing with the rest of the repo or with the base version:
//
//   payload-parity        an emit added without a key every other emit of the
//                         same event carries (BE #2119, the ping without sheetId).
//   writer-parity         a row created without a field every other writer of
//                         that model sets, noting which ones readers filter on
//                         (BE #2119, contactId).
//   job-without-handler   a job enqueued under a name no worker has a case for.
//   status-order          a 401 added after a 403 the same function answers
//                         first (BE #2120).
//   test-without-assertion, mocks-subject
//                         a test case that asserts nothing, or that mocks the
//                         module it tests (BE #2106, #2084).
//   contract-change       an exported helper that now returns a sentinel where
//                         it threw, throws where it returned one, or needs more
//                         arguments (BE #2117, #1996).
//
// The parser is the reviewed repo's own `typescript` package, used for syntax
// only (no program, no type check: milliseconds per file). A repo without it
// gets no facts, and says so.

import { createRequire } from "node:module";
import path from "node:path";

const SOURCE_RE = /\.(tsx?|jsx?|mjs|cjs)$/;
const TEST_RE = /(^|\/)(__tests__|__mocks__|tests?)\/|\.(test|spec)\.[^/]+$/;
const ENQUEUE_RE = /^(add\w*Job|enqueue\w*)$/;
const MAX_FACTS = 12;
const KIND_RANK = {
  "payload-parity": 0,
  "writer-parity": 0,
  "status-order": 0,
  "contract-change": 1,
  "exit-before-finally": 1,
  "job-without-handler": 1,
  "mocks-subject": 2,
  "test-without-assertion": 2,
  "unawaited-enqueue": 3,
  "new-emit": 4,
  "sentinel-from-catch": 5,
};

export function loadTypescript(dirs) {
  for (const dir of dirs.filter(Boolean)) {
    try {
      return createRequire(path.join(dir, "noop.js"))("typescript");
    } catch {}
  }
  return null;
}

// Added lines only, per file: a fact is about code the PR wrote.
export function addedLines(diff) {
  const out = new Map();
  let file = null;
  let line = 0;
  for (const raw of String(diff || "").split("\n")) {
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
    if (raw.startsWith("+")) out.get(file).add(line++);
    else if (raw.startsWith(" ")) line++;
  }
  return out;
}

const isFunctionLike = (ts, n) =>
  ts.isFunctionDeclaration(n) || ts.isFunctionExpression(n) || ts.isArrowFunction(n) || ts.isMethodDeclaration(n);

// Is `node` inside `ancestor` without crossing a function boundary?
function withinSameFunction(ts, node, ancestor) {
  for (let p = node.parent; p; p = p.parent) {
    if (p === ancestor) return true;
    if (isFunctionLike(ts, p)) return false;
  }
  return false;
}

function calleeName(ts, call) {
  const e = call.expression;
  if (ts.isIdentifier(e)) return e.text;
  if (ts.isPropertyAccessExpression(e)) return e.name.text;
  return "";
}

// A bare `return;` is how a void function ends early, not a sentinel: counting
// it flagged every void helper with a catch (four of the noisy corpus facts).
function sentinelOf(ts, expr) {
  if (!expr) return null;
  if (expr.kind === ts.SyntaxKind.NullKeyword) return "null";
  if (expr.kind === ts.SyntaxKind.FalseKeyword) return "false";
  if (ts.isArrayLiteralExpression(expr) && expr.elements.length === 0) return "[]";
  if (ts.isIdentifier(expr) && expr.text === "undefined") return "undefined";
  return null;
}

export function factsForFile(ts, file, content, added) {
  const kind = /\.tsx$/.test(file) ? ts.ScriptKind.TSX : /\.jsx$/.test(file) ? ts.ScriptKind.JSX : /\.(js|mjs|cjs)$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(file, content, ts.ScriptTarget.Latest, true, kind);
  const lineOf = (n) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const facts = [];
  // Sentinel returns of a function body, split by whether a catch returns them.
  const sentinelReturns = (fn) => {
    const out = [];
    const walk = (m) => {
      if (m !== fn && isFunctionLike(ts, m)) return;
      if (ts.isReturnStatement(m)) {
        const s = sentinelOf(ts, m.expression);
        if (s) {
          let inCatch = false;
          for (let p = m.parent; p && p !== fn; p = p.parent) if (ts.isCatchClause(p)) inCatch = true;
          out.push({ s, inCatch, line: lineOf(m) });
        }
      }
      ts.forEachChild(m, walk);
    };
    walk(fn.body);
    return out;
  };
  // Functions this file declares by name, for the calls the PR adds to them.
  // One fact per callee: the first new call names it, the rest repeat it.
  const calledAlready = new Set();
  const declared = new Map();
  ts.forEachChild(sf, (s) => {
    if (ts.isFunctionDeclaration(s) && s.name && s.body) declared.set(s.name.text, s);
    if (ts.isVariableStatement(s))
      for (const d of s.declarationList.declarations)
        if (ts.isIdentifier(d.name) && d.initializer && (ts.isArrowFunction(d.initializer) || ts.isFunctionExpression(d.initializer)) && ts.isBlock(d.initializer.body))
          declared.set(d.name.text, d.initializer);
  });
  const visit = (n) => {
    if (ts.isTryStatement(n) && n.finallyBlock) {
      // Only an exit that comes BEFORE the try fills something the finally
      // reads: on BE #2162 the finally removed a path set above the try, and a
      // fact there was noise on a clean PR.
      const readInFinally = new Set();
      const collect = (m) => {
        if (ts.isIdentifier(m)) readInFinally.add(m.text);
        ts.forEachChild(m, collect);
      };
      collect(n.finallyBlock);
      const fills = [];
      // Callbacks included: that handler filled its list in `files.forEach((f) => list.push(f))`.
      const findFills = (m) => {
        if (ts.isBinaryExpression(m) && m.operatorToken.kind === ts.SyntaxKind.EqualsToken && ts.isIdentifier(m.left) && readInFinally.has(m.left.text))
          fills.push({ name: m.left.text, pos: m.getStart(sf) });
        if (
          ts.isCallExpression(m) &&
          ts.isPropertyAccessExpression(m.expression) &&
          ["push", "add", "set", "unshift"].includes(m.expression.name.text) &&
          ts.isIdentifier(m.expression.expression) &&
          readInFinally.has(m.expression.expression.text)
        )
          fills.push({ name: m.expression.expression.text, pos: m.getStart(sf) });
        ts.forEachChild(m, findFills);
      };
      findFills(n.tryBlock);
      const walk = (m) => {
        if (isFunctionLike(ts, m)) return;
        if ((ts.isReturnStatement(m) || ts.isThrowStatement(m)) && added.has(lineOf(m))) {
          const later = [...new Set(fills.filter((f) => f.pos > m.getStart(sf)).map((f) => f.name))];
          if (later.length)
            facts.push({
              kind: "exit-before-finally",
              file,
              line: lineOf(m),
              detail: `a ${ts.isReturnStatement(m) ? "return" : "throw"} added inside a try whose finally (line ${lineOf(n.finallyBlock)}) reads ${later.join(", ")}, which the try fills only after this line`,
            });
        }
        ts.forEachChild(m, walk);
      };
      walk(n.tryBlock);
    }
    // BE #1996: a new loop called getAccountByExternalId, whose catch returns null on
    // any database error, and read that null as "no account".
    if (ts.isCallExpression(n) && ts.isIdentifier(n.expression) && added.has(lineOf(n)) && declared.has(n.expression.text)) {
      const fn = declared.get(n.expression.text);
      const inside = n.pos >= fn.pos && n.end <= fn.end;
      const hidden = inside || calledAlready.has(n.expression.text) ? null : sentinelReturns(fn).find((r) => r.inCatch);
      if (hidden) calledAlready.add(n.expression.text);
      if (hidden)
        facts.push({
          kind: "sentinel-from-catch",
          file,
          line: lineOf(n),
          detail: `calls ${n.expression.text}, which returns ${hidden.s} from a catch (line ${hidden.line}), so an error there arrives here as ${hidden.s}`,
        });
    }
    if (ts.isCallExpression(n) && ENQUEUE_RE.test(calleeName(ts, n)) && added.has(lineOf(n)) && ts.isExpressionStatement(n.parent))
      facts.push({ kind: "unawaited-enqueue", file, line: lineOf(n), detail: `${calleeName(ts, n)}(...) is called and its promise is not awaited, returned or caught` });
    if (
      ts.isCallExpression(n) &&
      ts.isPropertyAccessExpression(n.expression) &&
      n.expression.name.text === "emit" &&
      n.arguments[0] &&
      ts.isStringLiteralLike(n.arguments[0]) &&
      added.has(lineOf(n))
    )
      facts.push({ kind: "new-emit", file, line: lineOf(n), detail: `emits "${n.arguments[0].text}"` });
    if (isFunctionLike(ts, n) && n.body && ts.isBlock(n.body)) {
      const start = lineOf(n);
      const end = sf.getLineAndCharacterOfPosition(n.end).line + 1;
      if ([...added].some((l) => l >= start && l <= end)) {
        const returns = [];
        const walk = (m) => {
          if (m !== n && isFunctionLike(ts, m)) return;
          if (ts.isReturnStatement(m)) {
            const s = sentinelOf(ts, m.expression);
            if (s) {
              let inCatch = false;
              for (let p = m.parent; p && p !== n; p = p.parent) if (ts.isCatchClause(p) && withinSameFunction(ts, m, n)) inCatch = true;
              returns.push({ s, inCatch, line: lineOf(m) });
            }
          }
          ts.forEachChild(m, walk);
        };
        walk(n.body);
        for (const s of new Set(returns.filter((r) => r.inCatch).map((r) => r.s))) {
          const other = returns.find((r) => !r.inCatch && r.s === s);
          const fromCatch = returns.find((r) => r.inCatch && r.s === s);
          if (other)
            facts.push({
              kind: "sentinel-from-catch",
              file,
              line: fromCatch.line,
              detail: `returns ${s} from a catch (line ${fromCatch.line}) and from another branch (line ${other.line})`,
            });
        }
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return facts;
}

const parse = (ts, file, content) => {
  const kind = /\.tsx$/.test(file) ? ts.ScriptKind.TSX : /\.jsx$/.test(file) ? ts.ScriptKind.JSX : /\.(js|mjs|cjs)$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS;
  return ts.createSourceFile(file, content, ts.ScriptTarget.Latest, true, kind);
};
const textOf = (sf, n) => n.getText(sf).replace(/\s+/g, "");
// The word a repo-wide grep can find: a literal's text, or the last name of
// `WEB_SOCKET.INVOICE_RESULT`.
function tokenOf(ts, n) {
  if (ts.isStringLiteralLike(n)) return n.text;
  if (ts.isPropertyAccessExpression(n)) return n.name.text;
  if (ts.isIdentifier(n)) return n.text;
  return null;
}
// Keys of an object literal; `open` when a spread can add keys nobody can list.
function objectKeys(ts, obj) {
  if (!obj || !ts.isObjectLiteralExpression(obj)) return null;
  const keys = [];
  let open = false;
  for (const p of obj.properties) {
    if (ts.isSpreadAssignment(p)) open = true;
    else if (p.name && (ts.isIdentifier(p.name) || ts.isStringLiteralLike(p.name))) keys.push(p.name.text);
  }
  return { keys, open };
}
const PRISMA_WRITES = new Set(["create", "createMany", "upsert"]);

// What a file emits, which rows it creates, which jobs it enqueues and which
// job names its workers handle. Every edge here is one the import graph cannot
// see: the producer and the consumer share a string, not a symbol.
export function edgesForFile(ts, file, content) {
  const sf = parse(ts, file, content);
  const lineOf = (n) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const out = { emits: [], writers: [], enqueues: [], cases: [] };
  const visit = (n) => {
    if (ts.isCaseClause(n)) out.cases.push({ text: textOf(sf, n.expression), line: lineOf(n) });
    if (ts.isCallExpression(n) && ts.isPropertyAccessExpression(n.expression)) {
      const name = n.expression.name.text;
      const [a0, a1] = n.arguments;
      if (name === "emit" && a0 && a1) {
        const k = objectKeys(ts, a1);
        const token = tokenOf(ts, a0);
        if (k && token) out.emits.push({ file, line: lineOf(n), event: textOf(sf, a0), token, ...k });
      }
      if (PRISMA_WRITES.has(name) && ts.isPropertyAccessExpression(n.expression.expression) && a0 && ts.isObjectLiteralExpression(a0)) {
        const model = n.expression.expression.name.text;
        const prop = a0.properties.find((p) => p.name && ts.isIdentifier(p.name) && p.name.text === (name === "upsert" ? "create" : "data"));
        let data = prop && ts.isPropertyAssignment(prop) ? prop.initializer : null;
        if (data && ts.isArrayLiteralExpression(data)) data = data.elements[0] || null;
        const k = objectKeys(ts, data);
        if (k) out.writers.push({ file, line: lineOf(n), model, op: name, ...k });
      }
      if (name === "add" && a0 && n.arguments.length >= 2 && (ts.isPropertyAccessExpression(a0) || ts.isStringLiteralLike(a0))) {
        const token = tokenOf(ts, a0);
        if (token) out.enqueues.push({ file, line: lineOf(n), job: textOf(sf, a0), token });
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

const STATUS_401 = /^(401|StatusCodes\.UNAUTHORIZED|HttpStatus\.UNAUTHORIZED)$/;
const STATUS_403 = /^(403|StatusCodes\.FORBIDDEN|HttpStatus\.FORBIDDEN)$/;
// 401 and 403 answers in one function body, in source order.
function authAnswers(ts, sf, fn) {
  const out = [];
  const lineOf = (n) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const walk = (m) => {
    if (m !== fn && isFunctionLike(ts, m)) return;
    let code = null;
    if (ts.isCallExpression(m) && ts.isPropertyAccessExpression(m.expression) && ["status", "sendStatus"].includes(m.expression.name.text) && m.arguments[0]) {
      const t = textOf(sf, m.arguments[0]);
      code = STATUS_401.test(t) ? 401 : STATUS_403.test(t) ? 403 : null;
    }
    if (ts.isNewExpression(m) && ts.isIdentifier(m.expression)) {
      const t = m.expression.text;
      code = /Unauthori[sz]ed/.test(t) ? 401 : /Forbidden/.test(t) ? 403 : null;
    }
    if (code) out.push({ code, pos: m.getStart(sf), line: lineOf(m) });
    ts.forEachChild(m, walk);
  };
  walk(fn.body);
  return out;
}

// A 401 the PR adds after a 403 the same function already answers: a request
// that fails both checks gets the 403 (BE #2120, the legacy authorize
// middleware, where the frontend only logs out on a 401).
export function statusOrderFacts(ts, file, content, added) {
  const sf = parse(ts, file, content);
  const facts = [];
  const visit = (n) => {
    if (isFunctionLike(ts, n) && n.body && ts.isBlock(n.body)) {
      const answers = authAnswers(ts, sf, n);
      for (const a of answers.filter((x) => x.code === 401 && added.has(x.line))) {
        const before = answers.find((x) => x.code === 403 && x.pos < a.pos);
        if (before)
          facts.push({ kind: "status-order", file, line: a.line, detail: `a 401 added here comes after the 403 at line ${before.line} in the same function, so a request that fails both checks gets the 403` });
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return facts;
}

const ASSERTION_RE = /\b(expect|assert\w*|expectTypeOf)\s*\(|\.should\b|toMatch(Inline)?Snapshot/;
// Test cases the PR adds that cannot fail: no assertion at all, or a mock of
// the very module the file tests (BE #2106 and #2084 were the subtler forms).
export function testFacts(ts, file, content, added) {
  const sf = parse(ts, file, content);
  const lineOf = (n) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const subject = file.split("/").pop().replace(/\.(test|spec)\.[^.]+$/, "");
  const facts = [];
  const visit = (n) => {
    if (ts.isCallExpression(n) && added.has(lineOf(n))) {
      const callee = ts.isIdentifier(n.expression) ? n.expression.text : ts.isPropertyAccessExpression(n.expression) && ts.isIdentifier(n.expression.expression) ? n.expression.expression.text : "";
      const cb = n.arguments[n.arguments.length - 1];
      if (["it", "test"].includes(callee) && cb && isFunctionLike(ts, cb) && ts.isStringLiteralLike(n.arguments[0]) && !ASSERTION_RE.test(cb.getText(sf)))
        facts.push({ kind: "test-without-assertion", file, line: lineOf(n), detail: `the test "${n.arguments[0].text.slice(0, 80)}" asserts nothing, so it passes whatever the code does` });
      if (ts.isPropertyAccessExpression(n.expression) && ["vi", "jest"].includes(callee) && n.expression.name.text === "mock" && n.arguments[0] && ts.isStringLiteralLike(n.arguments[0])) {
        const spec = n.arguments[0].text;
        if (spec.split("/").pop().replace(/\.[^.]+$/, "") === subject)
          facts.push({ kind: "mocks-subject", file, line: lineOf(n), detail: `mocks "${spec}", the module this file tests, so its assertions run against the mock` });
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return facts;
}

// The blob each changed file had at the base, from the diff's `index` lines.
export function baseBlobs(diff) {
  const out = new Map();
  let blob = null;
  for (const raw of String(diff || "").split("\n")) {
    if (raw.startsWith("diff --git ")) blob = null;
    const m = raw.match(/^index ([0-9a-f]{7,40})\.\.[0-9a-f]{7,40}/);
    if (m) blob = /^0+$/.test(m[1]) ? null : m[1];
    if (raw.startsWith("+++ ") && blob) out.set(raw.slice(4).trim().replace(/^b\//, ""), blob);
  }
  return out;
}

// Exported functions of a file: name -> {params, sentinels, throws, line}.
export function exportedShapes(ts, file, content) {
  const sf = parse(ts, file, content);
  const lineOf = (n) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const isExported = (n) => (ts.getCombinedModifierFlags(n) & ts.ModifierFlags.Export) !== 0;
  const listed = new Set();
  const fns = new Map();
  const shape = (fn) => {
    const sentinels = new Set();
    let throws = 0;
    const walk = (m) => {
      if (m !== fn && isFunctionLike(ts, m)) return;
      if (ts.isReturnStatement(m)) {
        const s = sentinelOf(ts, m.expression);
        if (s && m.expression) sentinels.add(s);
      }
      if (ts.isThrowStatement(m)) throws++;
      ts.forEachChild(m, walk);
    };
    if (fn.body) walk(fn.body);
    const required = fn.parameters.filter((p) => !p.initializer && !p.questionToken && !p.dotDotDotToken).length;
    return { params: fn.parameters.length, required, sentinels: [...sentinels], throws, line: lineOf(fn) };
  };
  const all = new Map();
  ts.forEachChild(sf, (st) => {
    if (ts.isFunctionDeclaration(st) && st.name) {
      all.set(st.name.text, st);
      if (isExported(st)) fns.set(st.name.text, st);
    }
    if (ts.isVariableStatement(st))
      for (const d of st.declarationList.declarations)
        if (ts.isIdentifier(d.name) && d.initializer && (ts.isArrowFunction(d.initializer) || ts.isFunctionExpression(d.initializer))) {
          all.set(d.name.text, d.initializer);
          if (isExported(st)) fns.set(d.name.text, d.initializer);
        }
    if (ts.isExpressionStatement(st) && ts.isBinaryExpression(st.expression) && st.expression.operatorToken.kind === ts.SyntaxKind.EqualsToken) {
      const { left, right } = st.expression;
      const target = left.getText(sf);
      const m = target.match(/^(?:module\.)?exports\.([A-Za-z_$][\w$]*)$/);
      if (m && (ts.isArrowFunction(right) || ts.isFunctionExpression(right))) fns.set(m[1], right);
      if (target === "module.exports" && ts.isObjectLiteralExpression(right))
        for (const p of right.properties) if (ts.isShorthandPropertyAssignment(p)) listed.add(p.name.text);
    }
  });
  for (const name of listed) if (!fns.has(name) && all.has(name)) fns.set(name, all.get(name));
  return new Map([...fns].map(([k, fn]) => [k, shape(fn)]));
}

// An exported helper whose failure mode or arity the PR changed: its callers
// outside the diff were written against the old contract (BE #2117, #1996).
export function contractFacts(ts, file, baseContent, headContent) {
  const base = exportedShapes(ts, file, baseContent);
  const head = exportedShapes(ts, file, headContent);
  const facts = [];
  for (const [name, h] of head) {
    const b = base.get(name);
    if (!b) continue;
    const newSentinels = h.sentinels.filter((x) => !b.sentinels.includes(x));
    if (newSentinels.length && b.throws > h.throws)
      facts.push({ kind: "contract-change", file, line: h.line, detail: `${name} now returns ${newSentinels.join(" or ")} where the base version threw, so callers that relied on the throw get the value instead` });
    else if (h.required > b.required || h.params < b.params)
      facts.push({ kind: "contract-change", file, line: h.line, detail: `${name} now takes ${h.params} parameter(s), ${h.required} required; the base version took ${b.params}, ${b.required} required` });
    else if (h.throws > b.throws && b.sentinels.some((x) => !h.sentinels.includes(x)))
      facts.push({ kind: "contract-change", file, line: h.line, detail: `${name} now throws where the base version returned ${b.sentinels.filter((x) => !h.sentinels.includes(x)).join(" or ")}, so callers that checked for that value now get an exception` });
  }
  return facts;
}

const MAX_EDGE_FILES = 40;
// Repo-wide comparisons for the edges this PR adds: an emit missing a key every
// other emit of that event carries, a created row missing a field every other
// writer of that model sets, a job no worker handles.
export function edgeFacts(ts, repo, changed) {
  const cache = new Map();
  const edgesOf = (f) => {
    if (!cache.has(f)) {
      const c = repo.read(f);
      let e = null;
      try {
        e = c ? edgesForFile(ts, f, c) : null;
      } catch {}
      cache.set(f, e);
    }
    return cache.get(f);
  };
  const filesFor = (token, self) => {
    const found = typeof repo.filesMentioning === "function" ? repo.filesMentioning(token) : [];
    return [self, ...found.filter((f) => f !== self && SOURCE_RE.test(f) && !TEST_RE.test(f))].slice(0, MAX_EDGE_FILES);
  };
  const common = (others) => others.map((o) => new Set(o.keys)).reduce((a, b) => new Set([...a].filter((k) => b.has(k))));
  const facts = [];
  for (const [file, added] of changed) {
    const mine = edgesOf(file);
    if (!mine) continue;
    for (const e of mine.emits.filter((x) => added.has(x.line) && !x.open)) {
      const others = filesFor(e.token, file).flatMap((f) => (edgesOf(f) || { emits: [] }).emits).filter((o) => o.event === e.event && !o.open && !(o.file === file && o.line === e.line));
      if (!others.length) continue;
      const missing = [...common(others)].filter((k) => !e.keys.includes(k));
      if (missing.length)
        facts.push({ kind: "payload-parity", file, line: e.line, detail: `emits ${e.event} without ${missing.join(", ")}, which the other ${others.length} emit(s) of this event carry` });
    }
    for (const w of mine.writers.filter((x) => added.has(x.line) && !x.open)) {
      const files = filesFor(w.model, file);
      const others = files.flatMap((f) => (edgesOf(f) || { writers: [] }).writers).filter((o) => o.model === w.model && !o.open && !(o.file === file && o.line === w.line));
      if (!others.length) continue;
      const missing = [...common(others)].filter((k) => !w.keys.includes(k));
      if (!missing.length) continue;
      const texts = files.map((f) => repo.read(f) || "");
      const keyed = missing.filter((k) => texts.some((t) => new RegExp(`where\\.${k}\\b|where\\s*:\\s*\\{[^}]{0,300}\\b${k}\\b`).test(t)));
      facts.push({
        kind: "writer-parity",
        file,
        line: w.line,
        detail: `creates a ${w.model} row without ${missing.join(", ")}, which every other writer of this model sets${keyed.length ? `; readers filter on ${keyed.join(", ")}` : ""}`,
      });
    }
    for (const q of mine.enqueues.filter((x) => added.has(x.line))) {
      const handled = filesFor(q.token, file).some((f) => (edgesOf(f) || { cases: [] }).cases.some((c) => c.text === q.job));
      if (!handled) facts.push({ kind: "job-without-handler", file, line: q.line, detail: `enqueues ${q.job}, and no worker in the repo has a \`case ${q.job}:\`` });
    }
  }
  return facts;
}

export function attentionFacts(diff, repo, ts, { max = MAX_FACTS } = {}) {
  if (!ts) return { facts: [], total: 0, truncated: false, parser: null };
  const facts = [];
  const changed = new Map();
  const bases = baseBlobs(diff);
  for (const [file, added] of addedLines(diff)) {
    if (!added.size || !SOURCE_RE.test(file)) continue;
    const content = repo.read(file);
    if (!content) continue;
    try {
      if (TEST_RE.test(file)) {
        facts.push(...testFacts(ts, file, content, added));
        continue;
      }
      facts.push(...factsForFile(ts, file, content, added), ...statusOrderFacts(ts, file, content, added));
      changed.set(file, added);
      const base = bases.get(file) && typeof repo.readBlob === "function" ? repo.readBlob(bases.get(file)) : null;
      if (base) facts.push(...contractFacts(ts, file, base, content));
    } catch {}
  }
  try {
    facts.push(...edgeFacts(ts, repo, changed));
  } catch {}
  const seen = new Set();
  const unique = facts
    .filter((f) => {
      const k = `${f.kind}:${f.file}:${f.line}`;
      return seen.has(k) ? false : (seen.add(k), true);
    })
    // The cap keeps the classes that compare against the rest of the repo or the
    // base version first: they fired on labelled defects and on no clean PR in
    // the corpus, while sentinel-from-catch is the noisiest.
    .map((f, i) => ({ f, i }))
    .sort((a, b) => (KIND_RANK[a.f.kind] ?? 9) - (KIND_RANK[b.f.kind] ?? 9) || a.i - b.i)
    .map((x) => x.f);
  return { facts: unique.slice(0, max), total: unique.length, truncated: unique.length > max, parser: `typescript ${ts.version}` };
}
