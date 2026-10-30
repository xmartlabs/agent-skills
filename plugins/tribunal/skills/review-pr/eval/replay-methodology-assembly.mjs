#!/usr/bin/env node
// replay-methodology-assembly.mjs — the methodology a reviewer reads is the
// plugin's template with the repo's rules filled in, and for the repos it was
// split from it is the old file, byte for byte.
//
// The identity check is the oracle for moving project knowledge out of the
// plugin: the frozen rules of each repo, put back into the templates, must give
// exactly the methodology v87 shipped (frozen beside them in methodology-v87).
// v93 changed the templates on purpose to match the repos' conventions, so the
// golden became methodology-v93: the same frozen rules in the v93 templates.
// v94 added "Which tool reads what" (each read tool for the place it can see),
// so the golden became methodology-v94. v99 told a tribunal lens to keep a
// doubtful finding's severity for the panel instead of self-downgrading it, so
// the golden is now methodology-v99, built as v94 plus exactly those two
// insertions rather than by running the assembler. A
// slot that lost a line, or a rules block that drifted, fails here before any
// review reads it.
//
// Usage: node eval/replay-methodology-assembly.mjs

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assemble, assembleKind, KINDS, templatePath } from "../scripts/assemble-methodology.mjs";
import { HARNESS_DATA, PLUGIN_ROOT, SKILL_DIR, TRIBUNAL_DATA } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};
const throws = (fn, pattern) => {
  try {
    fn();
    return false;
  } catch (err) {
    return pattern.test(err.message);
  }
};
const sha = (t) => createHash("sha256").update(t).digest("hex");

const GOLDEN = join(TRIBUNAL_DATA, "fixtures", "methodology-v99");
const RULES_DIR = join(HARNESS_DATA, "rules");
const SHIPPED = { backend: "code-review-methodology.md", frontend: "fe-code-review-methodology.md", e2e: "e2e-code-review-methodology.md" };
// The kinds the templates were split from in v99; a kind added later has no
// frozen methodology to come back to.
const V99_KINDS = ["backend", "frontend"];
const profiles = readdirSync(join(HARNESS_DATA, "profiles"))
  .filter((f) => f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(join(HARNESS_DATA, "profiles", f), "utf8")));

console.log("\nmethodology assembly — templates, slots and the repo's rules");

// Deliberate edits to the generic text since v99, applied to the frozen copy
// before comparing, so the identity still covers every other byte. The prefix
// example was a real one from the repo the templates were split from.
const SINCE_V99 = [[/startsWith\('[a-z]{2}-'\)/, "startsWith('ext-')"]];
const sinceV99 = (text) => SINCE_V99.reduce((t, [from, to]) => t.replace(from, to), text);

console.log("\n--- the repos it was split from get their old methodology back ---");
for (const kind of V99_KINDS) {
  const profile = profiles.find((p) => p.reviewer === kind);
  const rulesFile = join(RULES_DIR, `${profile.repo.replace("/", "__")}.md`);
  const golden = sinceV99(readFileSync(join(GOLDEN, `${kind}.md`), "utf8"));
  const a = assembleKind({ kind, rulesText: readFileSync(rulesFile, "utf8") });
  check(`${kind}: template + frozen rules is the v99 methodology byte for byte`, a.text === golden, `${sha(a.text).slice(0, 12)} vs ${sha(golden).slice(0, 12)}`);
  check(`${kind}: every slot is filled and no block is left over`, a.filled.length === a.slots.length && a.unknown.length === 0, JSON.stringify({ filled: a.filled.length, slots: a.slots.length, unknown: a.unknown }));
  check(`${kind}: the profile names its rules file`, profile.projectRules === ".claude/review-rules.md", String(profile.projectRules));
}

console.log("\n--- the shipped files are the templates with nothing filled in ---");
for (const kind of KINDS) {
  const shipped = readFileSync(join(PLUGIN_ROOT, "methodology", SHIPPED[kind]), "utf8");
  check(
    `methodology/${SHIPPED[kind]} == assemble(templates/${kind}.md, no rules)`,
    shipped === assembleKind({ kind }).text,
    "regenerate it: node scripts/assemble-methodology.mjs --kind " + kind + " --out ../../methodology/" + SHIPPED[kind],
  );
  check(`templates/${kind}.md carries no slot marker into the shipped file`, !/<!-- \/?slot/.test(shipped));
}

