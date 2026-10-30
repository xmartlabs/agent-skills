#!/usr/bin/env node
// assemble-methodology.mjs — build the methodology one review reads: the
// plugin's template for the reviewer kind, with each named slot filled from the
// repo's own review rules.
//
// The plugin ships no project knowledge. `methodology/templates/<kind>.md`
// holds the review method with slots, each carrying generic default text:
//
//   <!-- slot: path-rules -->
//   ...default text, often empty...
//   <!-- /slot -->
//
// The repo's rules file (profile.projectRules, read at the PR's base branch
// like the profile) holds blocks named after the plugin and the slot:
//
//   <!-- slot: tribunal/path-rules -->
//   ...this repo's text...
//   <!-- /slot -->
//
// A block replaces its slot's default, markers and all; a slot with no block
// keeps its default. The file can be shared with other review tools, each
// reading its own `<tool>/*` blocks, so blocks of another tool are ignored here. Text outside the blocks
// is ignored too, so the file can explain itself to the people who edit it.
// A `tribunal/*` block naming no slot is reported, since it is either a typo or
// a slot that moved, and either way its text reaches no reviewer.
//
// Usage:
//   node assemble-methodology.mjs --kind backend|frontend|e2e [--rules <file> [--rules-sha256 <sha>]] --out <file>
// Prints ONE line of JSON: {ok, out, sha256, kind, rulesSha256, filled, unknown}
// or {ok: false, error} and exits 1. --rules-sha256 refuses a rules file whose
// bytes are not the ones load-profile.mjs recorded.

import { createHash } from "node:crypto";
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const NAMESPACE = "tribunal";
export const KINDS = ["backend", "frontend", "e2e"];
const TEMPLATES = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "methodology", "templates");

const OPEN = /^<!-- slot: ([a-z0-9][a-z0-9-]*(?:\/[a-z0-9][a-z0-9-]*)?) -->$/;
const CLOSE = /^<!-- \/slot -->$/;
const LOOKS_LIKE_MARKER = /^<!--\s*\/?\s*slot\b/;

export const templatePath = (kind) => join(TEMPLATES, `${kind}.md`);
const sha256 = (text) => createHash("sha256").update(text).digest("hex");

// [{text}] for a line outside any slot, [{name, body}] for a slot.
export function parseSlots(text, where) {
  const parts = [];
  let open = null;
  text.split("\n").forEach((line, i) => {
    const o = line.match(OPEN);
    if (o) {
      if (open) throw new Error(`${where}:${i + 1}: slot "${o[1]}" opens inside "${open.name}"`);
      open = { name: o[1], body: [] };
      return;
    }
    if (CLOSE.test(line)) {
      if (!open) throw new Error(`${where}:${i + 1}: a slot closes that never opened`);
      parts.push(open);
      open = null;
      return;
    }
    if (LOOKS_LIKE_MARKER.test(line)) throw new Error(`${where}:${i + 1}: not a slot marker: ${line}`);
    if (open) open.body.push(line);
    else parts.push({ text: line });
  });
  if (open) throw new Error(`${where}: slot "${open.name}" never closes`);
  return parts;
}

export function assemble(template, rules, namespace = NAMESPACE) {
  const parts = parseSlots(template, "template");
  const slots = parts.filter((p) => p.name).map((p) => p.name);
  const dup = slots.find((n, i) => slots.indexOf(n) !== i);
  if (dup) throw new Error(`template: slot "${dup}" appears twice`);
  if (slots.some((n) => n.includes("/"))) throw new Error("template: a slot name carries a namespace");
  const given = new Map();
  if (rules) {
    for (const p of parseSlots(rules, "rules")) {
      if (!p.name) continue;
      const [ns, name] = p.name.includes("/") ? p.name.split("/") : [null, p.name];
      if (ns !== namespace) continue;
      if (given.has(name)) throw new Error(`rules: block "${p.name}" appears twice`);
      given.set(name, p.body);
    }
  }
  const lines = [];
  const filled = [];
  for (const p of parts) {
    if (!p.name) lines.push(p.text);
    else if (given.has(p.name)) {
      filled.push(p.name);
      lines.push(...given.get(p.name));
    } else lines.push(...p.body);
  }
  const unknown = [...given.keys()].filter((n) => !slots.includes(n)).map((n) => `${namespace}/${n}`);
  return { text: lines.join("\n"), slots, filled, unknown };
}

export function assembleKind({ kind, rulesText = null }) {
  if (!KINDS.includes(kind)) throw new Error(`--kind must be one of ${KINDS.join(", ")}, got ${JSON.stringify(kind)}`);
  return assemble(readFileSync(templatePath(kind), "utf8"), rulesText);
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

if (isMain) {
  const argv = process.argv.slice(2);
  const flag = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : "";
  };
  const done = (out) => {
    process.stdout.write(JSON.stringify(out) + "\n");
    process.exit(out.ok ? 0 : 1);
  };
  const kind = flag("--kind");
  const outPath = flag("--out");
  const rulesPath = flag("--rules");
  const expected = flag("--rules-sha256");
  if (!outPath) done({ ok: false, error: "--out <file> is required" });
  let rulesText = null;
  let rulesSha256 = null;
  if (rulesPath) {
    try {
      rulesText = readFileSync(rulesPath, "utf8");
    } catch (err) {
      done({ ok: false, error: `could not read --rules ${rulesPath}: ${err.message}` });
    }
    rulesSha256 = sha256(rulesText);
    if (expected && expected !== rulesSha256)
      done({ ok: false, error: `--rules ${rulesPath} has sha256 ${rulesSha256}, not the ${expected} load-profile.mjs recorded` });
  } else if (expected) done({ ok: false, error: "--rules-sha256 given without --rules" });
  try {
    const a = assembleKind({ kind, rulesText });
    writeFileSync(outPath, a.text);
    done({ ok: true, out: outPath, sha256: sha256(a.text), kind, rulesSha256, filled: a.filled, unknown: a.unknown });
  } catch (err) {
    done({ ok: false, error: err.message });
  }
}
