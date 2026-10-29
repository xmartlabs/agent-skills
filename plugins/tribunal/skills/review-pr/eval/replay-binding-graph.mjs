#!/usr/bin/env node
// replay-binding-graph.mjs — lib/blast-radius.mjs with a parser: callers found
// by binding, not by word, and ranked by what a changed failure mode would hit.
//
// The word scan listed a file that merely shared a name with the changed
// symbol, counted `mod.fn()` as a plain reference, and let one widely used
// symbol fill every slot (BE #2120: 25 route registrations of a middleware
// factory pushed out the two callers of the changed account lookup). Each form
// of import has a case here, as does each ranking rule.
//
// Like replay-attention-facts.mjs it needs TRIBUNAL_TS_FROM (a directory whose
// node_modules holds typescript) and fails without one.
//
// Usage: TRIBUNAL_TS_FROM=<repo> node eval/replay-binding-graph.mjs

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { lib, WORKFLOW } from "./paths.mjs";
import { loadRegion } from "./extract-pure-region.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { blastRadius, makeRepo, returnsPromise } = await import(lib("blast-radius.mjs"));
const { loadTypescript } = await import(lib("attention-facts.mjs"));
let top = null;
try {
  top = execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
} catch {}
const ts = loadTypescript([process.env.TRIBUNAL_TS_FROM, process.cwd(), top]);
check("a typescript package to parse with was found", !!ts, "set TRIBUNAL_TS_FROM to a directory whose node_modules has typescript");
if (!ts) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}

const auth = (body) => `export const getAccount = async (id) => {
  return db.get(id, ${body});
};

export const makeMw = (roles) => async (req, res, next) => {
  if (!roles.includes(req.role)) return res.status(${body});
  next();
};
`;
const BASE = {
  "tsconfig.json": `{ "compilerOptions": { "paths": { "@services/*": ["./src/services/*"] } } }\n`,
  "src/services/auth.ts": auth("1"),
  "src/services/index.ts": `export { getAccount } from './auth';\n`,
  "src/controllers/aliased.ts": `import { getAccount as fetchAccount } from '@services/auth';

export async function handle(req) {
  const a = await fetchAccount(req.id);
  try {
    fetchAccount(req.other);
  } catch (e) {}
  return a;
}
`,
  "src/controllers/namespace.ts": `import * as authSvc from '../services/auth';

export function poke(id) {
  authSvc.getAccount(id);
}
`,
  "src/legacy/destructured.js": `const {
  getAccount
} = require('../services/auth');

exports.run = (id) => getAccount(id);
`,
  "src/legacy/module.js": `const svc = require('../services/auth');

exports.go = (id) => {
  svc.getAccount(id);
};
`,
  "src/controllers/barrel.ts": `import { getAccount } from '../services';

export const viaBarrel = (id) => getAccount(id);
`,
  "src/other/collision.ts": `import { getAccount } from './elsewhere';

export const unrelated = () => getAccount(9);
`,
  "src/routes/r.ts": `import { makeMw } from '../services/auth';

router.get('/a', makeMw(['admin']), handler);
router.get('/b', makeMw(['user']), handler);
router.get('/c', makeMw(['user']), handler);
`,
};

