#!/usr/bin/env node
// fake-gh-okf.mjs — a `gh` that answers the calls load-conventions.mjs makes
// for conventionSources from a fixture, so the loader runs with no network:
// replay-convention-sources.mjs, templates/check.sh and the Rust differential
// test (which records what this answers) all use it.
//
// FAKE_GH_FIXTURE names a JSON file:
//   head        the commit `gh pr view <n> --json headRefOid` answers
//   pointers    {<submodule path>: {head: <sha>|null, base: <sha>|null}}: the
//               pointer at the head commit and at any other ref; null is a 404
//   anyPointer  a commit every other path is a submodule pointing to
//   commits     {<ref>: <sha>}, and defaultCommit for any other ref
//   bundles     {<sha>: {<path>: <text>}}: the docs repo at that commit, or
//               the reviewed repo's for a source without `repo`. The tree
//               answers every path in it, README.md and non-Markdown files
//               included, and ignores the --jq filter: the loader keeps only
//               the paths under `root` and, for okf-code and all, the `.md`
//               ones itself, so a filter it forgets shows here.
//   truncated   true to answer every tree as truncated
//   files       {<path>: <text>}: the reviewed repo's own files (raw contents)
//   fail        [{match, stderr, code}]: any call whose joined arguments
//               contain `match` fails that way
// Repositories are not told apart. GH_CALL_LOG, when set, gets one JSON line
// per call.

import { createHash } from "node:crypto";
import { appendFileSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);
if (process.env.GH_CALL_LOG) appendFileSync(process.env.GH_CALL_LOG, JSON.stringify(args) + "\n");
const fx = JSON.parse(readFileSync(process.env.FAKE_GH_FIXTURE, "utf8"));
const answer = (stdout, code = 0, stderr = "") => {
  process.stdout.write(stdout);
  process.stderr.write(stderr);
  process.exitCode = code;
};
const notFound = () => answer("", 1, "gh: Not Found (HTTP 404)\n");
const blobSha = (text) => {
  const body = Buffer.from(text, "utf8");
  return createHash("sha1").update(Buffer.concat([Buffer.from(`blob ${body.length}\0`), body])).digest("hex");
};

const joined = args.join(" ");
const failure = (fx.fail || []).find((f) => joined.includes(f.match));
if (failure) answer(failure.stdout || "", failure.code ?? 1, failure.stderr || "");
else if (args[0] === "pr" && args[1] === "view") {
  if (fx.head) answer(`${fx.head}\n`);
  else notFound();
} else if (args[0] === "api" && args[1] === "graphql") {
  const query = args[3].replace(/^query=/, "");
  const data = {};
  for (const m of query.matchAll(/(b\d+): object\(expression: ("(?:[^"\\]|\\.)*")\)/g)) {
    const expr = JSON.parse(m[2]);
    const cut = expr.indexOf(":");
    const docs = (fx.bundles || {})[expr.slice(0, cut)] || {};
    const path = expr.slice(cut + 1);
    data[m[1]] = path in docs ? { text: docs[path] } : null;
  }
  answer(JSON.stringify({ data: { repository: data } }));
} else if (args[0] === "api") {
  const endpoint = args[1];
  let m;
  if ((m = endpoint.match(/^repos\/[^/]+\/[^/]+\/git\/trees\/([0-9a-f]+)\?recursive=1$/))) {
    const docs = (fx.bundles || {})[m[1]];
    if (!docs) notFound();
    else {
      const tree = Object.keys(docs).map((p) => ({ path: p, sha: blobSha(docs[p]), size: Buffer.byteLength(docs[p]) }));
      answer(JSON.stringify({ truncated: fx.truncated === true, tree }) + "\n");
    }
  } else if ((m = endpoint.match(/^repos\/[^/]+\/[^/]+\/commits\/(.+)$/))) {
    const ref = decodeURIComponent(m[1]);
    const sha = (fx.commits || {})[ref] || fx.defaultCommit;
    if (sha) answer(`${sha}\n`);
    else notFound();
  } else if ((m = endpoint.match(/^repos\/[^/]+\/[^/]+\/contents\/(.*)\?ref=(.*)$/))) {
    const path = m[1];
    const ref = decodeURIComponent(m[2]);
    if (args.includes("Accept: application/vnd.github.raw")) {
      const files = fx.files || {};
      if (path in files) answer(files[path]);
      else notFound();
    } else {
      const p = (fx.pointers || {})[path];
      const sha = p ? (ref === fx.head ? p.head : p.base) : fx.anyPointer || null;
      if (!sha) notFound();
      else answer(JSON.stringify({ name: path.split("/").pop(), path, sha, size: 0, type: "submodule" }));
    }
  } else notFound();
} else notFound();
