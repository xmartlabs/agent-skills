#!/usr/bin/env node
// relay-census.mjs — how lossy are the LLM relays inside a Workflow run?
//
// Tribunal is a Workflow script: plain JS with no filesystem. Data
// reaches disk only when an agent retypes it into a Bash tool call (A-relay,
// JS -> disk), and script output reaches the JS only when an agent copies it
// into its StructuredOutput call (B-relay, disk -> JS). Both copies are done by
// a model, so both can drop, invent or alter content. This script measures
// that from stored subagent transcripts. Analysis only: it is never part of a
// harness data path, and costs 0 tokens.
//
//   node eval/relay-census.mjs                      — projectDir() from paths.mjs
//   node eval/relay-census.mjs --project <dir>      — every wf_ run dir under it
//   node eval/relay-census.mjs --dir <wf_...> [--dir ...]
//   node eval/relay-census.mjs --json               — machine-readable output
//   node eval/relay-census.mjs --self-test
//
// A-relay: each heredoc body in the agent's prompt is matched by tag to a
// heredoc body in its Bash commands; `exact` is byte equality after removing
// the prompt's common indent. B-relay: the Bash tool_result JSON sharing the
// most key paths with the StructuredOutput input is diffed by key path
// (dropped / invented) and by exact path (changed scalar values). With no JSON
// tool_result, a long SO string is checked as a verbatim span of a result.
//
// outputTokens is a LOWER BOUND: subagent transcripts log placeholder output
// tokens. inputUnits is exact.

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { readRunDir, findRunDirs, inputUnitsOf } from "./agent-cost.mjs";
import { projectDir, isMainModule } from "./paths.mjs";

const bytes = (s) => Buffer.byteLength(String(s ?? ""), "utf8");
const trunc = (v, n = 80) => {
  const s = typeof v === "string" ? v : JSON.stringify(v);
  return s.length > n ? `${s.slice(0, n)}…` : s;
};
const median = (xs) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

// --- A-relays: heredocs ------------------------------------------------------

const leadingWs = (l) => l.match(/^[ \t]*/)[0];

export function dedent(text) {
  const lines = String(text).split("\n");
  let min = Infinity;
  for (const l of lines) if (l.trim()) min = Math.min(min, leadingWs(l).length);
  if (!Number.isFinite(min) || min === 0) return String(text);
  return lines.map((l) => (l.trim() ? l.slice(min) : l.replace(/^[ \t]*/, ""))).join("\n");
}

const HEREDOC_RE = /(?<!<)<<(?!<)-?[ \t]*(?:'([^'\n]+)'|"([^"\n]+)"|([A-Za-z_][A-Za-z0-9_]*))/g;

// Only heredocs whose terminator line (exactly the tag, whitespace allowed)
// exists are returned: a `<<'EOF'` quoted in prose has none.
export function extractHeredocs(text) {
  const lines = String(text).split("\n");
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    const tags = [...lines[i].matchAll(HEREDOC_RE)].map((m) => m[1] || m[2] || m[3]);
    if (!tags.length) continue;
    let j = i + 1;
    for (const tag of tags) {
      const body = [];
      let k = j;
      while (k < lines.length && lines[k].trim() !== tag) body.push(lines[k++]);
      if (k >= lines.length) continue;
      out.push({ tag, body: body.join("\n"), terminatorIndent: leadingWs(lines[k]) });
      j = k + 1;
    }
    i = j - 1;
  }
  return out;
}

const stripIndent = (body, indent) =>
  indent
    ? body
        .split("\n")
        .map((l) => (l.startsWith(indent) ? l.slice(indent.length) : l))
        .join("\n")
    : body;

const commonPrefixLen = (a, b) => {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return i;
};

export function compareHeredocs(promptText, commands) {
  const fromPrompt = extractHeredocs(dedent(promptText));
  const fromCmds = commands.flatMap((c) => extractHeredocs(c));
  const used = new Set();
  return fromPrompt.map((p) => {
    const variants = [...new Set([p.body, stripIndent(p.body, p.terminatorIndent), dedent(p.body)])];
    const same = fromCmds.map((c, idx) => ({ c, idx })).filter((x) => x.c.tag === p.tag);
    const pool = same.filter((x) => !used.has(x.idx));
    const cands = pool.length ? pool : same;
    let pick = cands.find((x) => variants.includes(x.c.body));
    if (!pick && cands.length) {
      pick = cands.reduce((best, x) =>
        commonPrefixLen(x.c.body, p.body) > commonPrefixLen(best.c.body, p.body) ? x : best
      );
    }
    if (pick) used.add(pick.idx);
    return {
      tag: p.tag,
      promptBytes: bytes(p.body),
      copiedBytes: pick ? bytes(pick.c.body) : 0,
      exact: !!pick && variants.includes(pick.c.body),
      found: !!pick
    };
  });
}

