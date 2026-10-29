#!/usr/bin/env node
// replay-attention-facts.mjs — lib/attention-facts.mjs on fixtures, each fact
// with a case that must fire and one that must not.
//
// The parser is the `typescript` package of a reviewed repo; this plugin ships
// none. Set TRIBUNAL_TS_FROM to any directory whose node_modules holds it (a
// TypeScript repo you review). Without it this replay fails rather than pass on
// nothing parsed.
//
// Usage: TRIBUNAL_TS_FROM=<repo> node eval/replay-attention-facts.mjs

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
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

const { addedLines, attentionFacts, baseBlobs, contractFacts, edgeFacts, factsForFile, loadTypescript, statusOrderFacts, testFacts } = await import(lib("attention-facts.mjs"));
let top = null;
try {
  top = execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
} catch {}
const ts = loadTypescript([process.env.TRIBUNAL_TS_FROM, process.cwd(), top]);
check(
  "a typescript package to parse with was found",
  !!ts,
  "set TRIBUNAL_TS_FROM to a directory whose node_modules has typescript (any TypeScript repo you review)",
);
if (!ts) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}

// Every line of the fixture counts as added unless a set is given.
const all = (src) => new Set(src.split("\n").map((_, i) => i + 1));
const facts = (src, added = all(src), file = "src/x.ts") => factsForFile(ts, file, src, added);
const kinds = (fs) => fs.map((f) => f.kind);

console.log("\n--- exit-before-finally (a backend upload handler, 2026-10) ---");
{
  const bad = `export async function upload(req, res) {
  const filesToClean = [];
  try {
    if (!req.ok) {
      return res.status(403).json({});
    }
    req.files.forEach((f) => filesToClean.push(f.path));
    return res.status(200).json({});
  } finally {
    cleanup(filesToClean);
  }
}`;
  const f = facts(bad, new Set([5]));
  check("an exit before the try fills what the finally reads fires", f.length === 1 && f[0].kind === "exit-before-finally" && f[0].line === 5 && /filesToClean/.test(f[0].detail), JSON.stringify(f));
  const after = facts(bad, new Set([8]));
  check("the same finally after the fill is quiet", !kinds(after).includes("exit-before-finally"), JSON.stringify(after));
  const setAbove = `export async function render(p) {
  const tmp = make(p);
  try {
    return await read(tmp);
  } finally {
    await rm(tmp);
  }
}`;
  check("a finally that reads only state set above the try is quiet (BE #2162)", facts(setAbove).length === 0, JSON.stringify(facts(setAbove)));
  check("an exit the PR did not add is quiet", facts(bad, new Set([2])).length === 0);
}

console.log("\n--- unawaited-enqueue (BE #2167) ---");
{
  const src = `export async function hook(req, res) {
  addSyncJob(req.body);
  await addOtherJob(req.body);
  return enqueueThing(req.body);
}`;
  const f = facts(src);
  check("a call whose promise is dropped fires", f.some((x) => x.kind === "unawaited-enqueue" && x.line === 2), JSON.stringify(f));
  check("awaited and returned calls are quiet", !f.some((x) => x.kind === "unawaited-enqueue" && (x.line === 3 || x.line === 4)));
  check("a method on a queue object counts too", facts(`queue.addSyncJob(x);`).some((x) => x.kind === "unawaited-enqueue"));
  check("an unrelated call is quiet", !facts(`addNumbers(1, 2);`).some((x) => x.kind === "unawaited-enqueue"));
}

console.log("\n--- sentinel-from-catch (BE #1996) ---");
{
  const src = `const getByVid = async (vid) => {
  try {
    return await db.find(vid);
  } catch (e) {
    return null;
  }
};

export async function resolve(vid) {
  for (let i = 0; i < 3; i++) {
    const a = await getByVid(vid);
    if (!a) return null;
  }
  return null;
}`;
  const f = facts(src, new Set([11]));
  check("a new call to a function that returns its sentinel from a catch fires at the call", f.some((x) => x.kind === "sentinel-from-catch" && x.line === 11 && /getByVid/.test(x.detail) && /null/.test(x.detail)), JSON.stringify(f));
  const noCatch = `const get = async (vid) => db.find(vid);
const a = get(1);`;
  check("a callee with no catch is quiet", facts(noCatch).length === 0);
  const both = `export function parse(s) {
  if (!s) return false;
  try {
    return JSON.parse(s).ok;
  } catch {
    return false;
  }
}`;
  check("a changed function returning the same sentinel from a catch and a branch fires", facts(both).some((x) => x.kind === "sentinel-from-catch" && x.line === 6));
  const distinct = `export function parse(s) {
  if (!s) return null;
  try {
    return JSON.parse(s).ok;
  } catch {
    throw new Error("bad");
  }
}`;
  check("a catch that throws is quiet", facts(distinct).length === 0);
}

