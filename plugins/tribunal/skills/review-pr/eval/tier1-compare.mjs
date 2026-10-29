// tier1-compare.mjs — score a Tier-1 package arm against its recorded baseline.
//
// One arm = one finding refuted twice: once by the historical agent that had to
// fetch its own context, once by an agent handed a pre-assembled package. This
// prints the two things the experiment exists to compare — did the VERDICT hold,
// and did the COST move — plus the one number that decides whether the fallback
// is safe: how many files the packaged arm still had to open.
//
// Two comparability caveats travel with every table this prints, and they are
// printed with it rather than left in a commit message:
//
//   · The arms ran in different harnesses. The baseline was a Workflow agent;
//     the package arm runs through the Agent tool, with a different system
//     prompt, tool set and turn floor. A turn is not exactly a turn across that
//     boundary.
//   · A verdict match is weak evidence when the finding itself is unstable. On
//     PR #1935's getStateCode claim the recorded refuters split 4 NOT_REFUTED /
//     4 REFUTED across runs, so on THAT finding neither agreement nor
//     disagreement says much — it measures the noise floor, not the package.
//
// Usage: node eval/tier1-compare.mjs <baseline.json> <agent-output.jsonl> [...]
//        (pairs, in that order)

import { readFileSync } from "node:fs";
import { parseTranscript } from "./agent-cost.mjs";
import { isMainModule } from "./paths.mjs";

// The arm's final message is the JSON verdict object. Taken from the LAST
// assistant text in the transcript, and reported as unparseable rather than
// guessed at — an arm whose verdict cannot be read is not an arm that agreed.
export function finalVerdict(text) {
  let last = null;
  for (const line of String(text || "").split("\n")) {
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      continue;
    }
    const c = rec && rec.message && rec.message.content;
    const parts = typeof c === "string" ? [c] : Array.isArray(c) ? c.map((x) => x && x.text).filter(Boolean) : [];
    for (const p of parts) {
      const m = /\{[\s\S]*"verdict"[\s\S]*\}/.exec(p);
      if (m) last = m[0];
    }
  }
  if (!last) return null;
  try {
    return JSON.parse(last);
  } catch {
    return null;
  }
}

// The CLI runs only when this file IS the entrypoint. Without the guard,
// importing `finalVerdict` from a sibling script executes this body, hits the
// usage check and exits the IMPORTER — which is exactly what happened to
// eval/groundtruth-compare.mjs.
const isMain = isMainModule(import.meta.url);

const args = isMain ? process.argv.slice(2) : [];
if (isMain && (args.length < 2 || args.length % 2 !== 0)) {
  console.error("usage: node eval/tier1-compare.mjs <baseline.json> <output.jsonl> [<baseline.json> <output.jsonl> ...]");
  process.exit(2);
}

const rows = [];
for (let i = 0; isMain && i < args.length; i += 2) {
  const base = JSON.parse(readFileSync(args[i], "utf8"));
  const text = readFileSync(args[i + 1], "utf8");
  const cost = parseTranscript(text);
  const v = finalVerdict(text);
  rows.push({ base, cost, v });
}

const pct = (a, b) => (b > 0 ? `${(((a - b) / b) * 100).toFixed(0)}%` : "—");
if (isMain) {
console.log(`\nTIER 1 — packaged refuter vs its recorded baseline\n`);
for (const { base, cost, v } of rows) {
  const b = base.baseline;
  const match = v && v.verdict === b.verdict;
  console.log(`PR #${base.pr} ${base.headSha.slice(0, 8)}  ${base.title.slice(0, 66)}`);
  console.log(`  package: ${base.package.length} file(s)   historically read but uncovered: ${base.uncoveredByPackage.length}`);
  console.log(
    `  verdict   ${String(b.verdict).padEnd(12)} -> ${String(v ? v.verdict : "UNPARSEABLE").padEnd(12)} ${
      v ? (match ? "MATCH" : "*** DIFFERS ***") : "*** NO VERDICT ***"
    }`
  );
  console.log(
    `  confidence ${String(b.confidence).padEnd(11)} -> ${v ? v.confidence : "—"}          coreStands ${b.coreStands} -> ${v ? !!v.coreStands : "—"}`
  );
  console.log(`  turns     ${String(b.turns).padEnd(12)} -> ${String(cost.turns).padEnd(12)} ${pct(cost.turns, b.turns)}`);
  console.log(
    `  input     ${(b.inputTokens / 1e6).toFixed(2)}M`.padEnd(26) +
      `-> ${(cost.inputTokens / 1e6).toFixed(2)}M`.padEnd(15) +
      `${pct(cost.inputTokens, b.inputTokens)}`
  );
  console.log(
    `  input/turn ${Math.round(b.inputTokens / b.turns).toLocaleString("en-US")}`.padEnd(26) +
      `-> ${Math.round(cost.inputTokens / Math.max(1, cost.turns)).toLocaleString("en-US")}`.padEnd(15) +
      `${pct(cost.inputTokens / Math.max(1, cost.turns), b.inputTokens / b.turns)}`
  );
  const outside = v && Array.isArray(v.filesOpenedOutsidePackage) ? v.filesOpenedOutsidePackage : null;
  console.log(`  opened outside the package: ${outside ? (outside.length ? outside.join(", ") : "none") : "not reported"}`);
  console.log();
}

const scored = rows.filter((r) => r.v);
const matches = scored.filter((r) => r.v.verdict === r.base.baseline.verdict).length;
const bT = rows.reduce((s, r) => s + r.base.baseline.turns, 0);
const aT = rows.reduce((s, r) => s + r.cost.turns, 0);
const bI = rows.reduce((s, r) => s + r.base.baseline.inputTokens, 0);
const aI = rows.reduce((s, r) => s + r.cost.inputTokens, 0);
console.log(`TOTALS over ${rows.length} arm(s)`);
console.log(`  verdicts matching baseline : ${matches}/${scored.length}${scored.length < rows.length ? ` (${rows.length - scored.length} unparseable)` : ""}`);
console.log(`  turns  ${bT} -> ${aT}  (${pct(aT, bT)})`);
console.log(`  input  ${(bI / 1e6).toFixed(2)}M -> ${(aI / 1e6).toFixed(2)}M  (${pct(aI, bI)})`);
console.log(`\nCAVEATS, always: different harnesses between the arms (Workflow vs Agent tool),`);
console.log(`and a verdict match is weak on findings whose recorded refuters already disagree`);
console.log(`with each other — PR #1935's getStateCode claim split 4/4 across runs.\n`);
}
