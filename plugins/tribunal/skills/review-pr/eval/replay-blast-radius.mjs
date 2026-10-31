#!/usr/bin/env node
// replay-blast-radius.mjs — the unchanged callers lenses are handed.
//
// BE #2167 (2026-10-18): the PR moved the ledger service's queue onto a
// fail-fast Redis connection, and the unchanged controller that enqueues its webhooks
// without await kept answering 200. Lenses raised it in 2 of 10 runs, because
// nothing listed that caller and the methodology keeps caller checks in the
// same file. rust/.../js/blast-radius.mjs lists such callers; this pins what it lists,
// on a fixture repo built here with the #2167 shape, so the gate needs no BE
// checkout.
//
// Usage: node eval/replay-blast-radius.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { lib, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  let ok;
  try {
    ok = typeof cond === "function" ? cond() : cond;
  } catch (e) {
    ok = false;
    detail = `threw: ${e.message}`;
  }
  if (ok) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { blastRadius, makeRepo, changedLines, declarations, readAliases, specPointsAt, MAX_CALLERS } = await import(lib("../rust/crates/tribunal-context/src/js/blast-radius.mjs"));
const relay = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["checkRelay"]);
const block = loadRegion(WORKFLOW, "DEPENDENTS-BLOCK", ["dependentsBlock"]);

const BASE = {
  "tsconfig.json": `{
  // JSONC, like the BE's
  "compilerOptions": { "paths": { "@helpers/*": ["./src/helpers/*"], "@jobs/*": ["./src/jobs/*"] } },
  "include": ["src/**/*"],
}
`,
  "src/helpers/redis.ts": `export const redisConnection = { host: "h" };

export function unrelated() {
  return 1;
}
`,
  "src/jobs/queue.ts": `import { redisConnection } from '@helpers/redis';

export const webhookQueue = new Queue('w', {
  connection: redisConnection
});

export function addWebhookJob(input: unknown) {
  return webhookQueue.add('w', input);
}

export function addReportJob(input: unknown) {
  return reportQueue.add('r', input);
}

export type JobInput = { id: string };
`,
  "src/controllers/webhook.ts": `import { addWebhookJob } from '@jobs/queue';

export const webhook = (req, res) => {
  addWebhookJob(req.body);
  return res.status(200).json({});
};
`,
  "src/controllers/admin.ts": `import {
  addWebhookJob,
  addReportJob
} from '../jobs/queue';

export async function replay(req) {
  await addWebhookJob(req.body);
  await addReportJob(req.body);
}
`,
  "src/controllers/shadow.ts": `import { addWebhookJob } from './local-queue';

export function shadow() {
  addWebhookJob({});
}
`,
  "src/legacy/index.js": `const { addWebhookJob } = require('@jobs/queue');
// addWebhookJob is mentioned in this comment
module.exports = () => addWebhookJob({});
`,
  "src/jobs/queue.test.ts": `import { addWebhookJob } from './queue';
addWebhookJob({});
`,
  "src/tests/integration/server.ts": `import { addWebhookJob } from '@jobs/queue';
addWebhookJob({});
`,
  "src/scripts/report.ts": `import { webhookQueue } from '@jobs/queue';

export function queueReport() {
  return [webhookQueue];
}
`,
  "src/controllers/stats.ts": `import { queueReport } from '../scripts/report';

export async function stats() {
  return queueReport();
}
`,
  "src/types/use.ts": `import { JobInput } from '@jobs/queue';
export const x: JobInput = { id: "1" };
`,
};

// The PR: the queue moves to a new connection, and redis.ts gains it with a
// comment block that belongs to the new declaration, not to redisConnection.
const HEAD = {
  ...BASE,
  "src/helpers/redis.ts": `export const redisConnection = { host: "h" };

// Producer-side, fails fast.
// Two comment lines.
export const queueConnection = { ...redisConnection, enableOfflineQueue: false };

export function unrelated() {
  return 1;
}
`,
  "src/jobs/queue.ts": BASE["src/jobs/queue.ts"]
    .replace("import { redisConnection } from '@helpers/redis';", "import { queueConnection } from '@helpers/redis';")
    .replace("connection: redisConnection", "connection: queueConnection"),
};