console.log("\n--- an e2e suite gets no application-stack checklist ---");
{
  const text = assembleKind({ kind: "e2e" }).text;
  const stack = text.match(/prisma|\bzod\b|N\+1|\$transaction|controller|\bDAL\b|BullMQ|React|useEffect|useMemo|useCallback|re-?render|virtualis/gi) || [];
  check("the e2e methodology names no backend or React check", stack.length === 0, JSON.stringify([...new Set(stack)]));
  const r = assembleKind({
    kind: "e2e",
    rulesText: ["<!-- slot: tribunal/real-data-writes -->", "ours", "<!-- /slot -->", "<!-- slot: tribunal/route-auth -->", "backend only", "<!-- /slot -->"].join("\n"),
  });
  check("a block for a backend-only slot is reported, not placed", r.unknown.join() === "tribunal/route-auth" && !r.text.includes("backend only") && r.filled.join() === "real-data-writes");
}

console.log("\n--- one rules file, two plugins ---");
const T = ["a", "<!-- slot: x -->", "default x", "<!-- /slot -->", "b", "<!-- slot: y -->", "<!-- /slot -->", "c"].join("\n");
{
  const r = assemble(T, ["<!-- slot: tribunal/x -->", "mine", "<!-- /slot -->", "<!-- slot: other/x -->", "other's", "<!-- /slot -->"].join("\n"));
  check("a tribunal/ block fills its slot and another tool's block is ignored", r.text === "a\nmine\nb\nc" && r.filled.join() === "x", JSON.stringify(r.text));
  const empty = assemble(T, null);
  check("no rules: every slot keeps its default", empty.text === "a\ndefault x\nb\nc");
  const odd = assemble(T, ["<!-- slot: tribunal/z -->", "lost", "<!-- /slot -->"].join("\n"));
  check("a tribunal/ block naming no slot is reported, not placed", odd.unknown.join() === "tribunal/z" && !odd.text.includes("lost"));
  const prose = assemble(T, ["# Review rules", "Text a person reads.", "<!-- slot: tribunal/y -->", "filled y", "<!-- /slot -->"].join("\n"));
  check("text outside the blocks is ignored", prose.text === "a\ndefault x\nb\nfilled y\nc");
}

console.log("\n--- a malformed file is refused, not half-applied ---");
check("an unclosed block", throws(() => assemble(T, "<!-- slot: tribunal/x -->\nno end"), /never closes/));
check("a block inside a block", throws(() => assemble(T, "<!-- slot: tribunal/x -->\n<!-- slot: tribunal/y -->\n<!-- /slot -->\n<!-- /slot -->"), /opens inside/));
check("a close with no open", throws(() => assemble(T, "<!-- /slot -->"), /never opened/));
check("a marker with a typo", throws(() => assemble(T, "<!-- slot:tribunal/x -->\nx\n<!-- /slot -->"), /not a slot marker/));
check("the same block twice", throws(() => assemble(T, "<!-- slot: tribunal/x -->\n1\n<!-- /slot -->\n<!-- slot: tribunal/x -->\n2\n<!-- /slot -->"), /appears twice/));
check("a template slot twice", throws(() => assemble("<!-- slot: x -->\n<!-- /slot -->\n<!-- slot: x -->\n<!-- /slot -->", null), /appears twice/));
check("a kind with no template", throws(() => assembleKind({ kind: "mobile" }), /--kind must be one of/));

console.log("\n--- the command line ---");
{
  const dir = mkdtempSync(join(tmpdir(), "pr-assemble-"));
  const rules = join(dir, "rules.md");
  const out = join(dir, "m.md");
  const text = "<!-- slot: tribunal/intro -->\nYou review this repo.\n<!-- /slot -->\n";
  writeFileSync(rules, text);
  const run = (args) => spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "assemble-methodology.mjs"), ...args], { encoding: "utf8" });
  const good = run(["--kind", "backend", "--rules", rules, "--rules-sha256", sha(text), "--out", out]);
  const printed = JSON.parse(good.stdout || "{}");
  check(
    "the sha256 it prints is the sha256 of the file it wrote",
    good.status === 0 && printed.sha256 === sha(readFileSync(out, "utf8")) && readFileSync(out, "utf8").startsWith("You review this repo.\n"),
    good.stdout + good.stderr,
  );
  const stale = run(["--kind", "backend", "--rules", rules, "--rules-sha256", "0".repeat(64), "--out", join(dir, "n.md")]);
  check("a rules file whose bytes are not the recorded ones is refused", stale.status === 1 && /not the 0{8}/.test(stale.stdout), stale.stdout);
  const noOut = run(["--kind", "backend"]);
  check("--out is required", noOut.status === 1 && noOut.stdout.includes("--out"), noOut.stdout);
  check("the template path is the plugin's own", templatePath("backend") === join(PLUGIN_ROOT, "methodology", "templates", "backend.md"));
}

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all methodology-assembly expectations hold"}`);
process.exit(failures.length ? 1 : 0);
