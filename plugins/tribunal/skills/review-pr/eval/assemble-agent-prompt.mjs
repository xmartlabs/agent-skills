#!/usr/bin/env node
// assemble-agent-prompt.mjs — render everything one dispatched agent is told,
// from every source, as a single text; then look for the sources disagreeing.
//
// WHY IT EXISTS. A dispatched subagent's instructions are not one file. They
// are assembled at runtime from three independently edited places:
//
//   1. the agent definition's frontmatter and body (agents/*.md)
//   2. a methodology file the body orders it to Read before doing anything
//   3. the prompt the workflow builds and interpolates at dispatch time
//
// Every review this harness has ever had reads those one at a time and asks
// "is this file right?". Each can be right while the union is incoherent, and
// two live instances say that is not hypothetical:
//
//   · SKILL.md says every read must be pinned to the PR head ref and the local
//     clone must not be grepped. methodology/code-review-methodology.md, which
//     the lens is told is "the actual procedure for this review", opens by
//     telling it to run `git diff` and `git status` against that clone.
//   · agents/tribunal-refuter.md hardcodes `slotStatus: "OK"` in the output it
//     demands, while REFUTATION_SCHEMA defines `UNAVAILABLE` as a real branch.
//     The abstention the schema built is unreachable from the prompt.
//
// Neither is visible in any single file, so no per-file check can find them.
// The published evidence says the agent will not flag the contradiction for
// you either: measured on six models over 1,200 conflict cases, obedience to
// the prioritised instruction under real conflict runs 9.6-45.8%, and explicit
// acknowledgment that a conflict existed at all runs 0-20.3% (Control
// Illusion, arXiv:2502.15851). A contradiction that reaches dispatch is
// resolved silently, and not predictably in your favour.
//
// WHAT IT IS NOT. It cannot evaluate the dispatch prompt, which is a template
// literal over live run state. It extracts that template's static skeleton
// with the `${...}` holes left in place, which is where the standing rules
// live and therefore where contradictions live. A conflict between two
// interpolated VALUES is out of reach here and always will be.
//
// Usage:
//   node eval/assemble-agent-prompt.mjs              # every agent, conflicts only
//   node eval/assemble-agent-prompt.mjs --full       # every agent, full text
//   node eval/assemble-agent-prompt.mjs tribunal-refuter --full
//   node eval/assemble-agent-prompt.mjs --strict     # exit 1 if any conflict

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const PLUGIN_ROOT = resolve(HERE, "..", "..", "..");

const read = (p) => (existsSync(p) ? readFileSync(p, "utf8") : null);

// A source is one file's contribution to what the agent is told, kept whole
// and labelled, so a conflict can always be traced back to a file and a line.
function source(origin, path, text) {
  return { origin, path, text: text || "" };
}

function lineOf(text, index) {
  return text.slice(0, index).split("\n").length;
}

// ---------------------------------------------------------------- assembly

export function agentNames(root = PLUGIN_ROOT) {
  const dir = join(root, "agents");
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith(".md"))
    .map((f) => basename(f, ".md"))
    .sort();
}

// The wrapper agents carry no procedure of their own on purpose; they name a
// methodology file and order the agent to Read it first. Follow that reference
// the way the agent would, so the assembled text is what actually reaches it.
export function methodologyRefs(body) {
  const out = new Set();
  for (const m of body.matchAll(/methodology\/([A-Za-z0-9._-]+\.md)/g))
    out.add(m[1]);
  return [...out];
}