console.log("\n--- new-emit (BE #2119) ---");
{
  check("a socket emit with a literal event fires", facts(`io.to(room).emit("xls:done", { id });`).some((x) => x.kind === "new-emit" && /xls:done/.test(x.detail)));
  check("an emit the PR did not add is quiet", facts(`io.emit("a", 1);\nfoo();`, new Set([2])).length === 0);
}

console.log("\n--- payload and writer parity, jobs without a handler (BE #2119) ---");
{
  const files = {
    "src/services/results.ts": `export const createPending = async (input) => {
  await prisma.results.create({ data: { orderId: input.orderId, sheetId: input.sheetId, contactId: input.contactId ?? null, status: "pending" } });
  socketIO.emit(WEB_SOCKET.RESULT, { kind: "created", orderId: input.orderId, sheetId: input.sheetId });
};
export const createTerminal = async (input) => {
  await prisma.results.create({ data: { orderId: input.orderId, status: input.status } });
  socketIO.emit(WEB_SOCKET.RESULT, { kind: "created", orderId: input.orderId });
};
export const listResults = (input) => {
  const where = {};
  if (input.contactId) where.contactId = { contains: input.contactId };
  return prisma.results.findMany({ where });
};
`,
    "src/services/other.ts": `export const mark = () => {
  socketIO.emit(WEB_SOCKET.RESULT, { kind: "updated", orderId: 1, sheetId: 2 });
  return queue.add(ResultJobNames.SYNC, { id: 1 });
};
export const spread = () => socketIO.emit(WEB_SOCKET.RESULT, { ...base });
`,
    "src/jobs/worker.ts": `switch (job.name) {
  case ResultJobNames.SYNC:
    break;
}
`,
  };
  const repo = {
    read: (f) => files[f] ?? null,
    filesMentioning: (name) => Object.keys(files).filter((f) => new RegExp(`\\b${name}\\b`).test(files[f])),
  };
  const facts = edgeFacts(ts, repo, new Map([["src/services/results.ts", new Set([6, 7])]]));
  const payload = facts.find((f) => f.kind === "payload-parity");
  check("an emit missing a key every other emit of the event carries fires, naming the key", payload && payload.line === 7 && /without sheetId/.test(payload.detail) && /other 2 emit/.test(payload.detail), JSON.stringify(facts));
  check("an emit with a spread is neither judged nor counted against", payload && !/3 emit/.test(payload.detail));
  const writer = facts.find((f) => f.kind === "writer-parity");
  check("a created row missing fields every other writer sets fires, and names the ones readers filter on", writer && writer.line === 6 && /without sheetId, contactId/.test(writer.detail) && /readers filter on contactId/.test(writer.detail), JSON.stringify(writer));
  const quiet = edgeFacts(ts, repo, new Map([["src/services/results.ts", new Set([2, 3])]]));
  check("the complete writer and emit are quiet", quiet.length === 0, JSON.stringify(quiet));
  const jobs = edgeFacts(ts, repo, new Map([["src/services/other.ts", new Set([3])]]));
  check("a job a worker handles is quiet", !jobs.some((f) => f.kind === "job-without-handler"), JSON.stringify(jobs));
  files["src/jobs/worker.ts"] = "switch (job.name) {\n  case ResultJobNames.OTHER:\n}\n";
  const lost = edgeFacts(ts, repo, new Map([["src/services/other.ts", new Set([3])]]));
  check("a job no worker has a case for fires", lost.some((f) => f.kind === "job-without-handler" && /ResultJobNames\.SYNC/.test(f.detail)), JSON.stringify(lost));
}

console.log("\n--- a 401 added after a 403 (BE #2120) ---");
{
  const src = `module.exports = (req, res, next) => {
  if (!allowed(req)) {
    return res.status(403).json({ code: 403 });
  }
  if (account.isActive === 0) {
    return res.status(StatusCodes.UNAUTHORIZED).json({ code: 401 });
  }
  next();
};`;
  const f = statusOrderFacts(ts, "src/legacy/authorize.js", src, new Set([6]));
  check("the 401 that a 403 above it shadows fires", f.length === 1 && f[0].line === 6 && /403 at line 3/.test(f[0].detail), JSON.stringify(f));
  const first = src.replace("return res.status(403)", "return res.status(400)");
  check("no 403 before it, no fact", statusOrderFacts(ts, "a.js", first, new Set([6])).length === 0);
  check("a 401 the PR did not add is quiet", statusOrderFacts(ts, "a.js", src, new Set([3])).length === 0);
}

