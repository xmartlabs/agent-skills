// refine-attribution.mjs — recover the ROUND-0 tally that the metrics row throws
// away, so the refine gate can be decided on data instead of on a guess.
//
// The problem. `needsRefine(tally)` fires on `tally.coreStands || tally.borderline`,
// and those two signals rescue at very different rates — so the obvious change is
// to drop the weaker disjunct. But the row cannot answer which signal fired: when
// a finding refines, the panel is REPLACED (peer-review.js builds a fresh
// round-1 panel and Phase 3 recomputes the tally at round 1), so the row's
// `refuteFraction` / `borderline` / `votes` describe the state AFTER the refine.
// 23 of 62 recorded refines are therefore unattributable from metrics.jsonl, and
// the 39 that look attributable are only being read through a post-refine tally
// that may not resemble the one the gate actually saw.
//
// The data is still on disk. Every run keeps a journal:
//
//   <session>/subagents/workflows/wf_<runId>/
//       journal.jsonl        — {type:"result", agentId, result} — the agent's
//                              FULL return value, including every round-0 vote
//       agent-<id>.jsonl     — first user message = the prompt, which names the
//                              finding each vote belongs to, and a timestamp
//
// So round 0 is reconstructible: pair each vote with its finding via the prompt,
// order by dispatch time, and cut at the strengthen agent — everything before it
// is what the gate saw, escalation votes included (the gate runs on the
// post-escalation tally, not the initial one).
//
// Why this is trustworthy. The reconstruction is checked against an INDEPENDENT
// oracle rather than against itself: for every finding that did NOT refine, the
// row still holds the real round-0 tally, and the reconstruction has to
// reproduce its refuteFraction, borderline and survived. Agreement on those is
// what licenses the numbers for the 62 that refined. `--verbose` prints the
// disagreements.
//
// Scope, stated up front: transcripts get deleted, so this reaches only the runs
// still on disk. It reports how many recorded refines it could and could NOT
// reach, and the second number is never rounded to zero.
//
// Usage: node eval/refine-attribution.mjs [--verbose] [--json]