// The static skeleton of every prompt the workflow interpolates for this agent
// type. Anchored on `agentType:` call sites, walking back to the `agent(` that
// owns them, because the prompt is the first argument and the options object
// carrying agentType is the last.
export function dispatchTemplates(workflowSrc, agentName) {
  const out = [];
  // agentType is written as a constant (LENS_AGENT) or an interpolation
  // (`${AGENT_NS}tribunal-refuter`), so resolve the constant back to a name.
  const constNames = new Set();
  for (const m of workflowSrc.matchAll(
    /const\s+(\w+)\s*=\s*[^\n;]*?["'`][^\n"'`]*?([a-z-]+)["'`]/g,
  )) {
    if (m[2] === agentName) constNames.add(m[1]);
  }
  for (const m of workflowSrc.matchAll(/agentType:\s*([^,\n}]+)/g)) {
    const expr = m[1].trim();
    const names = [...constNames].some((c) => expr.includes(c));
    if (!names && !expr.includes(agentName)) continue;
    // Walk back to the opening of the agent( call that owns this option.
    const open = workflowSrc.lastIndexOf("agent(", m.index);
    if (open === -1) continue;
    const promptStart = workflowSrc.indexOf("`", open);
    if (promptStart === -1 || promptStart > m.index) continue;
    let i = promptStart + 1;
    // Find the closing backtick of the template literal, skipping escapes and
    // the `${ ... }` holes (which may themselves contain backticks).
    for (; i < m.index; i++) {
      const ch = workflowSrc[i];
      if (ch === "\\") {
        i++;
        continue;
      }
      if (ch === "$" && workflowSrc[i + 1] === "{") {
        let depth = 1;
        i += 2;
        for (; i < m.index && depth > 0; i++) {
          if (workflowSrc[i] === "{") depth++;
          else if (workflowSrc[i] === "}") depth--;
        }
        i--;
        continue;
      }
      if (ch === "`") break;
    }
    if (workflowSrc[i] !== "`") continue;
    out.push({
      line: lineOf(workflowSrc, promptStart),
      text: workflowSrc.slice(promptStart + 1, i),
    });
  }
  return out;
}

export function assembleAgentPrompt(agentName, root = PLUGIN_ROOT) {
  const defPath = join(root, "agents", `${agentName}.md`);
  const def = read(defPath);
  if (def == null) throw new Error(`no agent definition at ${defPath}`);

  const sources = [source("agent-definition", defPath, def)];

  for (const ref of methodologyRefs(def)) {
    const p = join(root, "methodology", ref);
    const t = read(p);
    if (t != null) sources.push(source("methodology (read at runtime)", p, t));
  }

  const workflowPath = join(root, "workflows", "tribunal.js");
  const workflow = read(workflowPath) || "";
  for (const d of dispatchTemplates(workflow, agentName)) {
    sources.push(
      source(
        `dispatch template (workflow line ${d.line}, interpolations left as \${…})`,
        workflowPath,
        d.text,
      ),
    );
  }

  // The entry skill states harness-wide rules the orchestrator applies to
  // every dispatch. It is not pasted into the prompt, so it is carried as
  // context rather than as text the agent reads, but a rule stated there and
  // contradicted in a methodology file is the exact shape of instance A.
  const skillPath = join(root, "skills", "review-pr", "SKILL.md");
  const skill = read(skillPath);
  if (skill != null)
    sources.push(source("entry skill (harness-wide rules)", skillPath, skill));

  return { agent: agentName, sources };
}

// -------------------------------------------------------------- conflicts

// Rule stated in one source, contradicted by a directive in another. Kept as
// data rather than as code so the shapes are readable and extendable: each
// entry is one rule and the directives that cannot coexist with it.
const CONTRADICTION_RULES = [
  {
    kind: "unpinned-read-vs-pinned-rule",
    rule: /pinned to the PR head ref|Do not grep the shared local clone|never the local working tree|Read at the commit, never the working tree/i,
    directive:
      /^\s*(?:[-*]\s*)?(?:`)?(git\s+(?:diff|status|log)\b(?!\s+[a-f0-9]{7,})|gh\s+pr\s+diff\b)/im,
    why: "one source requires every read to be pinned to the PR head ref; another hands the agent a working-tree command that reads whatever branch the clone is parked on",
    // A directive is reconciled, not contradictory, when the source carrying it
    // says in so many words which caller it applies to and defers to the
    // pinning rule for the other. That is the mitigated form the criteria
    // accept elsewhere (a prose carrier, TD-24), and it is the honest reading:
    // the methodology serves two callers, and for a person reviewing their own
    // uncommitted work the working tree IS the subject — pinning there would
    // report the very changes they asked about as missing. The marker is a
    // literal so it cannot be satisfied by vague nearby prose.
    exemption: /PINNED-READ EXCEPTION APPLIES/,
  },
];

export function findContradictions(assembled) {
  const found = [];
  for (const r of CONTRADICTION_RULES) {
    const stating = assembled.sources.filter((s) => r.rule.test(s.text));
    if (!stating.length) continue;
    for (const s of assembled.sources) {
      const m = r.directive.exec(s.text);
      if (!m) continue;
      if (r.exemption && r.exemption.test(s.text)) continue;
      // A source contradicting itself is a different (and weaker) claim than
      // two sources disagreeing; this detector is only about the union.
      const others = stating.filter((x) => x.path !== s.path);
      if (!others.length) continue;
      const ruleHit = r.rule.exec(others[0].text);
      found.push({
        kind: r.kind,
        why: r.why,
        rule: {
          path: others[0].path,
          origin: others[0].origin,
          line: lineOf(others[0].text, ruleHit.index),
          text: ruleHit[0].trim(),
        },
        directive: {
          path: s.path,
          origin: s.origin,
          line: lineOf(s.text, m.index),
          text: m[0].trim(),
        },
      });
    }
  }
  return found;
}

// Every template literal the workflow passes as an agent()'s first argument.
// This is the whole set of things a model is ever told to write, which is what
// makes the next check answerable: an enum member no prompt anywhere names is
// a value no agent can produce.
export function promptTemplatesInWorkflow(src) {
  const out = [];
  // reviewerAgent() is agent() with the methodology line prepended
  // (workflows/tribunal.js); its call sites carry the reviewer prompts.
  for (const m of src.matchAll(/\b(?:reviewerAgent|agent)\(/g)) {
    // Whitespace and line comments can sit between `agent(` and its prompt —
    // several call sites explain a cache-ordering decision right there. Missing
    // them silently drops real prompts, which turns findDeadEnumBranches into
    // a false-positive machine: it reported "SURVIVES" as unreachable when the
    // adjudicator prompt names it, behind exactly such a comment.
    let start = m.index + m[0].length;
    for (;;) {
      while (start < src.length && /\s/.test(src[start])) start++;
      if (src.startsWith("//", start)) {
        const nl = src.indexOf("\n", start);
        if (nl === -1) break;
        start = nl + 1;
        continue;
      }
      break;
    }
    if (src[start] !== "`") continue;
    let i = start + 1;
    for (; i < src.length; i++) {
      const ch = src[i];
      if (ch === "\\") {
        i++;
        continue;
      }
      if (ch === "$" && src[i + 1] === "{") {
        let depth = 1;
        i += 2;
        for (; i < src.length && depth > 0; i++) {
          if (src[i] === "{") depth++;
          else if (src[i] === "}") depth--;
        }
        i--;
        continue;
      }
      if (ch === "`") break;
    }
    out.push({ line: lineOf(src, start), text: src.slice(start + 1, i) });
  }
  return out;
}

// A schema branch nothing can reach. The harness declares the value, spends
// code branching on it, and no prompt in the harness ever tells any agent it
// may emit it — so the branch is dead and the signal it carries is one the
// panel believes it has and does not.
//
// The question has to be asked HARNESS-WIDE, not per agent, or it is wrong.
// `source: "claude"` is hardcoded in the Claude refuter's prompt and that is
// correct: the field names who is voting, and the other members are produced
// by the external-CLI path. Only a member NO prompt can emit is a defect.
export function findDeadEnumBranches(root = PLUGIN_ROOT) {
  const workflowSrc = read(join(root, "workflows", "tribunal.js")) || "";

  const promptText = [
    ...promptTemplatesInWorkflow(workflowSrc).map((p) => p.text),
    ...agentNames(root).map((n) => read(join(root, "agents", `${n}.md`)) || ""),
    ...(existsSync(join(root, "methodology"))
      ? readdirSync(join(root, "methodology"))
          .filter((f) => f.endsWith(".md"))
          .map((f) => read(join(root, "methodology", f)) || "")
      : []),
  ].join("\n");

  const found = [];
  for (const m of workflowSrc.matchAll(
    /(\w+):\s*\{\s*type:\s*"string",\s*enum:\s*\[([^\]]+)\]/g,
  )) {
    const field = m[1];
    const members = [...m[2].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    if (members.length < 2) continue;
    const schemaLine = lineOf(workflowSrc, m.index);

    for (const member of members) {
      const quoted = new RegExp(`["'\`]${member}["'\`]|\\b${member}\\b`);
      if (quoted.test(promptText)) continue;
      // Declared and unreachable is only interesting if the harness acts on it.
      const branches = [
        ...workflowSrc.matchAll(
          new RegExp(`[!=]==?\\s*"${member}"|"${member}"\\s*[!=]==?`, "g"),
        ),
      ];
      if (!branches.length) continue;
      found.push({
        kind: "unreachable-enum-branch",
        why: `"${field}" declares "${member}" and the workflow branches on it at ${branches.length} site(s), but no prompt anywhere in the harness tells any agent it may emit that value — the branch is reachable only from code that fabricates it`,
        field,
        members,
        unreachable: member,
        schema: { line: schemaLine },
        branchLines: branches.map((b) => lineOf(workflowSrc, b.index)),
      });
    }
  }
  return found;
}

export function auditAgent(agentName, root = PLUGIN_ROOT) {
  const assembled = assembleAgentPrompt(agentName, root);
  return { assembled, conflicts: findContradictions(assembled) };
}

// ------------------------------------------------------------------- CLI

if (import.meta.url === `file://${process.argv[1]}`) {
  const args = process.argv.slice(2);
  const full = args.includes("--full");
  const strict = args.includes("--strict");
  const only = args.filter((a) => !a.startsWith("--"));
  const targets = only.length ? only : agentNames();

  let total = 0;
  for (const name of targets) {
    const { assembled, conflicts } = auditAgent(name);
    const bytes = assembled.sources.reduce((n, s) => n + s.text.length, 0);
    console.log(
      `\n=== ${name} — ${assembled.sources.length} source(s), ${bytes} chars assembled ===`,
    );
    for (const s of assembled.sources)
      console.log(`  · ${s.origin}: ${s.path.replace(PLUGIN_ROOT + "/", "")}`);

    if (full) {
      for (const s of assembled.sources) {
        console.log(`\n----- FROM ${s.path.replace(PLUGIN_ROOT + "/", "")} (${s.origin}) -----`);
        console.log(s.text);
      }
    }

    if (!conflicts.length) {
      console.log(`  no cross-source conflict detected`);
      continue;
    }
    total += conflicts.length;
    for (const c of conflicts) {
      console.log(`\n  CONFLICT [${c.kind}]`);
      console.log(`    ${c.why}`);
      console.log(
        `    rule:      ${c.rule.path.replace(PLUGIN_ROOT + "/", "")}:${c.rule.line} — ${JSON.stringify(c.rule.text)}`,
      );
      console.log(
        `    directive: ${c.directive.path.replace(PLUGIN_ROOT + "/", "")}:${c.directive.line} — ${JSON.stringify(c.directive.text)}`,
      );
    }
  }

  console.log(`\n=== schema branches no prompt can reach (harness-wide) ===`);
  const dead = findDeadEnumBranches();
  total += dead.length;
  if (!dead.length) console.log("  none");
  for (const d of dead) {
    console.log(`\n  CONFLICT [${d.kind}]`);
    console.log(`    ${d.why}`);
    console.log(
      `    schema:  workflows/tribunal.js:${d.schema.line} — "${d.field}" enum [${d.members.join(", ")}]`,
    );
    console.log(
      `    branches: workflows/tribunal.js:${d.branchLines.join(", ")}`,
    );
  }

  console.log(
    `\n${total} cross-source conflict(s) across ${targets.length} agent(s)`,
  );
  // Reporting is the default because the conflicts it finds today are real and
  // open; a gate that fails on a known-open defect only trains people to skip
  // the gate. --strict is for the moment they are closed and you want them to
  // stay closed.
  process.exit(strict && total ? 1 : 0);
}
