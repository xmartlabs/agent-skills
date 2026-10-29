#!/usr/bin/env node
// build-workflow.mjs: build workflows/tribunal.js from workflow-src/tribunal.js.
//
// The Workflow runtime refuses a script over 524,288 bytes, per file, a
// scriptPath child included (probed 2026-10-19: "Workflow script file ...
// exceeds 524288 bytes"). On that date tribunal.js was 507,883 bytes, 45% of
// them full-line comments. So the commented source lives in workflow-src/, which
// the runtime never loads, and the shipped script is that source with every
// comment-only line removed and blank runs collapsed. Nothing else
// changes: code, prompts and embedded bash are byte-identical, so the token
// stream of the output equals the source's (replay-workflow-built.mjs).
//
// Region markers (`// ===== BEGIN X =====`) are kept: lib/*.mjs slices regions
// out of the shipped file at run time.
//
// Usage:
//   node build-workflow.mjs           # write workflows/tribunal.js
//   node build-workflow.mjs --check   # exit 1 when the committed file is stale

import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const PLUGIN_ROOT = resolve(HERE, "..", "..", "..");
export const SRC = join(PLUGIN_ROOT, "workflow-src", "tribunal.js");
export const OUT = join(PLUGIN_ROOT, "workflows", "tribunal.js");

const MARKER = /^\s*\/\/ ===== (BEGIN|END) [A-Z0-9-]+ =====\s*$/;

const REGEX_AFTER_WORD = new Set(["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield", "await"]);

// For each line, whether it starts in plain code (not inside a string,
// template literal, regex or block comment). Code inside `${}` counts as code.
export function lineStartsInCode(src) {
  const starts = [true];
  const stack = []; // "t" = template text, number = brace depth of a ${} code frame
  let mode = "code"; // code | sq | dq | tpl | line | block | regex | class
  let prev = ""; // last significant token in code: "" | "word" | "num" | punct char
  let word = "";
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    const n = src[i + 1];
    // An escaped newline continues a string, template or regex; it still ends a line.
    if (c === "\\" && n === "\n" && mode !== "code" && mode !== "line" && mode !== "block") {
      starts.push(false);
      i++;
      continue;
    }
    if (c === "\n") {
      if (mode === "line") mode = stack.length && stack[stack.length - 1] === "t" ? "tpl" : "code";
      if (mode === "code" && word) {
        prev = REGEX_AFTER_WORD.has(word) ? "kw" : "word";
        word = "";
      }
      starts.push(mode === "code");
      continue;
    }
    switch (mode) {
      case "line":
        break;
      case "block":
        if (c === "*" && n === "/") { mode = "code"; i++; }
        break;
      case "sq":
      case "dq":
        if (c === "\\") i++;
        else if ((c === "'" && mode === "sq") || (c === '"' && mode === "dq")) { mode = "code"; prev = "str"; }
        break;
      case "tpl":
        if (c === "\\") i++;
        else if (c === "`") { stack.pop(); mode = "code"; prev = "str"; }
        else if (c === "$" && n === "{") { stack.push(0); mode = "code"; prev = "("; i++; }
        break;
      case "regex":
        if (c === "\\") i++;
        else if (c === "[") mode = "class";
        else if (c === "/") { mode = "code"; prev = "str"; while (/[a-z]/.test(src[i + 1] || "")) i++; }
        break;
      case "class":
        if (c === "\\") i++;
        else if (c === "]") mode = "regex";
        break;
      case "code": {
        if (/[A-Za-z0-9_$]/.test(c)) { word += c; continue; }
        if (word) { prev = REGEX_AFTER_WORD.has(word) ? "kw" : "word"; word = ""; }
        if (c === " " || c === "\t" || c === "\r") break;
        if (c === "/" && n === "/") { mode = "line"; i++; break; }
        if (c === "/" && n === "*") { mode = "block"; i++; break; }
        if (c === "'") { mode = "sq"; break; }
        if (c === '"') { mode = "dq"; break; }
        if (c === "`") { stack.push("t"); mode = "tpl"; break; }
        if (c === "/") {
          const division = prev === "word" || prev === "str" || prev === ")" || prev === "]" || prev === "}";
          if (!division) { mode = "regex"; break; }
          prev = "/";
          break;
        }
        const top = stack[stack.length - 1];
        if (typeof top === "number") {
          if (c === "{") stack[stack.length - 1]++;
          else if (c === "}") {
            if (top === 0) { stack.pop(); mode = "tpl"; break; }
            stack[stack.length - 1]--;
          }
        }
        prev = c;
        break;
      }
    }
  }
  if (mode !== "code" || stack.length) throw new Error(`tokenizer ended inside ${mode} (stack ${JSON.stringify(stack)}): refusing to strip`);
  return starts;
}

export function stripComments(src) {
  const lines = src.split("\n");
  const inCode = lineStartsInCode(src);
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (inCode[i] && /^\s*\/\//.test(l) && !MARKER.test(l)) continue;
    if (inCode[i] && l.trim() === "" && out.length && out[out.length - 1].trim() === "") continue;
    out.push(l);
  }
  return out.join("\n");
}

const NOTICE = "// GENERATED by skills/review-pr/scripts/build-workflow.mjs from workflow-src/tribunal.js. Edit that file, then rebuild.";

export function build(source = readFileSync(SRC, "utf8")) {
  const stripped = stripComments(source);
  const end = stripped.indexOf("\n};\n");
  if (!stripped.startsWith("export const meta") || end === -1) throw new Error("the source must open with the meta block");
  return stripped.slice(0, end + 4) + NOTICE + "\n" + stripped.slice(end + 4);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const built = build();
  if (process.argv.includes("--check")) {
    const current = readFileSync(OUT, "utf8");
    if (current !== built) {
      console.error("workflows/tribunal.js is stale: run node skills/review-pr/scripts/build-workflow.mjs");
      process.exit(1);
    }
    console.log(`workflows/tribunal.js is current (${Buffer.byteLength(built)} bytes)`);
  } else {
    writeFileSync(OUT, built);
    console.log(`wrote workflows/tribunal.js (${Buffer.byteLength(built)} bytes)`);
  }
}
