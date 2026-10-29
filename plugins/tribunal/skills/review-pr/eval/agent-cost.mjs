// agent-cost.mjs — what a peer-review run actually cost.
//
// `budget.spent()` counts output tokens, and output is the minority term.
// Measured on two independent runs (PR #1911, 172 agents; PR #1968, 21 agents)
// the input side is 144x and 166x the output side respectively, because cost is
// `turns x context`: every tool result stays in the conversation and is re-read,
// and therefore re-billed, by every later turn. A 30-turn lens that read ten
// files pays for those ten files roughly twenty times. That makes cost
// quadratic in tool calls, and it makes the number of turns — not the reasoning
// tier — the variable worth attacking.
//
// The data is already on disk. Each workflow run has its own directory:
//
//   <session>/subagents/workflows/wf_<runId>/
//       journal.jsonl              — the authoritative agentId list for THIS run
//       agent-<id>.jsonl           — per-turn records, each with `usage`
//       agent-<id>.meta.json       — {agentType, spawnDepth, model}
//
// Two things this module is careful about, both learned the hard way:
//
//   1. Membership comes from journal.jsonl, never from globbing the directory.
//      A glob cannot tell this run's agents from leftovers.
//   2. These directories are DELETED. wf_87e1efc1-94d is already empty in the
//      live tree and survives only as a manual backup. Anything not copied into
//      metrics.jsonl is gone, which is why `agentCost` exists on the row.
//
// Imported by eval/analyze-agent-cost.mjs; covered by eval/replay-agent-cost.mjs.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, basename } from "node:path";

