#!/usr/bin/env node
// replay-workflow-parses.mjs — the Workflow can actually LOAD peer-review.js.
//
// WHY THIS EXISTS, and why the existing check was not enough. A backtick in
// prompt text terminates the JS template literal it sits in. The file usually
// stays syntactically valid — it just means something else — so `node --check`
// passes and the Workflow tool then refuses the script outright. That is the
// v10/v11/v12 failure: three freezes tagged, shipped, and unable to execute.
//
// `replay-embedded-bash.mjs` was written to close that class, and RELEASES.md
// has said "Class closed" ever since. It was not closed. That check scans the
// bodies of quoted HEREDOCS, and on 2026-07-28 a bare `` `cat` `` in ordinary
// prompt text — nowhere near a heredoc — broke the file again. The whole battery
// was green, 30/30, and the gate that caught it was a human launching a run.
//
// So the check here is not "are backticks escaped" — that is one spelling of one
// instance. It is: **does this file still parse the way the runtime parses it?**
//
// HOW. The Workflow evaluates the script body in an async context. Reproducing
// that is what discriminates: `node --check` accepts the broken file, while
// wrapping the body in an async function and handing it to `new Function` throws
// `missing ) after argument list` on exactly the version the Workflow rejected.
// Verified by reproducing the real fault, not by reasoning about it.
//
// Parsing only — `new Function` COMPILES its body and never runs it, so nothing
// in the workflow executes here.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { WORKFLOW_BUILT as WORKFLOW } from "./paths.mjs";


const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// The built file, since that is the one the runtime loads (paths.mjs). Its
// comments are gone, so a name bound only in a comment no longer counts below.
console.log(`\nworkflow loads — ${WORKFLOW}`);

const src = readFileSync(WORKFLOW, "utf8");

// `export` is module syntax and `new Function` compiles a function body, so the
// one export is neutralised first. Anchored on the declaration the meta block is
// required to open with, so a file that lost it fails loudly here rather than
// being silently checked in some other shape.
check("the meta block is still the first export", /^export const meta\s*=/m.test(src));
const body = src.replace(/^export const meta\s*=/m, "const meta =");

let parseError = null;
try {
  // eslint-disable-next-line no-new-func
  new Function(`return (async () => {\n${body}\n})`);
} catch (err) {
  parseError = err;
}
check(
  "the script parses in an async context, as the Workflow loads it",
  parseError === null,
  parseError ? `${parseError.message} — the Workflow will refuse this file` : "",
);

// The Workflow tool refuses a script over 524288 bytes ("exceeds 524288 bytes",
// measured 2026-10-19 on a v91 snapshot at 529,685). v90 shipped at 518,154,
// 6 KB under it. Bytes, not characters: an em dash costs three. HEADROOM keeps
// the next release from finding the limit the same way.
const WORKFLOW_LIMIT_BYTES = 524288;
const HEADROOM_BYTES = 16384;
const bytes = Buffer.byteLength(readFileSync(WORKFLOW));
check(
  `the script is at least ${HEADROOM_BYTES} bytes under the Workflow's ${WORKFLOW_LIMIT_BYTES}-byte limit`,
  bytes <= WORKFLOW_LIMIT_BYTES - HEADROOM_BYTES,
  `${bytes} bytes`,
);

// A second, independent reading of the same risk, kept because it names the
// cause rather than the symptom: every agent prompt is a template literal, and
// an unescaped backtick inside one is the way this breaks in practice. Counting
// them cannot prove correctness (an even number of stray backticks still
// parses), so this is a REPORT, not an assertion — the parse above is the gate.
const stray = [...src.matchAll(/^.*[^\\]`[a-z-]+[^\\]`.*$/gm)]
  .map((m) => m[0].trim())
  .filter((l) => !l.trimStart().startsWith("//") && !l.trimStart().startsWith("*"));
if (stray.length)
  console.log(
    `  note  ${stray.length} line(s) contain an inline \`word\` pair; harmless when escaped, and the parse above is what decides`,
  );

// A parse cannot see a name nobody declared. On 2026-10-18 an edit left
// `${decisionsRule}` in fetch-pr's prompt with no such binding anywhere: the
// file parsed, and the first live run would have thrown a ReferenceError at the
// fetch, before any review. A bare name that appears ONLY inside `${...}` is
// that bug, whatever its spelling.
const interpolated = new Set([...src.matchAll(/\$\{([A-Za-z_$][\w$]*)\}/g)].map((m) => m[1]));
const outside = src.replace(/\$\{[A-Za-z_$][\w$]*\}/g, "");
const undeclared = [...interpolated].filter((name) => !new RegExp(`(^|[^\\w$])${name.replace(/\$/g, "\\$")}([^\\w$]|$)`).test(outside));
check("every name interpolated into a template is bound somewhere else in the file", undeclared.length === 0, undeclared.join(", "));

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe workflow parses the way its runtime loads it");
