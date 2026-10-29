#!/usr/bin/env node
// replay-assembled-prompt.mjs — verify the cross-source conflict detector by
// building trees whose conflicts we already know, not by trusting its output
// on the real one.
//
// WHY THE FIXTURES MATTER MORE THAN USUAL HERE. This detector was wrong twice
// before it was right, and both times it looked right:
//
//   · it flagged `source: "claude"` in the Claude refuter's prompt as making
//     "codex" and "gemini" unreachable. They are produced by the external-CLI
//     path; the field names who is voting and is correctly fixed. The question
//     has to be asked harness-wide, never per agent.
//   · it then flagged `decision: "SURVIVES"` as unreachable. The adjudicator
//     prompt names it — the prompt extractor was skipping any agent() call
//     with a comment between the paren and the backtick, so real prompts went
//     missing while the count still looked plausible.
//
// A detector whose failures look like findings is worse than none, so every
// rule below is exercised against a tree that must trip it and a tree that
// must not.
//
// Usage: node eval/replay-assembled-prompt.mjs

import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  PLUGIN_ROOT,
  agentNames,
  assembleAgentPrompt,
  dispatchTemplates,
  findContradictions,
  findDeadEnumBranches,
  methodologyRefs,
  promptTemplatesInWorkflow,
} from "./assemble-agent-prompt.mjs";
import { readFileSync } from "node:fs";
import { WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- a throwaway plugin tree, shaped like the real one ---------------------
function buildTree({ methodology, agentBody, workflow }) {
  const root = mkdtempSync(join(tmpdir(), "assembled-prompt-"));
  for (const d of ["agents", "methodology", "workflows", "skills/review-pr"])
    mkdirSync(join(root, d), { recursive: true });
  writeFileSync(
    join(root, "agents", "lens.md"),
    `---\nname: lens\ntools: Read, Grep, Glob, Bash\n---\n\n${agentBody || "Read `methodology/m.md` and follow it exactly."}\n`,
  );
  writeFileSync(join(root, "methodology", "m.md"), methodology || "Review the diff you were given.\n");
  writeFileSync(join(root, "workflows", "tribunal.js"), workflow || "const x = 1;\n");
  writeFileSync(
    join(root, "skills", "review-pr", "SKILL.md"),
    "- Every read of source used to confirm or refute a finding must be pinned to the PR head ref. Do not grep the shared local clone.\n",
  );
  return root;
}

console.log(`\n--- assembly follows the runtime reference chain ---`);
{
  const root = buildTree({});
  const a = assembleAgentPrompt("lens", root);
  check(
    "the methodology the body names is pulled in as its own source",
    a.sources.some((s) => s.origin.startsWith("methodology")),
    a.sources.map((s) => s.origin).join(" | "),
  );
  check(
    "the entry skill is carried as harness-wide context",
    a.sources.some((s) => s.origin.startsWith("entry skill")),
  );
  check(
    "methodologyRefs finds a reference written inline in prose",
    methodologyRefs("Read `methodology/code-review-methodology.md` first").length === 1,
  );
  check(
    "an agent naming no methodology gets no phantom source",
    assembleAgentPrompt("lens", buildTree({ agentBody: "Just review it." })).sources.every(
      (s) => !s.origin.startsWith("methodology"),
    ),
  );
}

console.log(`\n--- rule in one file vs directive in another ---`);
{
  const conflicting = buildTree({
    methodology: "## Scope\n\n```bash\ngit diff --name-only\ngit status\n```\n",
  });
  const clean = buildTree({
    methodology: "## Scope\n\nRead the diff you were handed. Resolve anything else with `git show <sha>:<path>`.\n",
  });

  const hits = findContradictions(assembleAgentPrompt("lens", conflicting));
  check("fires when a pinning rule meets a working-tree command", hits.length === 1, JSON.stringify(hits));
  check(
    "names both sides with a file and a line",
    hits.length === 1 &&
      hits[0].rule.line > 0 &&
      hits[0].directive.line > 0 &&
      hits[0].rule.path !== hits[0].directive.path,
    JSON.stringify(hits[0] || {}),
  );
  check(
    "silent when the methodology uses the pinned read form",
    findContradictions(assembleAgentPrompt("lens", clean)).length === 0,
  );
  check(
    "a pinned `git show <sha>:<path>` is not mistaken for an unpinned read",
    findContradictions(
      assembleAgentPrompt(
        "lens",
        buildTree({ methodology: "Run `git diff a1b2c3d4 -- file.ts` only.\n" }),
      ),
    ).length === 0,
  );
}

console.log(`\n--- schema branch no prompt can reach ---`);
{
  const enumDecl = `const S = { properties: { slot: { type: "string", enum: ["OK", "STUCK"] } } };`;
  const branch = `if (v.slot === "STUCK") abstain(v);`;

  const unreachable = buildTree({
    workflow: `${enumDecl}\n${branch}\nawait agent(\`Return slot: "OK".\`, {});\n`,
  });
  const reachable = buildTree({
    workflow: `${enumDecl}\n${branch}\nawait agent(\`Return slot: "OK", or "STUCK" if you could not read the file.\`, {});\n`,
  });
  const declaredNotUsed = buildTree({
    workflow: `${enumDecl}\nawait agent(\`Return slot: "OK".\`, {});\n`,
  });

  const hits = findDeadEnumBranches(unreachable);
  check(
    "fires when the workflow branches on a value no prompt offers",
    hits.length === 1 && hits[0].unreachable === "STUCK",
    JSON.stringify(hits),
  );
  check(
    "silent once a prompt tells the agent it may emit that value",
    findDeadEnumBranches(reachable).length === 0,
  );
  check(
    "silent when nothing branches on the unreachable member (declared, unused, harmless)",
    findDeadEnumBranches(declaredNotUsed).length === 0,
  );
  check(
    "a prompt behind a comment still counts — the regression that produced the SURVIVES false positive",
    findDeadEnumBranches(
      buildTree({
        workflow: `${enumDecl}\n${branch}\nawait agent(\n  // hoisted for cache ordering\n  \`Return slot: "OK" or "STUCK".\`,\n  {},\n);\n`,
      }),
    ).length === 0,
  );
}

console.log(`\n--- the shipping tree parses and the extractors reach it ---`);
{
  const src = readFileSync(join(PLUGIN_ROOT, "workflows", "tribunal.js"), "utf8");
  const prompts = promptTemplatesInWorkflow(src);
  check("every agent definition assembles without throwing", (() => {
    try {
      for (const n of agentNames()) assembleAgentPrompt(n);
      return true;
    } catch (e) {
      return String(e);
    }
  })() === true);
  check(
    "the prompt extractor reaches most of the workflow's agent() call sites",
    prompts.length >= 15,
    `${prompts.length} extracted`,
  );
  check(
    "the extracted prompts are real prompt text, not slices of code",
    prompts.filter((p) => p.text.length > 200).length >= 10,
  );
  check(
    "dispatchTemplates resolves an agentType written as a constant",
    dispatchTemplates(src, "finding-verifier").length >= 1,
  );
}

console.log(`\n--- no prompt overrides the tool rule ---`);
{
  // A lens prompt that said "Read it first (`cat <patch>`)" (until 2026-10-19)
  // had its lenses obey the prompt over the methodology in every A/B run.
  // A mechanic's bare `cat` (no argument) is a heredoc instruction, not this.
  const SHELL_READ_IN_PROSE = /\\`(cat|sed -n|head) [^`>]+\\`/;
  const src = readFileSync(WORKFLOW, "utf8");
  const told = src.split("\n").filter((l) => !l.trimStart().startsWith("//") && SHELL_READ_IN_PROSE.test(l));
  check("no prompt in tribunal.js tells an agent to read a file with cat, sed -n or head", told.length === 0, told.map((l) => l.trim().slice(0, 100)).join(" | "));
}

console.log(`\n--- the scoped exemption is narrow, not a mute button ---`);
{
  // Added 2026-09-08 with the TD-15 fix. The methodology serves two callers and
  // only one of them must pin, so a directive that states which caller it
  // applies to is reconciled rather than contradictory. That is a real
  // distinction and also an obvious way to silence a detector, so it gets a
  // must-fire and a must-not-fire like every other rule here.
  const UNPINNED = "**Default:**\n```\ngit diff --name-only\ngit status\n```\n";
  const EXEMPT =
    "Dispatched by the harness: do not run the commands below; pin every read to the head commit. PINNED-READ EXCEPTION APPLIES.\n\n" +
    UNPINNED +
    "\nInvoked directly by a person: the working tree IS the subject.\n";

  check(
    "an unqualified working-tree command still conflicts",
    findContradictions(assembleAgentPrompt("lens", buildTree({ methodology: UNPINNED })))
      .length === 1,
  );
  check(
    "the same command with the scoped exemption does not",
    findContradictions(assembleAgentPrompt("lens", buildTree({ methodology: EXEMPT })))
      .length === 0,
  );
  check(
    "prose that merely sounds careful does NOT exempt — the marker is literal",
    findContradictions(
      assembleAgentPrompt(
        "lens",
        buildTree({
          methodology:
            "Be careful about which branch you read; pinning is usually better.\n\n" + UNPINNED,
        }),
      ),
    ).length === 1,
  );
  check(
    "the marker alone, with no working-tree command, invents no exemption to report",
    findContradictions(
      assembleAgentPrompt(
        "lens",
        buildTree({ methodology: "PINNED-READ EXCEPTION APPLIES. Read at the head commit.\n" }),
      ),
    ).length === 0,
  );
}

console.log(`\n--- regression floor: the shipping tree, against a named baseline ---`);
{
  // The checks above test the DETECTOR and are deliberately independent of what
  // the live tree happens to contain, which is why they stayed green through
  // the 2026-09-07 refuter fix. That independence leaves a hole: nothing here
  // failed when the tree was broken, and nothing would fail if it broke again.
  // This block closes it without pinning a number that goes stale — it names
  // the conflicts known to be open, so a NEW one turns this red while the
  // known-open ones do not.
  //
  // Known open, both the same defect: the mandatory methodology hands every
  // lens `git diff` / `git status` against the local checkout while
  // SKILL.md:363 requires every read to be pinned to the PR head ref. That is
  // confirmed finding 4 (TD-15) of the 2026-09-04 concept audit, still unfixed
  // because closing it means rewriting the procedure both lens families run.
  // Empty since 2026-09-08. It held the two halves of TD-15 — the BE and FE
  // methodologies handing every lens `git diff` / `git status` against the local
  // checkout while SKILL.md required every read pinned to the PR head ref — and
  // both were closed by naming, in the methodology itself, which caller each
  // mode belongs to. An empty baseline is the strong state: any conflict at all
  // is now a regression, and nothing here is grandfathered.
  const KNOWN_OPEN = [];
  const rel = (p) => p.replace(`${PLUGIN_ROOT}/`, "");
  const live = agentNames()
    .flatMap((n) => findContradictions(assembleAgentPrompt(n)))
    .map((c) => `${c.kind}@${rel(c.directive.path)}`);
  const unexpected = [...new Set(live.filter((k) => !KNOWN_OPEN.includes(k)))];
  const disappeared = KNOWN_OPEN.filter((k) => !live.includes(k));

  check(
    "no cross-source conflict beyond the ones known to be open",
    unexpected.length === 0,
    unexpected.join(", "),
  );
  check(
    "the known-open conflicts are still there — if one vanished, fix this baseline",
    disappeared.length === 0,
    disappeared.join(", "),
  );
  check(
    "the baseline is empty, so nothing is grandfathered",
    KNOWN_OPEN.length === 0,
    `${KNOWN_OPEN.length} conflict(s) still tolerated`,
  );

  // Closed on 2026-09-07. Three prompts (the agent definition and both inline
  // refuter tails) hardcoded `slotStatus: "OK"`, so REFUTATION_SCHEMA's
  // deliberately built UNAVAILABLE + failureReason: "confined" abstention was
  // declared, consumed by tallyVotes and recordHealthFromVotes, and reachable
  // by nothing. An enum branch no prompt can name is a promise the harness
  // cannot keep, and this is the assertion that keeps it kept.
  // TD-19, closed as far as it can be. The read-only premise lived only in the
  // methodology prose, which finding-verifier does not read at all, while all
  // four definitions grant an unrestricted `Bash`. No per-agent mechanical
  // surface exists to close the rest — hooks and permission rules are
  // session-scoped and the session has to post the review — so what is
  // enforceable is that every definition states the limit in the same words.
  // tribunal-mechanic is the one agent whose job is to write (worktrees, the
  // metrics row), so the read-only sentence would be false there; it states
  // its own limit instead, also in fixed words.
  const READ_ONLY = "Use the shell for reading and nothing else.";
  const MECHANIC_LIMIT = "Use the shell for the commands your prompt gives you and nothing else.";
  const toolsOf = (n) => {
    const m = readFileSync(join(PLUGIN_ROOT, "agents", `${n}.md`), "utf8").match(/^tools:\s*(.*)$/m);
    return m ? m[1].split(",").map((t) => t.trim()).filter(Boolean) : null;
  };
  const grantsBash = agentNames().filter((n) => (toolsOf(n) || []).includes("Bash"));
  const missingLimit = grantsBash.filter((n) => {
    const body = readFileSync(join(PLUGIN_ROOT, "agents", `${n}.md`), "utf8");
    return !body.includes(n === "tribunal-mechanic" ? MECHANIC_LIMIT : READ_ONLY);
  });
  check(
    "every agent definition that grants Bash states what its Bash grant is for and is not for",
    missingLimit.length === 0,
    missingLimit.join(", "),
  );
  // The scribe is the one agent with no shell: its dispatches never called a
  // tool, and Read is held only because an empty allowlist does not launch.
  // Pinning the set keeps the check above from going quiet: an agent that
  // loses Bash by accident shows up here instead of silently leaving it.
  const NO_SHELL = ["tribunal-scribe"];
  const withoutBash = agentNames().filter((n) => !grantsBash.includes(n));
  check(
    "…and the agents without Bash are exactly the ones meant to have none",
    withoutBash.join(",") === NO_SHELL.join(","),
    withoutBash.join(", "),
  );
  check("every agent definition declares a tools allowlist", agentNames().every((n) => toolsOf(n) !== null));

  // Claude Code's tools reference (code.claude.com/docs/en/tools-reference,
  // "Glob tool behavior"): on macOS, Linux and WSL a subagent gets Glob and
  // Grep back only when it lists them AND leaves out Bash. Listed beside Bash
  // they resolve to nothing, so the frontmatter would promise tools the agent
  // never has. Search on disk goes through the shell's grep and find there.
  const inert = agentNames().filter((n) => {
    const t = toolsOf(n) || [];
    return t.includes("Bash") && (t.includes("Grep") || t.includes("Glob"));
  });
  check("no agent declares Grep or Glob beside Bash, where they never resolve", inert.length === 0, inert.join(", "));

  // One rule for which tool reads what, in the same words wherever an agent
  // has both Read and the shell: in its own body, or in the methodology file
  // its body sends it to. The words are pinned so the copies cannot drift.
  const TOOL_RULE = "Choose the tool by where the file is.";
  const readers = grantsBash.filter((n) => (toolsOf(n) || []).includes("Read"));
  const noRule = readers.filter(
    (n) => !assembleAgentPrompt(n).sources.some((src) => !src.origin.startsWith("dispatch template") && src.text.includes(TOOL_RULE)),
  );
  check("every agent with Read and Bash is told which tool reads what", noRule.length === 0, noRule.join(", "));
  check("…and that is not vacuous: such agents exist", readers.length >= 4, `${readers.length}`);

  const dead = findDeadEnumBranches();
  check(
    "no schema enum branch is unreachable from every prompt in the tree",
    dead.length === 0,
    dead.map((d) => `${d.unreachable} (${d.sites || "?"} site(s))`).join(", "),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall assembled-prompt expectations hold");