// Prompt openings as they actually appear in a transcript's first user message.
// Anchored at the start: these are the literal prefixes peer-review.js builds,
// and a substring match would let "REFUTE" inside a quoted finding reclassify a
// lens. Order matters only where one prefix could shadow another; none do today.
export const AGENT_PATTERNS = [
  // --- not peer-review at all -------------------------------------------------
  // Other workflows run in the same session tree and land in sibling wf_ dirs.
  // The harness self-audit is the big one: on this workspace it accounts for
  // ~69M input tokens across 35 agents. Pooling it with review runs would put
  // another workflow's cost inside a per-lens average, so it is matched FIRST
  // and marked as foreign rather than left to fall through to `unclassified`.
  { kind: "harness-audit", name: "audit-lens", re: /^You are auditing a personal Claude Code skill/ },
  { kind: "harness-audit", name: "audit-refuter", re: /^REFUTE — do not confirm — this audit finding/ },
  { kind: "harness-audit", name: "audit-refuter", re: /^REFUTE this root-cause diagnosis of a harness defect/ },
  { kind: "harness-audit", name: "audit-invariant", re: /^A self-audit of the peer-review harness failed an invariant/ },
  { kind: "harness-audit", name: "audit-invariant", re: /^A harness self-audit reported:/ },
  { kind: "harness-audit", name: "audit-fixer", re: /^Propose fixes for \d+ confirmed defect\(s\) in the peer-rev/ },
  { kind: "harness-audit", name: "audit-battery", re: /^Run the peer-review offline battery/ },
  { kind: "harness-audit", name: "audit-runner", re: /^Audit the most recent peer-review run/ },

  // --- peer-review agents -----------------------------------------------------
  { kind: "lens", name: "lens", re: /^Review PR #\S+ in repo / },
  // The pre-2026-07 lens prompt. Kept because the corpus on disk reaches back to
  // 2026-07-01 and dropping these would quietly shrink the population that
  // matters most: lenses are 44% of input.
  { kind: "lens", name: "lens-legacy", re: /^You are a code reviewer focusing ONLY on:/ },
  { kind: "refuter", name: "refute-single", re: /^REFUTE — do not confirm — this code review finding/ },
  { kind: "refuter", name: "refute-batch", re: /^You are refuting \d+ INDEPENDENT code-review findings on PR #/ },
  { kind: "external-heads", name: "heads-driver", re: /You will NOT be asked for a verdict and must not form one/ },
  // The Codex/Gemini slot wrappers. Their own tokens are Claude-side (the CLI
  // subprocess is billed elsewhere and invisible here), and at 153 agents /
  // 68M input they are far too big to leave uncategorised.
  { kind: "external-heads", name: "heads-codex", re: /^You are a model-diverse refuter \(Codex\/GPT slot\)/ },
  // The clean-verdict sweep's artifact read-back (2026-08-14): a mechanical cat
  // of the sweep's saved output. Cheap, but an unclassified agent is exactly
  // what this table exists to prevent — its driver already matches heads-driver.
  { kind: "external-heads", name: "sweep-artifact-read", re: /^Run exactly: cat / },
  { kind: "external-heads", name: "heads-gemini", re: /^You are a model-diverse refuter \(Gemini slot\)/ },
  { kind: "strengthen", name: "strengthen", re: /^A code-review finding on PR #\d+ .{0,40}was nearly refuted/ },
  { kind: "defense", name: "defense", re: /^DEFEND this code-review finding on PR #/ },
  { kind: "adjudicator", name: "adjudicator", re: /^You are the ADJUDICATOR \(judge lens\)/ },
  { kind: "dedup", name: "dedup-candidates", re: /^You are given candidate code-review findings from DIFFERENT review lenses/ },
  { kind: "dedup", name: "panel-ceiling-tiebreak", re: /^PANEL-CEILING TIE-BREAK\. This review has more findings/ },
  { kind: "bot-vetting", name: "bot-vetting", re: /^Vet these automated review comments on PR #/ },
  { kind: "critic", name: "completeness-critic", re: /^Completeness critic for PR #/ },
  { kind: "critic", name: "fidelity-critic", re: /^FIDELITY CRITIC for the peer-review report on PR #/ },
  // The incremental re-review's prior-finding verifier (agentType
  // "finding-verifier"). It has never had an entry: in the pooled cost table it
  // is the `unclassified` row — two agents whose tokens were attributed to
  // nothing. Found 2026-07-28 by replay-agent-classifier-coverage.mjs on its
  // first run, which is the gate paying for itself immediately.
  { kind: "verifier", name: "verify-prior-finding", re: /^A previous review of this PR raised the finding below/ },
  { kind: "synthesize", name: "synthesize", re: /^Synthesize the final peer review report for PR #/ },
  // `PR #\S+` not `PR #\d+`: a run from 2026-07-21 shows "Fetch PR #undefined"
  // (the arg never reached the script). That run still cost money and must be
  // counted, not dropped for having a broken prompt.
  { kind: "mechanical", name: "fetch-pr", re: /^Fetch PR #\S+ from GitHub repo/ },
  // Kept, and still named for the marker: the fused `context-bookkeeping`
  // dispatch falls back to exactly this prompt when --full or --head-sha closes
  // the probe half, and every historical run used it on its own.
  { kind: "mechanical", name: "mark-run-started", re: /^Best-effort bookkeeping —/ },
  // The incremental probe — fused with the marker as of the split below, and
  // never classified before it: on PR #1970 this is the single `unclassified`
  // agent in the byKind table, 107k input tokens attributed to nothing. Adding
  // it reclassifies historical runs from `unclassified` to `mechanical` the next
  // time their cost is recomputed. That is a correction, not a drift — the
  // stored agentCost on existing rows is frozen and unaffected.
  { kind: "mechanical", name: "context-bookkeeping", re: /^Mechanical, best-effort\. Decide whether this PR was already reviewed/ },
  // Both forms are kept. The dispatch gained a schema on 2026-09-08 and its
  // first line moved from "verbatim" to "as structured data"; historical runs
  // still carry the old wording and must stay attributable.
  { kind: "mechanical", name: "evidence-resolver", re: /^Run this bash EXACTLY and return its stdout verbatim/ },
  { kind: "mechanical", name: "evidence-resolver", re: /^Run this bash EXACTLY and return its stdout as structured data/ },
  // Fusion A/B (2026-07-28): availability probe + head worktree + grounded
  // typecheck now dispatch as ONE agent when panelCandidates exist — see
  // "check-external-heads" below in peer-review.js. Older/shorter forms below
  // stay for historical runs; each is its own pattern rather than three
  // regexes union'd, because a run predating the fusion could dispatch
  // setup-worktree/grounded-typecheck as separate agents and must still
  // attribute correctly.
  { kind: "mechanical", name: "check-external-heads", re: /^Run these commands and return their output verbatim, nothing else, in this exact order/ },
  { kind: "mechanical", name: "check-external-heads", re: /^Run these three commands and return their output verbatim/ },
  // Earlier, simpler forms of the same availability probe.
  { kind: "mechanical", name: "check-external-heads", re: /^Run `command -v (codex|gemini)/ },
  { kind: "mechanical", name: "setup-worktree", re: /^Create a detached git worktree/ },
  // 2026-09-11: the pinned head checkout moved EARLIER than the fusion above,
  // because the fused probe runs after the lens fan-out and lenses read source
  // too. Its own pattern rather than a widened one: a run that dispatches both
  // (this one early, the fused probe later reusing its path) has to attribute
  // each separately or the early one's cost lands on the wrong step.
  { kind: "mechanical", name: "setup-head-worktree", re: /^Run this bash EXACTLY and return the LAST line it prints, verbatim, as your entire answer/ },
  // 2026-09-11: resolves the identifiers a refutation names against the pinned
  // checkout, so a kill that rested on code absent at the reviewed commit can be
  // seen at all. Fires once per run, after the panel.
  { kind: "mechanical", name: "ref-evidence-check", re: /^Run this bash EXACTLY and return its output verbatim, nothing else\. It prints one MISSING line/ },
  { kind: "mechanical", name: "grounded-typecheck", re: /^Run a TypeScript typecheck for grounded verification/ },
  { kind: "mechanical", name: "classify-artifacts", re: /^Run one classification script and report its output EXACTLY/ },
  // Both spellings. The prompt said "a sequence of scripts" while it carried the
  // whole persist script inline; since 2026-07-28 that script lives in
  // lib/persist-run.sh and the prompt says "one script". The old form stays for
  // every historical run — and the new one is here because the rewrite silently
  // dropped this agent to `unclassified`, which is the SECOND time in one day a
  // prompt edit orphaned its own cost entry (the incremental probe was the first).
  { kind: "mechanical", name: "persist", re: /^Run (a sequence of scripts|one script) EXACTLY as given/ },
  { kind: "mechanical", name: "persist", re: /^Append one JSON line to a metrics log/ },
  // v81: the persist bodies are written as checksummed parts by one mechanic.
  { kind: "mechanical", name: "persist-parts", re: /^Mechanical step\. Run EXACTLY this bash and return its LAST line verbatim as your entire answer\. The only text you compose yourself is copying each heredoc body/ },
  // v92: one printf line per part instead of a heredoc.
  { kind: "mechanical", name: "persist-parts", re: /^Mechanical step\. Run EXACTLY this bash and return its LAST line verbatim as your entire answer\. Each `printf` line carries one part/ },
  { kind: "mechanical", name: "persist-parts", re: /^Mechanical step\. Run EACH of the \d+ bash block\(s\) below as its own Bash call/ },
  { kind: "mechanical", name: "reconcile", re: /^Run a vote-reconciliation script/ },
  { kind: "mechanical", name: "self-audit", re: /^Run an audit script and report its output EXACTLY/ },
  { kind: "mechanical", name: "verify-metrics-row", re: /^Return the last line of/ },
  // Singular and plural: the teardown is one fused dispatch now and says
  // "worktrees" when it removes both, so a regex anchored on the singular alone
  // would drop that agent from the cost table entirely on those runs.
  { kind: "mechanical", name: "teardown-worktree", re: /^Remove (a temporary git worktree|temporary git worktrees)/ },
  // v101: the first persist attempt writes the parts, persists and reads the row back in one dispatch.
  { kind: "mechanical", name: "persist-and-verify", re: /^Mechanical step in three parts\./ },
  // The four below go through relayAgent(), noteBashSize() or an array join, which
  // replay-agent-classifier-coverage did not read until 2026-10-23, so none had an entry.
  // Diff sections and blast radius share this opening; so do the evidence and anchor resolvers.
  { kind: "mechanical", name: "context-relay", re: /^Mechanical step\. Run the bash below and return its relay line\./ },
  { kind: "mechanical", name: "resolver-relay", re: /^Mechanical step\. Return the relay line of the bash below\./ },
  // The external-head slot driver and the clean-verdict sweep driver. Past 400
  // characters the unanchored heads-driver sentence is cut off the prompt head.
  { kind: "external-heads", name: "heads-driver", re: /^You are a DRIVER, not a reviewer\./ },
  { kind: "external-heads", name: "sweep-artifact-read", re: /^Run exactly this bash, then report what it printed\./ }
];

// Since about 2026-09-21 the Workflow runtime opens every subagent's first
// message with one line of its own ("[Workflow harness — computed task] ... The
// computed task text follows:") and indents the script's prompt under it. That
// line is 421 characters, longer than the 400-character promptHead, so every
// agent of every run read as `unclassified` until 2026-10-23. Only the opening
// line is removed; the indent goes with the whitespace collapse and trim.
const RUNTIME_FRAME = /^\s*\[Workflow harness[^\]\n]*\][^\n]*\n/;
export function stripRuntimeFrame(text) {
  return String(text || "").replace(RUNTIME_FRAME, "");
}

// Kinds that are NOT part of a peer-review run's cost. A run containing these
// and no review agents is another workflow that happened to share the session.
export const FOREIGN_KINDS = new Set(["harness-audit"]);

// Is this run a peer-review run? Decided by presence of review-side agents, not
// by absence of foreign ones: a review run whose lens prompt changes shape must
// not silently become "foreign", so the positive signal is what counts.
export function isPeerReviewRun(run) {
  return run.agents.some((a) => a.kind === "lens" || a.name === "fetch-pr");
}

// An unrecognised prompt is `unclassified` and stays that way. It is never
// folded into the nearest-looking bucket: a first pass at this classifier caught
// 1 lens out of 8 and swept the rest into "refuter", and a per-class breakdown
// carrying that error is worse than no breakdown at all. Callers must surface
// the unclassified count and its share of input.
// Every reviewer dispatch starts with the methodology line (workflows/tribunal.js,
// METHODOLOGY_LINE), and with conventions on, reviewers and refuters carry
// CONVENTIONS_LINE; the role is in what follows them.
export function classifyAgent(promptHead) {
  const s = stripRuntimeFrame(promptHead).trim().replace(/^(?:(?:METHODOLOGY|CONVENTIONS) FILE: \S+\s*)+/, "");
  for (const p of AGENT_PATTERNS) if (p.re.test(s)) return { kind: p.kind, name: p.name };
  return { kind: "unclassified", name: "unclassified" };
}

const firstUserText = (rec) => {
  const c = rec && rec.message && rec.message.content;
  if (typeof c === "string") return c;
  if (Array.isArray(c)) return c.map((x) => (x && typeof x.text === "string" ? x.text : "")).join(" ");
  return "";
};

// Pure: one transcript's raw text -> its cost. Kept separate from any fs access
// so the replay can feed it hand-built records.
export function parseTranscript(text) {
  const out = {
    turns: 0,
    outputTokens: 0,
    cacheRead: 0,
    cacheCreation: 0,
    freshInput: 0,
    peakContext: 0,
    efforts: {},
    promptHead: "",
    malformedLines: 0,
    toolUses: 0,
    toolTurns: 0
  };
  // Dedup key for turns/cacheRead/cacheCreation/freshInput: one assistant
  // MESSAGE is written as several JSONL LINES (thinking/tool_use/text blocks),
  // each repeating the same `usage` object. Summing per line instead of per
  // message.id inflated these fields 2-2.6x on real transcripts (verified
  // against two production agent-*.jsonl files, 2026-08-20). A line with no
  // message.id is counted on its own, same as every line was before this fix.
  //
  // output_tokens is a per-MESSAGE snapshot too, not a per-block count, so it
  // is taken as the max across the message's lines. Measured 2026-10-16
  // against the stream's own final usage: CLI 2.1.302 writes the final value
  // on every line ([6848, 6848] for a message whose true output was 6848),
  // and 2.1.266-2.1.299 wrote a placeholder first and the final value last
  // ([2, 2, 647]). Summing lines double-counted the first shape and added
  // the placeholder to the second.
  //
  // That fixes the arithmetic, not the source. A SUBAGENT transcript's
  // output_tokens is itself a placeholder on every message; only the main
  // loop's is real (Agent SDK docs: "Per-step `output_tokens` is a placeholder,
  // so read output tokens from the result message"). Measured 2026-10-17 on
  // CLI 2.1.302, subagents logged 6-48% of their real output while every input
  // field matched to the token. So outputTokens here is a LOWER BOUND, and
  // every figure built from it is one too; the input fields are exact.
  const seenMessages = new Set();
  const outputByMessage = new Map();
  const toolBlocksByMessage = new Map();
  let lineIndex = 0;
  for (const line of String(text || "").split("\n")) {
    lineIndex++;
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      out.malformedLines++;
      continue;
    }
    if (rec && rec.effort) out.efforts[rec.effort] = (out.efforts[rec.effort] || 0) + 1;
    if (!out.promptHead && rec && rec.type === "user") {
      const t = stripRuntimeFrame(firstUserText(rec)).replace(/\s+/g, " ").trim();
      if (t) out.promptHead = t.slice(0, 400);
    }
    const message = rec && rec.message;

    // toolUses/toolTurns: the batching ratio (tool calls per tool-bearing
    // MESSAGE, not per line) — counted independently of `usage`, since a
    // tool_use block can land on a line with no usage object at all.
    if (message && message.role === "assistant" && Array.isArray(message.content)) {
      const toolBlocks = message.content.filter((b) => b && b.type === "tool_use").length;
      if (toolBlocks > 0) {
        const key = message.id || `__line${lineIndex}`;
        toolBlocksByMessage.set(key, (toolBlocksByMessage.get(key) || 0) + toolBlocks);
      }
    }

    const u = (message && message.usage) || (rec && rec.usage);
    if (!u) continue;
    const messageId = message && message.id;
    const firstLineOfMessage = !messageId || !seenMessages.has(messageId);
    if (messageId) seenMessages.add(messageId);

    const lineOutput = u.output_tokens || 0;
    if (!messageId) {
      out.outputTokens += lineOutput;
    } else if (lineOutput > (outputByMessage.get(messageId) || 0)) {
      out.outputTokens += lineOutput - (outputByMessage.get(messageId) || 0);
      outputByMessage.set(messageId, lineOutput);
    }
    if (firstLineOfMessage) {
      out.turns++;
      out.cacheRead += u.cache_read_input_tokens || 0;
      out.cacheCreation += u.cache_creation_input_tokens || 0;
      out.freshInput += u.input_tokens || 0;
      const ctx =
        (u.cache_read_input_tokens || 0) + (u.cache_creation_input_tokens || 0) + (u.input_tokens || 0);
      if (ctx > out.peakContext) out.peakContext = ctx;
    }
  }
  out.inputTokens = out.cacheRead + out.cacheCreation + out.freshInput;
  out.toolTurns = toolBlocksByMessage.size;
  out.toolUses = [...toolBlocksByMessage.values()].reduce((a, b) => a + b, 0);
  out.toolUsesPerTurn = out.toolTurns ? Number((out.toolUses / out.toolTurns).toFixed(2)) : null;
  return out;
}

// Pure: journal text -> the agentIds this run dispatched, in first-seen order.
export function agentIdsFromJournal(text) {
  const ids = [];
  const seen = new Set();
  for (const line of String(text || "").split("\n")) {
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      continue;
    }
    const id = rec && rec.agentId;
    if (typeof id !== "string" || !id || seen.has(id)) continue;
    seen.add(id);
    ids.push(id);
  }
  return ids;
}

const emptyTotals = () => ({
  agents: 0,
  turns: 0,
  outputTokens: 0,
  inputTokens: 0,
  cacheRead: 0,
  cacheCreation: 0,
  freshInput: 0,
  toolUses: 0,
  toolTurns: 0
});

const addInto = (acc, a) => {
  acc.agents++;
  acc.turns += a.turns;
  acc.outputTokens += a.outputTokens;
  acc.inputTokens += a.inputTokens;
  acc.cacheRead += a.cacheRead;
  acc.cacheCreation += a.cacheCreation;
  acc.freshInput += a.freshInput;
  acc.toolUses += a.toolUses;
  acc.toolTurns += a.toolTurns;
  return acc;
};

// Reads one wf_<runId> directory. `missingTranscripts` is the count of agentIds
// the journal names but whose transcript is gone — reported, never silently
// treated as a zero-cost agent, because a deleted transcript and a free agent
// are the same number and opposite facts.
export function readRunDir(dir) {
  const journalPath = join(dir, "journal.jsonl");
  if (!existsSync(journalPath)) throw new Error(`no journal.jsonl in ${dir}`);
  const ids = agentIdsFromJournal(readFileSync(journalPath, "utf8"));

  const agents = [];
  let missingTranscripts = 0;
  for (const id of ids) {
    const tPath = join(dir, `agent-${id}.jsonl`);
    if (!existsSync(tPath)) {
      missingTranscripts++;
      continue;
    }
    const parsed = parseTranscript(readFileSync(tPath, "utf8"));
    const mPath = join(dir, `agent-${id}.meta.json`);
    let model = "unknown";
    if (existsSync(mPath)) {
      try {
        model = JSON.parse(readFileSync(mPath, "utf8")).model || "unknown";
      } catch {
        model = "unknown";
      }
    }
    const { kind, name } = classifyAgent(parsed.promptHead);
    agents.push({ agentId: id, model, kind, name, ...parsed });
  }

  const totals = emptyTotals();
  const byKind = {};
  const byModel = {};
  const byName = {};
  for (const a of agents) {
    addInto(totals, a);
    addInto((byKind[a.kind] = byKind[a.kind] || emptyTotals()), a);
    addInto((byModel[a.model] = byModel[a.model] || emptyTotals()), a);
    addInto((byName[a.name] = byName[a.name] || emptyTotals()), a);
  }
  return {
    runId: basename(dir),
    dir,
    journalAgentCount: ids.length,
    missingTranscripts,
    agents,
    totals,
    byKind,
    byModel,
    byName
  };
}

// Every wf_<runId> directory under a session tree that still has a journal.
export function findRunDirs(projectDir) {
  const out = [];
  let sessions = [];
  try {
    sessions = readdirSync(projectDir, { withFileTypes: true }).filter((d) => d.isDirectory());
  } catch {
    return out;
  }
  for (const s of sessions) {
    const wfRoot = join(projectDir, s.name, "subagents", "workflows");
    if (!existsSync(wfRoot)) continue;
    let runs = [];
    try {
      runs = readdirSync(wfRoot, { withFileTypes: true }).filter(
        (d) => d.isDirectory() && d.name.startsWith("wf_")
      );
    } catch {
      continue;
    }
    for (const r of runs) {
      const dir = join(wfRoot, r.name);
      if (existsSync(join(dir, "journal.jsonl"))) out.push(dir);
    }
  }
  return out;
}

// ASSUMPTIONS, not a bill. Public list prices in USD per million tokens, and
// the caller may override any of it. Two reasons this is never folded into a
// single "cost" number inside the aggregates: a run mixes model tiers, and
// prices change. What the measurement establishes is token volumes and turns;
// the dollars are an illustration of their ratio.
// opus is 2x sonnet: two identical `claude -p` probes on 2026-10-20 reported
// costUSD that priced claude-opus-5-5 at 2.005x claude-sonnet-5-5. The 5x it
// replaced was an older generation's ratio and inflated every opus arm.
export const DEFAULT_PRICES = {
  sonnet: { input: 3, cacheWrite: 3.75, cacheRead: 0.3, output: 15 },
  haiku: { input: 1, cacheWrite: 1.25, cacheRead: 0.1, output: 5 },
  opus: { input: 6, cacheWrite: 7.5, cacheRead: 0.6, output: 30 },
  unknown: { input: 3, cacheWrite: 3.75, cacheRead: 0.3, output: 15 }
};

// inputUsd is exact at the assumed prices. outputUsd is priced from transcript
// output, which is a lower bound (see parseTranscript), so outputUsd, totalUsd
// and outputShare are lower bounds too. outputUsd keeps its name for existing
// readers; outputUsdLowerBound is the same number under a name that says so.
// From v100 the judging roles pass a full model id ("claude-sonnet-5-5"), not
// the alias, and the transcript records whatever was passed. Price an id by the
// tier it names; an unrecognised id stays unpriced, as before.
export function priceTierOf(model) {
  const m = /\b(opus|sonnet|haiku)\b/.exec(String(model || "").replace(/[-_.]/g, " "));
  return m ? m[1] : null;
}

export function priceRun(byModel, prices = DEFAULT_PRICES) {
  const per = {};
  let inputUsd = 0;
  let outputUsd = 0;
  const unpriced = [];
  for (const [model, t] of Object.entries(byModel || {})) {
    const p = prices[model] || prices[priceTierOf(model)];
    if (!p) {
      unpriced.push(model);
      continue;
    }
    const i =
      (t.freshInput / 1e6) * p.input +
      (t.cacheCreation / 1e6) * p.cacheWrite +
      (t.cacheRead / 1e6) * p.cacheRead;
    const o = (t.outputTokens / 1e6) * p.output;
    per[model] = { inputUsd: i, outputUsd: o, outputUsdLowerBound: o, totalUsd: i + o };
    inputUsd += i;
    outputUsd += o;
  }
  const total = inputUsd + outputUsd;
  return {
    per,
    inputUsd,
    outputUsd,
    outputUsdLowerBound: outputUsd,
    totalUsd: total,
    outputShare: total > 0 ? outputUsd / total : 0,
    outputExact: false,
    unpriced
  };
}

// Units: list-price ratios with fresh input as 1, the same for every tier, so a
// run's units compare across model mixes without assuming a dollar price.
export const UNIT_WEIGHTS = { input: 1, cacheCreation: 1.25, cacheRead: 0.1, output: 5 };

const round2 = (x) => Math.round(x * 100) / 100;

// Exact: built only from the three input fields, never from output.
export function inputUnitsOf(t) {
  const { freshInput, cacheCreation, cacheRead } = t || {};
  if (![freshInput, cacheCreation, cacheRead].every(Number.isFinite)) return null;
  return round2(
    freshInput * UNIT_WEIGHTS.input +
      cacheCreation * UNIT_WEIGHTS.cacheCreation +
      cacheRead * UNIT_WEIGHTS.cacheRead
  );
}

// A stored agentCost -> its inputUnits. Rows written before 2026-10-17 lack the
// field but carry what it is made of: freshInput is inputTokens minus the two
// cache kinds, exactly as parseTranscript summed it.
export function rowInputUnits(agentCost) {
  if (!agentCost) return null;
  if (Number.isFinite(agentCost.inputUnits)) return agentCost.inputUnits;
  const { inputTokens, cacheRead, cacheCreation } = agentCost;
  if (![inputTokens, cacheRead, cacheCreation].every(Number.isFinite)) return null;
  return inputUnitsOf({ freshInput: inputTokens - cacheRead - cacheCreation, cacheCreation, cacheRead });
}

// The row-shaped summary Step 4 patches into metrics.jsonl. Deliberately small:
// per-agent detail dies with the transcripts, so what survives is the breakdown
// a later comparison actually needs.
export function toMetricsShape(run) {
  const pick = (t) => ({
    agents: t.agents,
    turns: t.turns,
    inputTokens: t.inputTokens,
    outputTokens: t.outputTokens,
    inputUnits: inputUnitsOf(t),
    toolUses: t.toolUses,
    toolTurns: t.toolTurns,
    toolUsesPerTurn: t.toolTurns ? Number((t.toolUses / t.toolTurns).toFixed(2)) : null
  });
  const byKind = {};
  for (const [k, v] of Object.entries(run.byKind)) byKind[k] = pick(v);
  const byModel = {};
  for (const [k, v] of Object.entries(run.byModel)) byModel[k] = pick(v);
  const u = run.byKind.unclassified;
  return {
    runId: run.runId,
    turns: run.totals.turns,
    inputTokens: run.totals.inputTokens,
    outputTokens: run.totals.outputTokens,
    inputUnits: inputUnitsOf(run.totals),
    // outputTokens comes from subagent transcripts, whose output_tokens is a
    // placeholder: a lower bound, not a measurement.
    outputExact: false,
    cacheRead: run.totals.cacheRead,
    cacheCreation: run.totals.cacheCreation,
    agents: run.totals.agents,
    missingTranscripts: run.missingTranscripts,
    toolUses: run.totals.toolUses,
    toolTurns: run.totals.toolTurns,
    toolUsesPerTurn: run.totals.toolTurns
      ? Number((run.totals.toolUses / run.totals.toolTurns).toFixed(2))
      : null,
    byKind,
    byModel,
    unclassified: { agents: u ? u.agents : 0, inputTokens: u ? u.inputTokens : 0 }
  };
}