const work = mkdtempSync(join(tmpdir(), "tribunal-bindings-"));
const git = (...a) => execFileSync("git", ["-C", work, ...a], { encoding: "utf8" });
const writeTree = (tree) => {
  for (const [f, text] of Object.entries(tree)) {
    mkdirSync(dirname(join(work, f)), { recursive: true });
    writeFileSync(join(work, f), text);
  }
};
try {
  git("init", "-q");
  git("config", "user.email", "t@t");
  git("config", "user.name", "t");
  writeTree(BASE);
  git("add", "-A");
  git("commit", "-qm", "base");
  writeTree({ "src/services/auth.ts": auth("2") });
  git("add", "-A");
  git("commit", "-qm", "head");
  const diff = git("diff", "HEAD~1", "HEAD");
  const repo = makeRepo(work);
  const br = blastRadius(diff, repo, { ts });
  const words = blastRadius(diff, repo);
  const at = (file, line) => br.callers.find((c) => c.file === file && c.line === line);

  console.log("\n--- every form of import is followed ---");
  check("a named import under an alias", at("src/controllers/aliased.ts", 4) && at("src/controllers/aliased.ts", 4).call === true, JSON.stringify(br.callers));
  check("a namespace import's member call is a call", at("src/controllers/namespace.ts", 4)?.call === true);
  check("a destructured require across lines", at("src/legacy/destructured.js", 5)?.call === true);
  check("a module object from require", at("src/legacy/module.js", 4)?.call === true);
  check("a barrel's re-export is followed to its importers", at("src/controllers/barrel.ts", 3)?.call === true);
  check("the import lines themselves are not listed", !at("src/controllers/aliased.ts", 1) && !at("src/legacy/destructured.js", 2));

  console.log("\n--- and only those ---");
  check("a file importing the same name from another module is not listed", !br.callers.some((c) => c.file === "src/other/collision.ts"));
  check("the word scan, for contrast, cannot see an aliased or namespace call", !words.callers.some((c) => c.file === "src/controllers/aliased.ts" && c.line === 4) || !words.callers.some((c) => c.file === "src/controllers/namespace.ts" && c.call));

  console.log("\n--- what each call site says ---");
  const awaitedCall = at("src/controllers/aliased.ts", 4);
  const bareCall = at("src/controllers/aliased.ts", 6);
  check("an awaited call is awaited, outside any try", awaitedCall?.awaited === true && awaitedCall.inTry === false);
  check("a bare call inside a try is not awaited and is inside the try", bareCall?.awaited === false && bareCall.inTry === true, JSON.stringify(bareCall));
  const factory = at("src/routes/r.ts", 3);
  check("a call to a factory that returns no promise carries no await verdict", factory?.call === true && factory.awaited === null, JSON.stringify(factory));

  console.log("\n--- ranking ---");
  const idx = (file, line) => br.callers.findIndex((c) => c.file === file && c.line === line);
  check("an unawaited call outside a try comes before everything else", br.callers[0].awaited === false && br.callers[0].inTry === false, JSON.stringify(br.callers[0]));
  check("an unawaited call inside a try comes after one outside", idx("src/controllers/namespace.ts", 4) < idx("src/controllers/aliased.ts", 6));
  check("a factory's route registrations do not outrank a real unawaited call", idx("src/routes/r.ts", 3) > idx("src/controllers/namespace.ts", 4));
  const firstTwoVias = new Set(br.callers.filter((c) => c.awaited !== false).slice(0, 2).map((c) => c.via));
  check("within a rank, one caller of each changed symbol comes before a second of any", firstTwoVias.size === 2, JSON.stringify(br.callers.map((c) => `${c.file}:${c.line} ${c.via}`)));
} finally {
  rmSync(work, { recursive: true, force: true });
}

console.log("\n--- does a declaration return a promise ---");
{
  const rp = (src, name = "f") => returnsPromise(ts, "x.ts", src, name);
  check("async is a promise", rp("export const f = async () => 1;") === true);
  check("a Promise return type is a promise", rp("export function f(): Promise<number> { return g(); }") === true);
  check("another return type is not", rp("export const f = (e): boolean => e.status === 404;") === false);
  check("a factory returning a middleware is not", rp("export const f = (roles) => async (req) => roles;") === false);
  check("a function returning another call is unknown", rp("export const f = (input) => queue.add('x', input);") === null);
  check("a void block is not", rp("export function f(x) { cache.delete(x); }") === false);
  check("a CommonJS export is found", returnsPromise(ts, "x.js", "exports.f = async (x) => x;", "f") === true);
  check("an unknown name is unknown", rp("export const g = 1;") === null);
}

console.log("\n--- the lens sees the try ---");
{
  const region = loadRegion(WORKFLOW, "DEPENDENTS-BLOCK", ["dependentsBlock"]);
  const block = region.api.dependentsBlock({
    changed: [{ file: "src/s.ts", symbols: ["f"] }],
    callers: [
      { file: "a.ts", line: 1, via: "f", call: true, awaited: false, inTry: false, text: "f()" },
      { file: "b.ts", line: 2, via: "f", call: true, awaited: false, inTry: true, text: "f()" },
      { file: "c.ts", line: 3, via: "f", call: true, awaited: false, text: "f()" },
    ],
    total: 3,
  });
  check("a call outside a try says so", block.includes("a.ts:1 [call, NOT awaited, no try around it]"));
  check("a call inside a try says so", block.includes("b.ts:2 [call, NOT awaited, inside a try]"));
  check("a caller from the word scan, with no try verdict, reads as before", block.includes("c.ts:3 [call, NOT awaited] via f"));
  check("the relay names the resolver it used", readFileSync(lib("blast-radius.mjs"), "utf8").includes('out.resolver = ts ? "bindings" : "words";'));
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall binding-graph expectations hold");
