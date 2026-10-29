#!/usr/bin/env node
// region-oracle.mjs: runs functions from a marked region of the JavaScript
// workflow, for the Rust port's differential tests.
//
//   node region-oracle.mjs <REGION> <export,export,...> [<dep,dep,...>]
//
// stdin:  a JSON array of calls, [{"fn": "<export>", "args": [...]}, ...]
// stdout: a JSON array of results, one per call, {"ok": <value>} or
//         {"err": "<message>"}. An undefined result is sent as null.
//
// The workflow path is resolved from this file's location on purpose, not
// from CLAUDE_PLUGIN_ROOT: inside a Claude Code session that variable can point
// at the installed plugin, and the port must be compared against the source in
// its own checkout.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "../../skills/review-pr/eval/extract-pure-region.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "workflow-src", "tribunal.js");

const [region, exportsArg, depsArg] = process.argv.slice(2);
if (!region || !exportsArg) {
  console.error("usage: region-oracle.mjs <REGION> <exports> [<deps>]");
  process.exit(2);
}
const { api, violations } = loadRegion(
  WORKFLOW,
  region,
  exportsArg.split(","),
  depsArg ? depsArg.split(",") : [],
);
if (violations.length) {
  console.error(`${region} is not pure:\n  ${violations.join("\n  ")}`);
  process.exit(1);
}

const calls = JSON.parse(readFileSync(0, "utf8"));
const results = calls.map(({ fn, args }) => {
  if (typeof api[fn] !== "function") return { err: `${region} exports no ${fn}` };
  try {
    const value = api[fn](...args);
    return { ok: value === undefined ? null : value };
  } catch (err) {
    return { err: String(err && err.message) };
  }
});
process.stdout.write(JSON.stringify(results));