const work = mkdtempSync(join(tmpdir(), "tribunal-blast-"));
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
  writeTree(HEAD);
  git("add", "-A");
  git("commit", "-qm", "head");
  const head = git("rev-parse", "HEAD").trim();
  const diff = git("diff", "HEAD~1", "HEAD");
  const diffFile = join(work, "pr.patch");
  writeFileSync(diffFile, diff);

  console.log("\n--- 1. parsing the diff and the declarations ---");
  const lines = changedLines(diff);
  check("both changed files are seen", lines.has("src/helpers/redis.ts") && lines.has("src/jobs/queue.ts"));
  const decls = declarations(HEAD["src/helpers/redis.ts"]);
  const qc = decls.find((d) => d.name === "queueConnection");
  check("a declaration owns the comment block above it", qc && qc.start === 3 && qc.at === 5, JSON.stringify(qc));
  check("the declaration above ends before that comment", decls.find((d) => d.name === "redisConnection").end === 2);
  const aliases = readAliases(BASE["tsconfig.json"]);
  check("tsconfig paths survive a glob that looks like a block comment", aliases["@jobs/*"] === "src/jobs/*", JSON.stringify(aliases));
  check("an alias resolves to the defining file", specPointsAt("@jobs/queue", "src/controllers/webhook.ts", "src/jobs/queue.ts", aliases));
  check("a relative specifier does too", specPointsAt("../jobs/queue", "src/controllers/admin.ts", "src/jobs/queue.ts", aliases));
  check("another module with the same export name does not", !specPointsAt("./local-queue", "src/controllers/shadow.ts", "src/jobs/queue.ts", aliases));

  console.log("\n--- 2. what it lists, on the #2167 shape ---");
  for (const [mode, repo] of [["worktree", makeRepo(work)], ["--rev", makeRepo(work, head)]]) {
    const r = blastRadius(diff, repo);
    const at = (f, l) => r.callers.find((c) => c.file === f && c.line === l);
    const files = new Set(r.callers.map((c) => c.file));
    check(`${mode}: the changed declarations are named`, () => {
      const by = Object.fromEntries(r.changed.map((c) => [c.file, c.symbols]));
      return by["src/helpers/redis.ts"].join() === "queueConnection" && by["src/jobs/queue.ts"].join() === "webhookQueue";
    }, JSON.stringify(r.changed));
    const fire = at("src/controllers/webhook.ts", 4);
    check(`${mode}: the unawaited caller two hops out is listed`, fire && fire.via === "webhookQueue -> addWebhookJob" && fire.call && fire.awaited === false, JSON.stringify(fire));
    check(`${mode}: and it comes first`, r.callers[0] === fire);
    check(`${mode}: an awaited caller is listed as awaited`, at("src/controllers/admin.ts", 7)?.awaited === true);
    const ref = at("src/scripts/report.ts", 4);
    check(`${mode}: a plain reference in an importer is listed, as a reference`, ref && ref.call === false && ref.awaited === null && ref.via === "webhookQueue", JSON.stringify(ref));
    check(`${mode}: references come after every call`, r.callers.findIndex((c) => !c.call) > r.callers.findLastIndex((c) => c.call));
    const viaImporter = at("src/controllers/stats.ts", 4);
    check(`${mode}: a caller of an importer's own declaration is listed`, viaImporter && viaImporter.via === "webhookQueue -> queueReport" && viaImporter.awaited === true, JSON.stringify(viaImporter));
    check(`${mode}: a declaration the PR did not reach is not followed`, !r.callers.some((c) => c.via.includes("addReportJob")));
    check(`${mode}: a multi-line import is not a call site`, !r.callers.some((c) => c.file === "src/controllers/admin.ts" && c.line < 5));
    check(`${mode}: a require line and a comment are not call sites`, () => {
      const legacy = r.callers.filter((c) => c.file === "src/legacy/index.js");
      return legacy.length === 1 && legacy[0].line === 3;
    });
    check(`${mode}: a same-named export from another module is not listed`, !files.has("src/controllers/shadow.ts"));
    check(`${mode}: tests are not listed`, !files.has("src/jobs/queue.test.ts") && !files.has("src/tests/integration/server.ts"));
    check(`${mode}: files in the PR are not listed`, !files.has("src/jobs/queue.ts") && !files.has("src/helpers/redis.ts"));
    check(`${mode}: types are left to the typecheck`, !files.has("src/types/use.ts"));
    check(`${mode}: the count is the whole list`, r.total === r.callers.length && r.truncated === false);
  }
  const capped = blastRadius(diff, makeRepo(work), { max: 1 });
  check("the cap keeps the first and says so", capped.callers.length === 1 && capped.truncated === true && capped.total > 1 && MAX_CALLERS === 25);

  console.log("\n--- 3. the CLI is a verified relay ---");
  // The CLI looks for `typescript` in the repo and then in its cwd. Run it from
  // the fixture, which has none, so the labels below do not depend on where the
  // battery is started: from a checkout with `typescript` installed they gained
  // ", no try around it" and section 4 failed.
  const cli = spawnSync(process.execPath, [lib("../rust/crates/tribunal-context/src/js/blast-radius.mjs"), diffFile, work, "--rev", head], { encoding: "utf8", cwd: work });
  const out = cli.stdout.trimEnd().split("\n");
  check("it prints a RELAY-CRC line and one JSON line", cli.status === 0 && out.length === 2 && out[0].startsWith("RELAY-CRC "), cli.stderr);
  check("a faithful copy verifies", () => relay.api.checkRelay({ relayCrc: out[0].slice(10), data: JSON.parse(out[1]) }).ok);

  console.log("\n--- 4. the block lenses get ---");
  check("the DEPENDENTS-BLOCK region is pure", block.violations.length === 0, JSON.stringify(block.violations));
  const text = block.api.dependentsBlock(JSON.parse(out[1]));
  check("each entry names the changed file it came from", text.includes("- src/controllers/webhook.ts:4 [call, NOT awaited] via webhookQueue -> addWebhookJob (changed in src/jobs/queue.ts): `addWebhookJob(req.body);`"), text);
  check("references are labelled as references", /src\/scripts\/report\.ts:4 \[reference\]/.test(text));
  check("it says how much of the list it shows", /This shows all \d+\./.test(text) && block.api.dependentsBlock({ ...JSON.parse(out[1]), truncated: true, total: 99 }).includes("the first"));
  check("it tells the lens where to anchor", /anchored at the changed declaration in the PR, never at the caller's file/.test(text));
  check("nothing to list is no block at all", block.api.dependentsBlock(null) === "" && block.api.dependentsBlock({ changed: [], callers: [], total: 0 }) === "");
  const { dependentsBlockFromWorkflow } = await import("../scripts/render-dependents.mjs");
  check(
    "the in-context renderer writes the same block from the built workflow",
    dependentsBlockFromWorkflow()(JSON.parse(out[1])) === text,
  );

  const bad = spawnSync(process.execPath, [lib("../rust/crates/tribunal-context/src/js/blast-radius.mjs"), join(work, "missing.patch"), work], { encoding: "utf8" });
  check("an unreadable diff is a FATAL, not an empty list", bad.status === 1 && /^FATAL: /.test(bad.stdout));
} finally {
  rmSync(work, { recursive: true, force: true });
}