// --- B-relays: tool_result JSON vs StructuredOutput ---------------------------

// End index (exclusive) of the JSON value starting at `start`, string-aware.
function balancedEnd(text, start) {
  let depth = 0;
  let inStr = false;
  for (let i = start; i < text.length; i++) {
    const ch = text[i];
    if (inStr) {
      if (ch === "\\") i++;
      else if (ch === '"') inStr = false;
    } else if (ch === '"') inStr = true;
    else if (ch === "{" || ch === "[") depth++;
    else if (ch === "}" || ch === "]") {
      depth--;
      if (depth === 0) return i + 1;
    }
  }
  return -1;
}

// Whole text if it is JSON, otherwise every JSON object/array that starts a line.
export function jsonCandidates(text) {
  const t = String(text).trim();
  if (/^[{[]/.test(t)) {
    try {
      return [JSON.parse(t)];
    } catch {
      // fall through to the line scan
    }
  }
  const out = [];
  let pos = 0;
  let skipUntil = 0;
  for (const line of String(text).split("\n")) {
    const lineStart = pos;
    pos += line.length + 1;
    if (lineStart < skipUntil) continue;
    const lead = leadingWs(line).length;
    if (line[lead] !== "{" && line[lead] !== "[") continue;
    const start = lineStart + lead;
    const end = balancedEnd(text, start);
    if (end < 0) continue;
    try {
      out.push(JSON.parse(text.slice(start, end)));
      skipUntil = end;
    } catch {
      // not JSON
    }
  }
  return out;
}

function collect(value, path, exact, paths, leaves) {
  if (value !== null && typeof value === "object") {
    if (Array.isArray(value)) {
      value.forEach((v, i) => {
        const p = `${path}[]`;
        paths.set(p, (paths.get(p) || 0) + 1);
        collect(v, p, `${exact}[${i}]`, paths, leaves);
      });
    } else {
      for (const [k, v] of Object.entries(value)) {
        const p = `${path}.${k}`;
        paths.set(p, (paths.get(p) || 0) + 1);
        collect(v, p, `${exact}.${k}`, paths, leaves);
      }
    }
  } else {
    leaves.set(exact, value);
  }
}

const mapsOf = (value) => {
  const paths = new Map();
  const leaves = new Map();
  collect(value, "$", "$", paths, leaves);
  return { paths, leaves };
};

export function sharedPathCount(a, b) {
  const pa = mapsOf(a).paths;
  let n = 0;
  for (const p of mapsOf(b).paths.keys()) if (pa.has(p)) n++;
  return n;
}

const side = (map, other) => {
  const missing = [...map].filter(([p]) => !other.has(p));
  return {
    paths: missing.length,
    occurrences: missing.reduce((s, [, c]) => s + c, 0),
    examples: missing.slice(0, 5).map(([p]) => p)
  };
};

// `stdout` is the parsed JSON a script printed; `so` the StructuredOutput input.
export function diffJson(stdout, so) {
  const a = mapsOf(stdout);
  const b = mapsOf(so);
  const changed = [];
  for (const [p, v] of a.leaves) {
    if (b.leaves.has(p) && JSON.stringify(b.leaves.get(p)) !== JSON.stringify(v)) {
      changed.push(`${p}: ${trunc(v)} -> ${trunc(b.leaves.get(p))}`);
    }
  }
  let shared = 0;
  for (const p of b.paths.keys()) if (a.paths.has(p)) shared++;
  const union = a.paths.size + b.paths.size - shared;
  return {
    shared,
    jaccard: union ? shared / union : 1,
    dropped: side(a.paths, b.paths),
    invented: side(b.paths, a.paths),
    changed: { count: changed.length, examples: changed.slice(0, 5) }
  };
}

const longStrings = (value, min, out = []) => {
  if (typeof value === "string") {
    if (value.length >= min) out.push(value);
  } else if (value && typeof value === "object") {
    for (const v of Object.values(value)) longStrings(v, min, out);
  }
  return out;
};

// Is a long SO string a relay of some tool result's text, and was it verbatim?
// Null when no result shares even a sampled 80-char chunk with it: then the
// string was written by the agent, not relayed, and "inexact" would be noise.
export function textCopyOf(so, resultTexts) {
  let best = null;
  for (const s of longStrings(so, 500)) {
    const exact = resultTexts.some((t) => t.includes(s) || t.trim() === s.trim());
    const chunks = [0, (s.length >> 1) - 40, s.length - 80].map((i) => s.slice(Math.max(0, i), Math.max(0, i) + 80));
    const related = exact || resultTexts.some((t) => chunks.some((c) => t.includes(c)));
    if (related && (!best || bytes(s) > best.bytes)) best = { bytes: bytes(s), exact };
  }
  return best;
}

export function compareRelayB(so, resultTexts) {
  let best = null;
  for (const text of resultTexts) {
    for (const cand of jsonCandidates(text)) {
      const shared = sharedPathCount(cand, so);
      if (shared > 0 && (!best || shared > best.shared)) best = { cand, shared };
    }
  }
  if (best) {
    const stdoutBytes = bytes(JSON.stringify(best.cand));
    const structuredBytes = bytes(JSON.stringify(so));
    return {
      kind: "json",
      stdoutBytes,
      structuredBytes,
      ratio: stdoutBytes ? structuredBytes / stdoutBytes : null,
      ...diffJson(best.cand, so)
    };
  }
  const tc = textCopyOf(so, resultTexts);
  return tc ? { kind: "text", textCopy: tc } : null;
}

// --- transcript extraction -----------------------------------------------------

const blockText = (c) =>
  typeof c === "string" ? c : Array.isArray(c) ? c.map((b) => (b && typeof b.text === "string" ? b.text : "")).join("\n") : "";

export function readAgentTranscript(text) {
  const out = { prompt: null, model: null, commands: [], bashResults: [], structured: null };
  const toolName = new Map();
  for (const line of String(text).split("\n")) {
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      continue;
    }
    const m = rec && rec.message;
    if (!m) continue;
    if (rec.type === "user" && out.prompt === null) {
      const c = m.content;
      const isResult = Array.isArray(c) && c.some((b) => b && b.type === "tool_result");
      if (!isResult) out.prompt = blockText(c);
    }
    if (m.role === "assistant") {
      if (!out.model && m.model) out.model = m.model;
      for (const b of Array.isArray(m.content) ? m.content : []) {
        if (!b || b.type !== "tool_use") continue;
        toolName.set(b.id, b.name);
        if (b.name === "Bash" && b.input && typeof b.input.command === "string") out.commands.push(b.input.command);
        if (String(b.name).includes("StructuredOutput")) out.structured = b.input;
      }
    } else if (rec.type === "user" && Array.isArray(m.content)) {
      for (const b of m.content) {
        if (b && b.type === "tool_result" && toolName.get(b.tool_use_id) === "Bash") {
          out.bashResults.push(blockText(b.content));
        }
      }
    }
  }
  return out;
}

// A verified relay answer is {relayCrc, data}; the copy to compare is `data`.
const relayed = (so) =>
  so && typeof so === "object" && "relayCrc" in so && "data" in so ? so.data : so;

export function analyzeTranscript(text) {
  const t = readAgentTranscript(text);
  return {
    model: t.model,
    A: t.prompt ? compareHeredocs(t.prompt, t.commands) : [],
    B: t.structured != null ? compareRelayB(relayed(t.structured), t.bashResults) : null,
    hasStructured: t.structured != null
  };
}

const shortModel = (m) => String(m || "unknown").replace(/^claude-/, "").replace(/-\d{8}$/, "");

export function analyzeRunDir(dir) {
  const run = readRunDir(dir);
  const agents = run.agents.map((a) => {
    const tPath = join(dir, `agent-${a.agentId}.jsonl`);
    const r = analyzeTranscript(readFileSync(tPath, "utf8"));
    return {
      agentId: a.agentId,
      label: `${a.kind}/${a.name}`,
      promptHead: a.promptHead.slice(0, 60),
      model: shortModel(r.model || a.model),
      A: r.A,
      B: r.B,
      hasStructured: r.hasStructured,
      outputTokens: a.outputTokens,
      inputUnits: inputUnitsOf(a)
    };
  });
  return { runId: run.runId, dir, missingTranscripts: run.missingTranscripts, agents };
}

// --- reporting -------------------------------------------------------------------

// A B match below this key-path overlap is a transformation (the agent wrote a
// different object that happens to share a few keys), not a relay of the output.
const RELAY_JACCARD = 0.5;

const kb = (n) => `${(n / 1024).toFixed(1)}KB`;
const hasDiff = (b) =>
  b && b.kind === "json" && (b.dropped.paths > 0 || b.invented.paths > 0 || b.changed.count > 0);

const aCell = (A) => {
  if (!A.length) return "-";
  const found = A.filter((x) => x.found).length;
  const exact = A.filter((x) => x.exact).length;
  return `${exact}/${found} of ${A.length} (max ${kb(Math.max(...A.map((x) => x.copiedBytes)))})`;
};
const bCell = (B) => {
  if (!B) return "-";
  if (B.kind === "text") return `text ${kb(B.textCopy.bytes)} ${B.textCopy.exact ? "exact" : "INEXACT"}`;
  return `${kb(B.stdoutBytes)}->${kb(B.structuredBytes)} d${B.dropped.paths} i${B.invented.paths} c${B.changed.count} J${B.jaccard.toFixed(2)}`;
};

export function aggregate(runs) {
  const byLabel = {};
  for (const r of runs) {
    for (const a of r.agents) {
      const t = (byLabel[a.label] ||= { agents: 0, a: 0, aExact: 0, aMissing: 0, b: 0, bRelay: 0, bDiff: 0, ratios: [], text: 0, textExact: 0 });
      t.agents++;
      t.a += a.A.length;
      t.aExact += a.A.filter((x) => x.exact).length;
      t.aMissing += a.A.filter((x) => !x.found).length;
      if (a.B && a.B.kind === "json") {
        t.b++;
        if (a.B.jaccard >= RELAY_JACCARD) {
          t.bRelay++;
          if (hasDiff(a.B)) t.bDiff++;
          if (a.B.ratio != null) t.ratios.push(a.B.ratio);
        }
      } else if (a.B) {
        t.text++;
        if (a.B.textCopy.exact) t.textExact++;
      }
    }
  }
  return Object.fromEntries(
    Object.entries(byLabel).map(([k, v]) => [k, { ...v, medianRatio: median(v.ratios) }])
  );
}

const pad = (s, n) => String(s).padEnd(n).slice(0, Math.max(n, String(s).length));

function printText(runs) {
  for (const r of runs) {
    const relayed = r.agents.filter((a) => a.A.length || a.B);
    console.log(`\n${r.runId}  (${r.agents.length} agents, ${relayed.length} with relays, ${r.missingTranscripts} missing transcripts)`);
    if (!relayed.length) continue;
    const rows = relayed.map((a) => [a.label, a.model, aCell(a.A), bCell(a.B), a.outputTokens.toLocaleString("en-US")]);
    const head = ["label", "model", "A: exact/copied of n (max KB)", "B: stdout->SO, dropped/invented/changed, J=key-path overlap", "outTok(lb: placeholder in subagent logs)"];
    const w = head.map((h, i) => Math.max(h.length, ...rows.map((x) => String(x[i]).length)));
    const fmt = (x) => x.map((c, i) => pad(c, w[i])).join(" | ");
    console.log(fmt(head));
    for (const x of rows) console.log(fmt(x));
  }
  const tot = aggregate(runs);
  console.log("\nTOTALS across all runs");
  const head = ["label", "agents", "A-relays", "A exact", "A not copied", "B json", "B relay (J>=0.5)", "relay w/ diffs", "median ratio", "B text (exact)"];
  const rows = Object.entries(tot)
    .filter(([, v]) => v.a || v.b || v.text)
    .sort((x, y) => y[1].a + y[1].bRelay - (x[1].a + x[1].bRelay))
    .map(([k, v]) => [k, v.agents, v.a, v.aExact, v.aMissing, v.b, v.bRelay, v.bDiff, v.medianRatio == null ? "-" : v.medianRatio.toFixed(2), `${v.text} (${v.textExact})`]);
  const w = head.map((h, i) => Math.max(h.length, ...rows.map((x) => String(x[i]).length)));
  const fmt = (x) => x.map((c, i) => pad(c, w[i])).join(" | ");
  console.log(fmt(head));
  for (const x of rows) console.log(fmt(x));
}

// --- self-test ---------------------------------------------------------------------

export function selfTest() {
  const fails = [];
  const check = (name, got, want) => {
    if (JSON.stringify(got) !== JSON.stringify(want)) fails.push(`${name}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
  };
  const body = 'line one\n  "quoted" $VAR\nlast';
  const prompt = `Run this:\ncat > /tmp/x.json <<'EOF'\n${body}\nEOF\nthen done`;

  const exact = compareHeredocs(prompt, [`mkdir -p d\ncat > /tmp/x.json <<'EOF'\n${body}\nEOF\necho ok`]);
  check("exact heredoc", exact.map((x) => [x.tag, x.exact, x.found, x.promptBytes === x.copiedBytes]), [["EOF", true, true, true]]);

  const changed = compareHeredocs(prompt, [`cat > /tmp/x.json <<'EOF'\n${body.replace("one", "onf")}\nEOF`]);
  check("one char changed", changed.map((x) => [x.exact, x.found, x.promptBytes === x.copiedBytes]), [[false, true, true]]);

  const missing = compareHeredocs(prompt, ["echo nothing"]);
  check("not copied", missing.map((x) => [x.exact, x.found, x.copiedBytes]), [[false, false, 0]]);

  const indented = prompt.split("\n").map((l) => `    ${l}`).join("\n");
  const ind = compareHeredocs(indented, [`cat <<'EOF'\n${body}\nEOF`]);
  check("indented prompt", ind.map((x) => [x.exact, x.found, x.promptBytes]), [[true, true, bytes(body)]]);

  const prose = extractHeredocs("write it with <<'EOF' then stop");
  check("unterminated heredoc in prose ignored", prose, []);

  const stdout = { items: [{ id: 1, name: "a", sev: "high" }, { id: 2, name: "b", sev: "low" }], total: 2 };
  const so = { items: [{ id: 1, name: "a", sev: "HIGH" }, { id: 2, name: "b", sev: "low" }], note: "added" };
  const d = diffJson(stdout, so);
  check("dropped paths", d.dropped.examples, ["$.total"]);
  check("invented paths", d.invented.examples, ["$.note"]);
  check("changed", d.changed.count, 1);
  check("changed example", d.changed.examples[0], "$.items[0].sev: high -> HIGH");
  check("jaccard", [d.shared, Number(d.jaccard.toFixed(2))], [5, 0.71]);
  check("clean diff", diffJson(stdout, JSON.parse(JSON.stringify(stdout))).changed.count, 0);

  const viaText = compareRelayB(so, ["noise\n" + JSON.stringify(stdout, null, 2) + "\n"]);
  check("json from pretty multi-line result", viaText && [viaText.kind, viaText.dropped.paths, viaText.invented.paths], ["json", 1, 1]);

  const big = "x".repeat(300) + " relayed text " + "y".repeat(300);
  const tcExact = compareRelayB({ report: big }, [`header\n${big}\nfooter`]);
  check("text copy exact", tcExact, { kind: "text", textCopy: { bytes: bytes(big), exact: true } });
  const altered = big.slice(0, 100) + "Z" + big.slice(101);
  const tcBad = compareRelayB({ report: altered }, [`header\n${big}\nfooter`]);
  check("text copy altered", tcBad, { kind: "text", textCopy: { bytes: bytes(altered), exact: false } });
  check("unrelated long string is not a relay", compareRelayB({ r: "q".repeat(600) }, [big]), null);

  return fails;
}

// --- main ----------------------------------------------------------------------------

if (isMainModule(import.meta.url)) {
  const argv = process.argv.slice(2);
  if (argv.includes("--self-test")) {
    const fails = selfTest();
    if (fails.length) {
      console.error(`self-test FAILED (${fails.length}):\n  ${fails.join("\n  ")}`);
      process.exit(1);
    }
    console.log("self-test passed");
    process.exit(0);
  }
  const valuesOf = (flag) => argv.flatMap((a, i) => (a === flag && argv[i + 1] ? [argv[i + 1]] : []));
  let dirs = valuesOf("--dir");
  const projects = valuesOf("--project");
  if (!dirs.length && !projects.length) projects.push(projectDir());
  for (const p of projects) dirs.push(...findRunDirs(p));
  dirs = [...new Set(dirs)];

  const runs = [];
  for (const d of dirs) {
    if (!existsSync(join(d, "journal.jsonl"))) {
      console.error(`skipping ${d}: no journal.jsonl`);
      continue;
    }
    runs.push(analyzeRunDir(d));
  }
  if (argv.includes("--json")) {
    console.log(JSON.stringify({ runDirs: runs.length, runs, totals: aggregate(runs) }, null, 2));
  } else {
    console.log(`${runs.length} run dir(s)`);
    printText(runs);
  }
}
