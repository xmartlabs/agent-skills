#!/usr/bin/env node
// span-oracle.mjs: runs code of the JavaScript workflow that sits OUTSIDE any
// marked region (the glue between regions), for the Rust port's differential
// tests.
//
//   node span-oracle.mjs
//
// stdin: a JSON array of calls,
//   [{"start": "<anchor>", "end": "<anchor>", "deps": ["<REGION>", ...],
//     "params": {"<free variable>": <value>, ...}, "ret": "<expression>"}, ...]
// The span runs from the start of `start` to the end of `end`, both of which
// must occur in the workflow, `start` exactly once. `pre` lists more
// [start, end] pairs, run before it: declarations the span reads. Every free variable of the
// span is passed in `params`; a param named in `fns` is a JavaScript source
// string evaluated to a value first (for a function such as `log`). `ret` is
// evaluated after the span, in its scope. The span runs as an async function,
// so it may `await`; a param that is a function may return a promise.
// stdout: a JSON array of results, one per call, {"ok": <value>} or
//         {"err": "<message>"}.
//
// The workflow path is resolved from this file's location, as region-oracle
// does, so the port is compared against the source in its own checkout.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { extractRegions } from "../../skills/review-pr/eval/extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "workflow-src", "tribunal.js");
const src = readFileSync(WORKFLOW, "utf8");

function span(start, end) {
  const s = src.indexOf(start);
  if (s === -1) throw new Error(`start anchor not found: ${start}`);
  if (src.indexOf(start, s + 1) !== -1) throw new Error(`start anchor is not unique: ${start}`);
  const e = src.indexOf(end, s);
  if (e === -1) throw new Error(`end anchor not found after start: ${end}`);
  return src.slice(s, e + end.length);
}

const AsyncFunction = (async () => {}).constructor;
const calls = JSON.parse(readFileSync(0, "utf8"));
const results = [];
for (const c of calls) results.push(await run(c));
async function run(c) {
  try {
    const body = [
      ...(c.deps || []).flatMap((d) => extractRegions(src, d)),
      ...(c.pre || []).map(([s, e]) => span(s, e)),
      span(c.start, c.end),
      `return (${c.ret});`,
    ].join("\n");
    const names = Object.keys(c.params || {});
    const fns = new Set(c.fns || []);
    const values = names.map((n) => (fns.has(n) ? new Function(`return (${c.params[n]});`)() : c.params[n]));
    const value = await new AsyncFunction(...names, body)(...values);
    return { ok: value === undefined ? null : value };
  } catch (err) {
    return { err: String(err && err.message) };
  }
}
process.stdout.write(JSON.stringify(results));