import { readFileSync, existsSync, readdirSync } from "node:fs";
import { canonicalRows, METRICS_PATH } from "./rows.mjs";
import { dirname, join, resolve, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { findRunDirs } from "./agent-cost.mjs";
import { isMainModule } from "./paths.mjs";
import { WORKFLOW, projectDir } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
// METRICS_PATH, not a locally rebuilt path: the log lives outside the plugin
// directory (which updates replace wholesale). Rebuilding it here pointed at a
// file that no longer exists — found 2026-07-30 by widening replay-rows.mjs to
// treat CONSTRUCTING the path as reading it.
const METRICS = METRICS_PATH;
const PROJECT = projectDir();

// ---------------------------------------------------------------------------
// Pure parsers. Every one of these reads a prompt string that peer-review.js
// built, so they are pinned to that file's wording — replay-refine-attribution
// asserts each against a literal sample, and a wording change must break there
// rather than silently reconstruct nothing.
// ---------------------------------------------------------------------------

export const normTitle = (s) =>
  String(s || "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();

// `file` in a prompt carries an optional `:line`. The line is not part of a
// finding's identity here — a strengthen pass may move it — so it is stripped.
export const normFile = (s) =>
  String(s || "")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/:\d+$/, "")
    .toLowerCase();

export const findingKey = (title, file) => `${normTitle(title)}@@${normFile(file)}`;

// Batched Claude refuter: one dispatch, N findings, each announced with its
// slotId. The slotId is the FINDING's id (peer-review.js keys
// escalationVoteBySlot / claudeRevoteBySlot by `finding.slotId`), not a vote id.
export function parseRefuterBatchCases(prompt) {
  const out = [];
  const re =
    /=====\s*CASE\s+\d+\s+of\s+\d+\s+—\s+slot\s+(\d+)\s*=====\s*\nFinding:\s*(.*)\nFile:\s*(.*)\nSeverity:\s*(.*)/g;
  let m;
  while ((m = re.exec(String(prompt || "")))) {
    out.push({
      slotId: Number(m[1]),
      title: m[2].trim(),
      file: m[3].trim(),
      severity: m[4].trim()
    });
  }
  return out;
}

// Unbatched Claude refuter: the finding block appears once, after the diff. The
// diff itself can contain a line starting with "Finding:" only if the PR does,
// so the anchor is the Finding/File/Severity triple, not "Finding:" alone.
export function parseSingleRefuterCase(prompt) {
  const m = /\nFinding:\s*(.*)\nFile:\s*(.*)\nSeverity:\s*(.*)/.exec(String(prompt || ""));
  if (!m) return null;
  return { slotId: null, title: m[1].trim(), file: m[2].trim(), severity: m[3].trim() };
}

// Codex/Gemini slot wrapper. Two prompt generations exist on disk and both have
// to parse, because the older one is most of the external votes in the corpus:
// the current form writes `<votes-dir>/<slot>-<source>.prompt` (slot recoverable,
// and it is the same key classify-artifacts uses), the pre-2026-07-21 form wrote
// a fixed `/tmp/codex_refute_prompt.txt` with no slot at all. So the slot is
// OPTIONAL and the (title, file) pair is what joins a vote to its finding.
// Requiring the slot is what made this return null for 102 external votes —
// dropping, in every case, the third voter on a 3-vote panel, which is why the
// oracle disagreed with the row on 30 findings.
export function parseExternalSlot(prompt) {
  const s = String(prompt || "");
  const f = /\nFinding:\s*(.*)\nFile:\s*(.*)/.exec(s);
  if (!f) return null;
  const slot = /peer-review-votes-[^/\s]*\/(\d+)-(codex|gemini)\.prompt/.exec(s);
  const header = /^You are a model-diverse refuter \((Codex\/GPT|Gemini) slot\)/.exec(s.trim());
  const source = slot ? slot[2] : header && header[1] === "Gemini" ? "gemini" : "codex";
  return {
    slotId: slot ? Number(slot[1]) : null,
    source,
    title: f[1].trim(),
    file: f[2].trim()
  };
}

export function parseStrengthenTarget(prompt) {
  const m = /\nOriginal finding:\s*(.*)\nFile:\s*(.*)\nSeverity:\s*(.*)/.exec(String(prompt || ""));
  if (!m) return null;
  return { title: m[1].trim(), file: m[2].trim(), severity: m[3].trim() };
}

// A journal result is either one vote or a batch of them. Normalised to a list
// of {slotId|null, vote} so the caller never branches on dispatch shape.
export function votesFromResult(result) {
  if (!result || typeof result !== "object") return [];
  if (Array.isArray(result.verdicts))
    return result.verdicts
      .filter((v) => v && typeof v === "object")
      .map((v) => ({ slotId: typeof v.slotId === "number" ? v.slotId : null, vote: v }));
  if (typeof result.verdict === "string") return [{ slotId: null, vote: result }];
  return [];
}

// ---------------------------------------------------------------------------
// Assembly: agent records -> per-finding vote timeline.
//
// An `agent` here is {agentId, role, ts, cases, votes} where `role` is one of
// claude-refuter | external | strengthen and `cases` came from its prompt.
// Pure, so the replay can hand it synthetic runs.
// ---------------------------------------------------------------------------

// Returns { findings, aliases }.
//
// `aliases` maps the STRENGTHENED identity of a finding back to its original
// one, and without it the whole reconstruction misses on exactly the population
// it exists to measure. A strengthen pass returns a new title/file and
// peer-review.js carries `workingFinding` forward from there (`finding:
// workingFinding` at the report boundary), so a refined finding appears in the
// row under a title that no round-0 refuter prompt ever contained. Looking it up
// directly finds only its round-1 votes — which is how a refine can appear to
// have "survived round 0", a contradiction rather than a measurement.
export function assembleTimeline(agents) {
  const findings = new Map();
  const aliases = new Map();
  const touch = (key, title, file) => {
    if (!findings.has(key))
      findings.set(key, { key, title, file, slotIds: new Set(), votes: [], strengthenTs: null });
    return findings.get(key);
  };

  for (const a of agents) {
    if (a.role === "strengthen") {
      for (const c of a.cases) {
        const key = findingKey(c.title, c.file);
        const e = touch(key, c.title, c.file);
        // Earliest strengthen wins: the cut has to sit at the FIRST one, or a
        // second pass would pull round-1 votes back into round 0.
        if (e.strengthenTs === null || a.ts < e.strengthenTs) e.strengthenTs = a.ts;
        if (a.strengthenedTo && a.strengthenedTo.title) {
          const newKey = findingKey(a.strengthenedTo.title, a.strengthenedTo.file || c.file);
          if (newKey !== key) aliases.set(newKey, key);
        }
      }
      continue;
    }
    if (a.role !== "claude-refuter" && a.role !== "external") continue;

    // Pair votes to cases. A batch names its slotIds on both sides, so match on
    // those; an unbatched dispatch has exactly one case and one vote.
    const bySlot = new Map();
    for (const c of a.cases) if (c.slotId !== null) bySlot.set(c.slotId, c);
    for (const { slotId, vote } of a.votes) {
      let c = slotId !== null ? bySlot.get(slotId) : null;
      if (!c && a.cases.length === 1) c = a.cases[0];
      if (!c) continue; // a vote we cannot attribute is dropped, and counted by the caller
      const e = touch(findingKey(c.title, c.file), c.title, c.file);
      if (c.slotId !== null) e.slotIds.add(c.slotId);
      e.votes.push({ ts: a.ts, agentId: a.agentId, role: a.role, vote });
    }
  }
  for (const e of findings.values()) e.votes.sort((x, y) => (x.ts < y.ts ? -1 : x.ts > y.ts ? 1 : 0));
  return { findings, aliases };
}

// Look a row's finding up in the timeline, following the strengthen alias when
// the row records the post-refine identity.
export function lookupFinding(timeline, title, file) {
  const key = findingKey(title, file);
  const direct = timeline.findings.get(key);
  const aliased = timeline.aliases.has(key)
    ? timeline.findings.get(timeline.aliases.get(key))
    : null;
  return { entry: aliased || direct || null, viaAlias: !!aliased };
}

// The votes the refine gate actually saw: everything cast before the strengthen
// pass. Escalation votes are INCLUDED — peer-review.js evaluates needsRefine on
// the post-escalation tally, so excluding them would replay a tally the gate
// never read.
export function roundZeroVotes(entry) {
  if (!entry) return [];
  if (entry.strengthenTs === null) return entry.votes.map((v) => v.vote);
  return entry.votes.filter((v) => v.ts < entry.strengthenTs).map((v) => v.vote);
}

// Which signal made needsRefine true. Both can hold; "neither" means the
// reconstruction disagrees with the fact that a refine happened, and that is a
// finding about the reconstruction, not about the gate.
export function attributeRefine(tally) {
  if (!tally) return "unreconstructed";
  const core = !!tally.coreStands;
  const border = !!tally.borderline;
  if (core && border) return "both";
  if (core) return "coreStands-only";
  if (border) return "borderline-only";
  return "neither";
}

// ---------------------------------------------------------------------------
// fs shell
// ---------------------------------------------------------------------------

// The metrics row a run wrote, taken from the run itself. The persist agent is
// handed the row as a heredoc body ("copy the METRICSJSON heredoc verbatim"), so
// the transcript contains the exact record that run produced — one JSON object
// on one line, beginning at `{"schemaVersion"`.
//
// This replaces matching runs to metrics.jsonl rows by (pr, headSha) + nearest
// timestamp, which is unreliable in exactly the cases that matter: PR #1935 has
// ten rows at ONE headSha inside a few hours, so a ±90-minute nearest-match
// silently validated one run's reconstruction against another run's panel. That
// mis-pairing, not the parsers, was most of the oracle's disagreement.
export function parseEmbeddedRow(prompt) {
  for (const line of String(prompt || "").split("\n")) {
    const t = line.trim();
    if (!t.startsWith('{"schemaVersion"')) continue;
    try {
      return JSON.parse(t);
    } catch {
      return null; // truncated in the transcript — treated as absent, never half-read
    }
  }
  return null;
}

const ROLE_PATTERNS = [
  { role: "claude-refuter", batch: true, re: /^You are refuting \d+ INDEPENDENT code-review findings on PR #/ },
  {
    role: "claude-refuter",
    batch: false,
    re: /^REFUTE — do not confirm — this code review finding/
  },
  { role: "external", re: /^You are a model-diverse refuter \((Codex\/GPT|Gemini) slot\)/ },
  { role: "strengthen", re: /^A code-review finding on PR #\d+ .{0,60}was nearly refuted/ },
  { role: "fetch-pr", re: /^Fetch PR #\S+ from GitHub repo/ },
  { role: "persist", re: /^Run a sequence of scripts EXACTLY as given/ },
  { role: "persist", re: /^Append one JSON line to a metrics log/ }
];

function firstUserPrompt(text) {
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      continue;
    }
    if (rec.type !== "user") continue;
    const c = rec.message && rec.message.content;
    const s =
      typeof c === "string"
        ? c
        : Array.isArray(c)
          ? c.map((x) => (x && typeof x.text === "string" ? x.text : "")).join(" ")
          : "";
    if (s.trim()) return { prompt: s, ts: rec.timestamp || "" };
  }
  return { prompt: "", ts: "" };
}

function lastTimestamp(text) {
  let last = "";
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      const r = JSON.parse(line);
      if (r.timestamp && r.timestamp > last) last = r.timestamp;
    } catch {
      /* a truncated line has no timestamp to offer */
    }
  }
  return last;
}