console.log("\n--- tests that cannot fail ---");
{
  const t = `import { vi } from "vitest";
vi.mock("./billing");
it("refunds the payment", async () => {
  await refund(1);
});
it("rejects a bad id", () => {
  expect(() => refund(-1)).toThrow();
});
it("uses a helper", () => {
  assertRefunded(1);
});`;
  const f = testFacts(ts, "src/services/billing.test.ts", t, new Set([2, 3, 6, 9]));
  check("a test with no assertion fires", f.some((x) => x.kind === "test-without-assertion" && x.line === 3), JSON.stringify(f));
  check("expect and an assert* helper count as assertions", !f.some((x) => x.kind === "test-without-assertion" && (x.line === 6 || x.line === 9)));
  check("mocking the module the file tests fires", f.some((x) => x.kind === "mocks-subject" && x.line === 2));
  check("mocking another module is quiet", testFacts(ts, "src/services/billing.test.ts", `vi.mock("./logger");`, new Set([1])).length === 0);
}

console.log("\n--- contract change against the base (BE #2117, #1996) ---");
{
  const base = `export const find = async (id) => {
  const r = await db.get(id);
  if (!r) throw new NotFound(id);
  return r;
};
export function sum(a, b) { return a + b; }
export const opt = (a) => a;
`;
  const head = `export const find = async (id) => {
  const r = await db.get(id);
  if (!r) return null;
  return r;
};
export function sum(a, b, c) { return a + b + c; }
export const opt = (a, b = 1) => a + b;
`;
  const f = contractFacts(ts, "src/x.ts", base, head);
  check("a helper that now returns null where it threw fires", f.some((x) => /find now returns null where the base version threw/.test(x.detail)), JSON.stringify(f));
  check("a new required parameter fires", f.some((x) => /sum now takes 3 parameter\(s\), 3 required; the base version took 2, 2 required/.test(x.detail)));
  check("a new optional parameter is quiet", !f.some((x) => /^opt /.test(x.detail)));
  const cjsBase = "function g(x) { return compute(x); }\nmodule.exports = { g };\n";
  const cjsHead = "function g(x) { try { return compute(x); } catch { return null; } }\nmodule.exports = { g };\n";
  check("a CommonJS export counts as exported, and an unchanged contract is quiet", contractFacts(ts, "a.js", cjsBase, cjsBase).length === 0 && Array.isArray(contractFacts(ts, "a.js", cjsBase, cjsHead)));
  const diff = "diff --git a/src/x.ts b/src/x.ts\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/x.ts\n+++ b/src/x.ts\ndiff --git a/src/n.ts b/src/n.ts\nnew file mode 100644\nindex 0000000..9a9a9a9\n--- /dev/null\n+++ b/src/n.ts\n";
  const blobs = baseBlobs(diff);
  check("the base blob comes from the index line, and a new file has none", blobs.get("src/x.ts") === "1a2b3c4" && !blobs.has("src/n.ts"));
}

console.log("\n--- sentinel noise ---");
{
  const voidFn = `export async function save(x) {
  if (!x) return;
  try {
    await db.put(x);
  } catch (e) {
    log(e);
    return;
  }
}`;
  check("a bare return in a void function is not a sentinel", facts(voidFn).length === 0, JSON.stringify(facts(voidFn)));
  const calls = `const get = async (id) => {
  try { return await db.get(id); } catch { return null; }
};
export async function a() { return get(1); }
export async function b() { return get(2); }`;
  const f = facts(calls, new Set([4, 5]));
  check("repeated new calls to the same hiding callee give one fact", f.filter((x) => x.kind === "sentinel-from-catch" && /calls get/.test(x.detail)).length === 1, JSON.stringify(f));
}

