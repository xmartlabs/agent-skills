#!/usr/bin/env node
// replay-workflow-built.mjs: Tier 0, 0 tokens. workflows/tribunal.js is the
// build of workflow-src/tribunal.js, and the build removed only comments.
//
// The source is what people edit and what the rest of this battery reads; the
// built file is what the Workflow runtime loads (scripts/build-workflow.mjs
// says why there are two). Three ways that goes wrong, one check each:
// - someone edits the source and forgets to rebuild, so the run ships old code;
// - someone edits the built file by hand, so the next build silently undoes it;
// - the stripper misreads where a template literal ends and deletes a prompt
//   line that starts with `//`. The file still parses and the prompt is wrong.
// The last one is checked against TypeScript's scanner, an independent
// tokenizer, when it is installed: the token streams of source and build must
// be identical. Skipped, not failed, when it is not.

import { readFileSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { join } from "node:path";
import { WORKFLOW, WORKFLOW_BUILT } from "./paths.mjs";
import { build, stripComments } from "../scripts/build-workflow.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const source = readFileSync(WORKFLOW, "utf8");
const built = readFileSync(WORKFLOW_BUILT, "utf8");

console.log("\n--- the committed build is current ---");
check("workflows/tribunal.js equals build(workflow-src/tribunal.js)", built === build(source), "run node skills/review-pr/scripts/build-workflow.mjs");
const markers = (t) => t.split("\n").filter((l) => /^\s*\/\/ ===== (BEGIN|END) /.test(l));
check("every region marker survives the build, in order", markers(built).join("\n") === markers(source).join("\n") && markers(source).length > 0, `${markers(source).length} in source, ${markers(built).length} built`);

console.log("\n--- the stripper keeps everything that is not a comment ---");
const FIXTURES = [
  ["a // line inside a template literal", "const a = `x\n// kept\ny`;\n// gone\n"],
  ["a backtick inside ${}", "const a = `x ${b ? `inner` : \"\"}\n// kept\n`;\n// gone\n"],
  ["a regex containing //", "const r = /https?:\\/\\//;\n// gone\nconst q = `\n// kept\n`;\n"],
  ["a regex containing a quote and a backtick", "const r = /['`]/g;\nconst s = `t\n// kept\n`;\n// gone\n"],
  ["a comment containing a backtick and a quote", "// gone `tick` and ' quote\nconst s = `t\n// kept\n`;\n"],
  ["a // line inside a string continued by \\", "const s = \"a\\\n// kept\";\n// gone\n"],
];
for (const [name, src] of FIXTURES) {
  const out = stripComments(src);
  check(name, !out.includes("// gone") && out.includes("// kept"), JSON.stringify(out));
}
let threw = false;
try { stripComments("const a = `never closed\n"); } catch { threw = true; }
check("an unterminated template makes the build refuse, not guess", threw);

console.log("\n--- an independent tokenizer agrees ---");
const tsCandidates = [process.env.TYPESCRIPT_PATH, join(homedir(), ".config", "yarn", "global", "node_modules", "typescript"), "/usr/local/lib/node_modules/typescript", "/opt/homebrew/lib/node_modules/typescript"].filter(Boolean);
const tsPath = tsCandidates.find((p) => existsSync(join(p, "package.json")));
if (!tsPath) {
  console.log("  skip  TypeScript is not installed; set TYPESCRIPT_PATH to run the token comparison");
} else {
  const ts = createRequire(import.meta.url)(tsPath);
  const K = ts.SyntaxKind;
  const DIVISION_AFTER = new Set([K.Identifier, K.NumericLiteral, K.StringLiteral, K.CloseParenToken, K.CloseBracketToken, K.CloseBraceToken, K.NoSubstitutionTemplateLiteral, K.TemplateTail, K.ThisKeyword, K.TrueKeyword, K.FalseKeyword, K.NullKeyword, K.RegularExpressionLiteral]);
  const tokens = (text) => {
    const sc = ts.createScanner(ts.ScriptTarget.Latest, true, ts.LanguageVariant.Standard, text);
    const out = [];
    const braces = [];
    let k;
    while ((k = sc.scan()) !== K.EndOfFileToken) {
      if ((k === K.SlashToken || k === K.SlashEqualsToken) && !(out.length && DIVISION_AFTER.has(out[out.length - 1][0]))) k = sc.reScanSlashToken();
      if (k === K.OpenBraceToken) braces.push("{");
      else if (k === K.TemplateHead) braces.push("${");
      else if (k === K.CloseBraceToken && braces.pop() === "${") {
        k = sc.reScanTemplateToken(false);
        if (k === K.TemplateMiddle) braces.push("${");
      }
      out.push([k, sc.getTokenText()]);
    }
    return out;
  };
  const a = tokens(source);
  const b = tokens(built);
  let at = -1;
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (!a[i] || !b[i] || a[i][0] !== b[i][0] || a[i][1] !== b[i][1]) { at = i; break; }
  }
  check(`source and build have the same ${a.length} tokens (TypeScript ${ts.version})`, at === -1 && a.length > 1000, at === -1 ? `${a.length} tokens` : `first difference at token ${at}: ${JSON.stringify((a[at] || [])[1] || "").slice(0, 120)}`);
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe built workflow is the source minus its comments");