export function readRun(dir) {
  const journalPath = join(dir, "journal.jsonl");
  if (!existsSync(journalPath)) return null;
  const results = new Map();
  for (const line of readFileSync(journalPath, "utf8").split("\n")) {
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      continue;
    }
    if (rec.type === "result" && rec.agentId) results.set(rec.agentId, rec.result);
  }

  const agents = [];
  let pr = null;
  let headSha = null;
  let endTs = "";
  let unattributableVotes = 0;
  let embeddedRow = null;
  let prFiles = [];
  for (const f of readdirSync(dir)) {
    const m = /^agent-(\w+)\.jsonl$/.exec(f);
    if (!m) continue;
    const agentId = m[1];
    const text = readFileSync(join(dir, f), "utf8");
    const { prompt, ts } = firstUserPrompt(text);
    const lt = lastTimestamp(text);
    if (lt > endTs) endTs = lt;
    const head = prompt.trim();
    const hit = ROLE_PATTERNS.find((p) => p.re.test(head));
    if (!hit) continue;

    if (hit.role === "persist") {
      const r = parseEmbeddedRow(prompt);
      // Several persist dispatches can exist (a retry, a resumed run). The one
      // that carries findings is the real record; an earlier partial write is
      // not allowed to shadow it.
      if (r && (!embeddedRow || (r.findings || []).length > (embeddedRow.findings || []).length))
        embeddedRow = r;
      continue;
    }

    if (hit.role === "fetch-pr") {
      const r = results.get(agentId);
      const nm = /Fetch PR #(\d+)/.exec(head);
      if (nm) pr = Number(nm[1]);
      if (r && typeof r === "object" && typeof r.headRefOid === "string") headSha = r.headRefOid;
      // The PR's real changed-file list. Consumers that reconstruct what the
      // harness knew at dispatch need this exactly as the run had it —
      // approximating it from the row's finding paths is a much shorter list and
      // silently biases any "was this file in the PR" test.
      if (r && typeof r === "object" && Array.isArray(r.files)) prFiles = r.files.filter((f) => typeof f === "string");
      continue;
    }

    let cases = [];
    if (hit.role === "claude-refuter")
      cases = hit.batch
        ? parseRefuterBatchCases(prompt)
        : [parseSingleRefuterCase(prompt)].filter(Boolean);
    else if (hit.role === "external") cases = [parseExternalSlot(prompt)].filter(Boolean);
    else if (hit.role === "strengthen") cases = [parseStrengthenTarget(prompt)].filter(Boolean);

    const votes = hit.role === "strengthen" ? [] : votesFromResult(results.get(agentId));
    if (votes.length && !cases.length) unattributableVotes += votes.length;
    let strengthenedTo = null;
    if (hit.role === "strengthen") {
      const r = results.get(agentId);
      if (r && typeof r === "object" && typeof r.title === "string")
        strengthenedTo = { title: r.title, file: r.file };
    }
    agents.push({ agentId, role: hit.role, ts, cases, votes, strengthenedTo });
  }

  return {
    runId: basename(dir),
    dir,
    pr: pr ?? (embeddedRow ? embeddedRow.pr : null),
    headSha: headSha ?? (embeddedRow ? embeddedRow.headSha : null),
    endTs,
    agents,
    unattributableVotes,
    embeddedRow,
    prFiles
  };
}