console.log("\n--- across a diff ---");
{
  const diff = `diff --git a/src/a.ts b/src/a.ts
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,2 +1,3 @@
 export function f() {
+  addSyncJob(1);
 }
diff --git a/src/a.test.ts b/src/a.test.ts
--- a/src/a.test.ts
+++ b/src/a.test.ts
@@ -1,1 +1,2 @@
+addSyncJob(2);
 x();
`;
  const lines = addedLines(diff);
  check("added lines are read per file, deleted ones never", [...lines.get("src/a.ts")].join() === "2");
  const repo = { read: (f) => (f === "src/a.ts" ? "export function f() {\n  addSyncJob(1);\n}\n" : "addSyncJob(2);\nx();\n") };
  const r = attentionFacts(diff, repo, ts);
  check("test files are skipped", r.facts.length === 1 && r.facts[0].file === "src/a.ts", JSON.stringify(r.facts));
  check("the parser is named in the result", /^typescript \d/.test(r.parser));
  check("no parser means no facts, said plainly", attentionFacts(diff, repo, null).parser === null && attentionFacts(diff, repo, null).facts.length === 0);
  const many = { read: () => Array.from({ length: 30 }, (_, i) => `addSyncJob(${i});`).join("\n") };
  const manyDiff = `diff --git a/src/m.ts b/src/m.ts\n--- a/src/m.ts\n+++ b/src/m.ts\n@@ -0,0 +1,30 @@\n${Array.from({ length: 30 }, (_, i) => `+addSyncJob(${i});`).join("\n")}\n`;
  const capped = attentionFacts(manyDiff, many, ts);
  check("the list is capped and says so", capped.facts.length === 12 && capped.total === 30 && capped.truncated === true);
  const mixedFile = Array.from({ length: 14 }, (_, i) => `const g${i} = async () => { try { return await q(); } catch { return null; } };\nexport const c${i} = () => g${i}();`).join("\n") + "\nexport const h = (req, res) => {\n  if (x) return res.status(403);\n  return res.status(401);\n};";
  const mixedLines = mixedFile.split("\n").length;
  const mixedDiff = `diff --git a/src/z.ts b/src/z.ts\n--- a/src/z.ts\n+++ b/src/z.ts\n@@ -0,0 +1,${mixedLines} @@\n${mixedFile.split("\n").map((l) => "+" + l).join("\n")}\n`;
  const ranked = attentionFacts(mixedDiff, { read: () => mixedFile }, ts);
  check("under the cap the stronger classes come first", ranked.truncated && ranked.facts[0].kind === "status-order", JSON.stringify(ranked.facts.map((f) => f.kind)));
}

console.log("\n--- the lens sees them ---");
{
  const region = loadRegion(WORKFLOW, "DEPENDENTS-BLOCK", ["dependentsBlock", "attentionBlock"]);
  check("the region stays pure", region.violations.length === 0, JSON.stringify(region.violations));
  const at = { facts: [{ kind: "unawaited-enqueue", file: "src/a.ts", line: 2, detail: "addSyncJob(...) is called and its promise is not awaited, returned or caught" }], total: 1, truncated: false };
  const onlyFacts = region.api.dependentsBlock({ changed: [], callers: [], total: 0, attention: at });
  check("facts render even with no callers", /<attention>/.test(onlyFacts) && !/<dependents>/.test(onlyFacts) && onlyFacts.includes("src/a.ts:2 [unawaited-enqueue]"));
  check("each fact is framed as a question, not a finding", /Question: if the queue rejects/.test(onlyFacts) && /not a finding/.test(onlyFacts));
  check("no facts, no block", region.api.dependentsBlock({ changed: [], callers: [], total: 0, attention: { facts: [] } }) === "");
  const src = readFileSync(lib("blast-radius.mjs"), "utf8");
  check("blast-radius attaches the facts to its relay, with the same parser it resolves bindings with", src.includes("const ts = loadTypescript([repoDir, process.cwd()]);") && src.includes("out.attention = attentionFacts(diff, repo, ts);") && src.includes("blastRadius(diff, repo, { ts })"));

  // The workflow calls dependentsBlock ABOVE the region's text, so anything the
  // region declares at its top level is still uninitialized at that call. A
  // top-level const here threw a ReferenceError on the first real PR with a fact
  // (BE #1996, 2026-10-21) while every check above, which loads the region on its
  // own, stayed green. Run it in the workflow's order.
  const wf = readFileSync(WORKFLOW, "utf8");
  const B = "// ===== BEGIN DEPENDENTS-BLOCK =====";
  const E = "// ===== END DEPENDENTS-BLOCK =====";
  const regionText = wf.slice(wf.indexOf(B), wf.indexOf(E));
  const callAt = wf.indexOf("const dependents = dependentsBlock(blastRadius);");
  check("the workflow calls dependentsBlock before the region's own text", callAt !== -1 && callAt < wf.indexOf(B));
  let inOrder = null;
  try {
    inOrder = new Function("BR", `const out = dependentsBlock(BR);\n${regionText}\nreturn out;`)({ changed: [], callers: [], total: 0, attention: at });
  } catch (e) {
    inOrder = e;
  }
  check("called in that order with a fact to show, it renders instead of throwing", typeof inOrder === "string" && inOrder.includes("<attention>"), inOrder instanceof Error ? inOrder.message : "");
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall attention-facts expectations hold");