// The legacy half of the BE exports with CommonJS. Until 2026-10-21 a changed
// legacy helper listed no callers: `module.exports = {...}` and `exports.x =`
// did not count as exports (BE `runWithConcurrencySettled`: 0 listed, 3 real).
console.log("\n--- 4b. CommonJS exports ---");
{
  const cjs = mkdtempSync(join(tmpdir(), "tribunal-blast-cjs-"));
  const g = (...a) => execFileSync("git", ["-C", cjs, ...a], { encoding: "utf8" });
  const write = (tree) => {
    for (const [f, text] of Object.entries(tree)) {
      mkdirSync(dirname(join(cjs, f)), { recursive: true });
      writeFileSync(join(cjs, f), text);
    }
  };
  const helper = (body) => `const LIMIT = 4;

async function runAll(items) {
  return items.map((x) => x * 2);
}

const local = (x) => {
  return ${body};
};

exports.notify = async (id) => {
  return ${body};
};

module.exports = {
  LIMIT,
  runAll,
  shared: local, // renamed on the way out
};
`;
  const before = {
    "src/legacy/helpers/bulk.js": helper("x + 1"),
    "src/legacy/controllers/orders.js": `const {
  runAll,
  shared
} = require('../helpers/bulk');

async function handle(req) {
  await runAll(req.items);
  return shared(req.id);
}
module.exports = { handle };
`,
    "src/legacy/controllers/hooks.js": `const bulk = require('../helpers/bulk');

exports.onEvent = (id) => {
  bulk.notify(id);
};
`,
    "src/legacy/controllers/other.js": `const { shared } = require('./elsewhere');
shared(1);
`,
  };
  try {
    g("init", "-q");
    g("config", "user.email", "t@t");
    g("config", "user.name", "t");
    write(before);
    g("add", "-A");
    g("commit", "-qm", "base");
    write({ "src/legacy/helpers/bulk.js": helper("x + 2") });
    g("add", "-A");
    g("commit", "-qm", "head");
    const r = blastRadius(g("diff", "HEAD~1", "HEAD"), makeRepo(cjs));
    const at = (file, line) => r.callers.find((c) => c.file === file && c.line === line);
    const decls = declarations(before["src/legacy/helpers/bulk.js"]);
    check("exports.name = counts as an exported declaration", decls.some((d) => d.name === "notify" && d.exported && d.kind === "cjs"));
    check("a name listed in module.exports = {...} is exported", decls.find((d) => d.name === "runAll").exported === true);
    check("a renamed entry exports the local under the outer name", (() => {
      const d = decls.find((x) => x.name === "local");
      return d.exported === true && d.exportAs === "shared";
    })());
    check("a declaration not listed stays unexported", decls.find((d) => d.name === "LIMIT").exportAs === "LIMIT" && decls.find((d) => d.name === "handle") === undefined);
    check("the changed local is found under its exported name", r.changed.some((c) => c.symbols.includes("local") && c.symbols.includes("notify")), JSON.stringify(r.changed));
    check("its caller through a multi-line require is listed as a call", (() => {
      const c = at("src/legacy/controllers/orders.js", 8);
      return c && c.call === true && c.via === "shared";
    })(), JSON.stringify(r.callers));
    check("the names inside that multi-line require are not listed", !at("src/legacy/controllers/orders.js", 2) && !at("src/legacy/controllers/orders.js", 3));
    check("exports.notify reaches a caller that went through the module object", !!at("src/legacy/controllers/hooks.js", 4));
    check("a file importing the same name from another module is not listed", !r.callers.some((c) => c.file === "src/legacy/controllers/other.js"));
    check("an unchanged export is not traced", !r.callers.some((c) => c.via === "runAll"));
  } finally {
    rmSync(cjs, { recursive: true, force: true });
  }
}