// Runs are matched to rows on (pr, headSha) and then by nearest end-of-run time,
// one-to-one. `rerunOf` exists precisely because several runs share a pr+sha, so
// the timestamp is what separates them — and a match further away than
// MATCH_WINDOW_MIN is refused rather than forced, because a wrong pairing would
// validate the reconstruction against somebody else's panel.
const MATCH_WINDOW_MIN = 90;

export function matchRunsToRows(runs, rows) {
  const pairs = [];
  const takenRows = new Set();
  const candidates = [];
  for (const run of runs) {
    if (!run.pr || !run.endTs) continue;
    for (let i = 0; i < rows.length; i++) {
      const row = rows[i];
      if (row.pr !== run.pr) continue;
      if (run.headSha && row.headSha && row.headSha !== run.headSha) continue;
      if (!row.ts) continue;
      const dmin = Math.abs(new Date(row.ts) - new Date(run.endTs)) / 60000;
      if (!Number.isFinite(dmin) || dmin > MATCH_WINDOW_MIN) continue;
      candidates.push({ run, rowIndex: i, dmin });
    }
  }
  candidates.sort((a, b) => a.dmin - b.dmin);
  const takenRuns = new Set();
  for (const c of candidates) {
    if (takenRuns.has(c.run.runId) || takenRows.has(c.rowIndex)) continue;
    takenRuns.add(c.run.runId);
    takenRows.add(c.rowIndex);
    pairs.push({ run: c.run, row: rows[c.rowIndex], rowIndex: c.rowIndex, dmin: c.dmin });
  }
  return pairs;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

if (isMainModule(import.meta.url)) {
  const VERBOSE = process.argv.includes("--verbose");
  const AS_JSON = process.argv.includes("--json");
  const { api } = loadRegion(WORKFLOW, "PANEL-GATES", ["tallyVotes", "needsRefine", "T_KILL", "DELTA"]);

  // canonicalRows(): one definition of comparable — see eval/rows.mjs.
  const rows = canonicalRows();

  const runs = findRunDirs(PROJECT)
    .map(readRun)
    .filter((r) => r && r.agents.length);
  // Only runs that carry their OWN row are paired. A run whose persist prompt is
  // gone is reported as unreached rather than matched by timestamp — see
  // parseEmbeddedRow for why the fuzzy match had to go.
  const pairs = runs.filter((r) => r.embeddedRow).map((r) => ({ run: r, row: r.embeddedRow }));
  const runsWithoutRow = runs.filter((r) => !r.embeddedRow);

  const isClaude = (v) => !v.source || v.source === "claude";

  // Two oracles, because the reconstruction is trustworthy for one signal and
  // only partial for the other, and collapsing them into one percentage would
  // hide which is which.
  //
  //   A — coreStands. Reconstructed from the Claude votes, which come back
  //       through the journal as the refuter's own return value. Measured over
  //       the corpus, 183 of 184 coreStands votes are Claude's (one codex vote
  //       is the exception), so this is the signal the gate change turns on and
  //       it is reconstructible end to end.
  //   B — refuteFraction / borderline. Needs the EXTERNAL votes too, and those
  //       do not come back the same way: a Codex/Gemini wrapper can crash while
  //       its artifact still holds a valid vote, which classifyArtifacts then
  //       recovers (`voteCorrections` on the row). The journal has the wrapper's
  //       self-report, not the corrected truth, so B is expected to disagree and
  //       is reported as coverage, not as a pass/fail.
  const oracleA = { checked: 0, agree: 0, misses: [] };
  const oracleB = { checked: 0, agree: 0, misses: [] };

  const refines = [];
  const rowRefinesTotal = rows.reduce(
    (n, r) => n + (r.findings || []).filter((f) => f.refined).length,
    0
  );

  for (const { run, row } of pairs) {
    const timeline = assembleTimeline(run.agents);
    for (const f of row.findings || []) {
      const { entry, viaAlias } = lookupFinding(timeline, f.title, f.file);
      const votes = roundZeroVotes(entry);
      const claudeVotes = votes.filter(isClaude);
      const tally = votes.length ? api.tallyVotes(votes, votes.map(() => ({ source: null }))) : null;
      const coreStands = votes.some((v) => v.coreStands || v.refutationType === "TECHNICALLY_WRONG");

      if (!f.refined) {
        if (!tally) continue;
        const rowClaude = (f.votes || []).filter(isClaude);
        const rowCore = rowClaude.some((v) => v.coreStands);
        oracleA.checked++;
        if (claudeVotes.length === rowClaude.length && coreStands === rowCore) oracleA.agree++;
        else
          oracleA.misses.push({
            pr: row.pr,
            title: f.title.slice(0, 58),
            got: { claude: claudeVotes.length, core: coreStands },
            row: { claude: rowClaude.length, core: rowCore }
          });

        oracleB.checked++;
        if (
          Math.abs(tally.refuteFraction - (f.refuteFraction ?? -1)) < 0.005 &&
          tally.borderline === !!f.borderline
        )
          oracleB.agree++;
        else
          oracleB.misses.push({
            pr: row.pr,
            title: f.title.slice(0, 58),
            got: { rf: +tally.refuteFraction.toFixed(3), b: tally.borderline, n: votes.length },
            row: { rf: f.refuteFraction, b: !!f.borderline, n: (f.votes || []).length }
          });
        continue;
      }

      refines.push({
        pr: row.pr,
        title: f.title,
        reached: !!entry,
        viaAlias,
        coreStands,
        explicitFlag: votes.some((v) => v.coreStands),
        technicallyWrong: votes.some((v) => v.refutationType === "TECHNICALLY_WRONG"),
        borderline: tally ? tally.borderline : null,
        rescued: !!f.survived,
        finalSeverity: f.severity,
        r0ClaudeVotes: claudeVotes.length,
        r0RefuteFraction: tally ? +tally.refuteFraction.toFixed(3) : null
      });
    }
  }

  const reached = refines.filter((r) => r.reached);

  if (AS_JSON) {
    console.log(JSON.stringify({ oracleA, oracleB, refines }, null, 2));
    process.exit(0);
  }

  const pct = (n, d) => (d ? `${((100 * n) / d).toFixed(0)}%` : "n/a");

  console.log(`\nRUNS ON DISK`);
  console.log(`  wf_ dirs with review agents : ${runs.length}`);
  console.log(`  carrying their own row      : ${pairs.length}`);
  console.log(`  no persist prompt on disk   : ${runsWithoutRow.length} (excluded, not matched by time)`);
  console.log(
    `  votes seen but unattributable to a finding : ${runs.reduce((n, r) => n + r.unattributableVotes, 0)}`
  );

  console.log(`\nORACLE A — coreStands, on findings that never refined (the signal the change turns on)`);
  console.log(`  checked : ${oracleA.checked}`);
  console.log(`  agree   : ${oracleA.agree} (${pct(oracleA.agree, oracleA.checked)})`);
  if (oracleA.misses.length && VERBOSE)
    for (const m of oracleA.misses.slice(0, 25))
      console.log(
        `    #${m.pr} ${m.title}\n      got ${m.got.claude} claude vote(s), coreStands=${m.got.core}` +
          `\n      row ${m.row.claude} claude vote(s), coreStands=${m.row.core}`
      );

  console.log(`\nORACLE B — refuteFraction / borderline (needs external votes; partial by construction)`);
  console.log(`  checked : ${oracleB.checked}`);
  console.log(`  agree   : ${oracleB.agree} (${pct(oracleB.agree, oracleB.checked)})`);
  console.log(`  The gap is the external side: a crashed wrapper's vote lives in its artifact,`);
  console.log(`  which classifyArtifacts recovers into the row but the journal never sees.`);

  console.log(`\nREFINES`);
  console.log(`  recorded in metrics.jsonl : ${rowRefinesTotal}`);
  console.log(`  in a run still on disk    : ${refines.length}`);
  console.log(`  round 0 reconstructed     : ${reached.length} (${pct(reached.length, rowRefinesTotal)})`);
  console.log(`  NOT reconstructed         : ${rowRefinesTotal - reached.length}`);
  console.log(`  resolved through the strengthen alias : ${reached.filter((r) => r.viaAlias).length}`);

  const buckets = {
    "coreStands (survives the change)": reached.filter((r) => r.coreStands),
    "borderline only (dropped)": reached.filter((r) => !r.coreStands && r.borderline),
    "neither — reconstruction incomplete": reached.filter((r) => !r.coreStands && !r.borderline)
  };
  console.log(`\n  round-0 signal                        fires  rescues   rate`);
  for (const [k, v] of Object.entries(buckets))
    console.log(
      `  ${k.padEnd(37)} ${String(v.length).padStart(5)} ${String(v.filter((r) => r.rescued).length).padStart(8)}   ` +
        `${pct(v.filter((r) => r.rescued).length, v.length)}`
    );

  // coreStands is a UNION of two things in tallyVotes: a refuter's explicit
  // `coreStands` flag, and a `TECHNICALLY_WRONG` refutation type. If one half
  // rescued far worse than the other, THAT would be the disjunct worth dropping
  // — a different change from the planned one, but derived from the same data.
  const twOnly = reached.filter((r) => !r.explicitFlag && r.technicallyWrong);
  const explicit = reached.filter((r) => r.explicitFlag);
  console.log(`\n  what made coreStands true             fires  rescues   rate   ended as nit`);
  for (const [k, v] of [
    ["explicit coreStands flag", explicit],
    ["TECHNICALLY_WRONG only", twOnly]
  ])
    console.log(
      `  ${k.padEnd(37)} ${String(v.length).padStart(5)} ${String(v.filter((r) => r.rescued).length).padStart(8)}   ` +
        `${pct(v.filter((r) => r.rescued).length, v.length)}   ${pct(v.filter((r) => r.finalSeverity === "nit").length, v.length)}`
    );

  const dropped = reached.filter((r) => !r.coreStands);
  const lost = dropped.filter((r) => r.rescued);
  const rescues = reached.filter((r) => r.rescued).length;
  console.log(`\n  IF needsRefine REQUIRED coreStands (dropping the borderline disjunct)`);
  console.log(`    still refine : ${reached.filter((r) => r.coreStands).length}`);
  console.log(`    no longer    : ${dropped.length}`);
  console.log(`    rescues lost : ${lost.length} of ${rescues}`);
  if (VERBOSE && lost.length)
    for (const r of lost) console.log(`      #${r.pr} rf=${r.r0RefuteFraction} ${r.title.slice(0, 70)}`);

  console.log(
    `\n  refines whose finding ended as a nit : ${reached.filter((r) => r.finalSeverity === "nit").length} of ${reached.length}`
  );

  console.log(
    `\nWhat this does NOT establish: whether the ${rescues} rescues were worth rescuing. The gate` +
      `\nchange is a cost claim; the value of a rescue needs the ground-truth corpus.\n`
  );
}