console.log("\n--- 5. the workflow is wired that way ---");
const src = readFileSync(WORKFLOW, "utf8");
const dispatch = src.slice(src.indexOf("const blastRelay ="), src.indexOf("const dependents = dependentsBlock(blastRadius);"));
check("it runs as a verified relay on the pinned checkout", dispatch.includes("relayAgent(") && dispatch.includes('rust/crates/tribunal-context/src/js/blast-radius.mjs "${diffPath}" "${headPin.path}"') && dispatch.includes("headPin.ok && diffMode === \"path\""));
check("an unverified relay sends no block", /const blastRadius = blastRelay && blastRelay\.ok \? blastRelay\.data : null;/.test(src));
check("the block sits in the shared lens prefix, above YOUR PASS", () => {
  const lens = src.slice(src.indexOf("Review PR #${p.prNumber} in repo ${p.repo}.\n\n<diff>"), src.indexOf("===== YOUR PASS ====="));
  const call = src.slice(src.indexOf("lensPrompt({"), src.indexOf("}),", src.indexOf("lensPrompt({")));
  return lens.includes("</context>\n${p.dependents ? `\\n${p.dependents}\\n` : \"\"}\n${LENS_BUDGET_INSTRUCTION}") && /\bdependents,/.test(call);
});
check("the row records the block and the dropped titles", /^\s*dependents: blastRelay$/m.test(src) && /droppedPrePanelItems: droppedPrePanel\.map/.test(src));
for (const m of ["code-review-methodology.md", "fe-code-review-methodology.md", "e2e-code-review-methodology.md"]) {
  const t = readFileSync(join(dirname(WORKFLOW), "..", "methodology", m), "utf8");
  check(`${m} sanctions the listed callers, and nothing wider`, (t.match(/the callers a `<dependents>` block in your prompt lists/g) || []).length === 2 && !/only sanctioned cross-file lookup\b(?!s)/.test(t));
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall blast-radius expectations hold");
