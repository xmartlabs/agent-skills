#!/usr/bin/env node
// analyze-cost-deep.mjs — the queries behind specs/2026-07-25-cost-architecture-analysis.md
//
//   node eval/analyze-cost-deep.mjs            — every section
//   node eval/analyze-cost-deep.mjs --only funnel,stages
//
// Costs 0 tokens. `analyze-agent-cost.mjs` answers "what did each run cost";
// this answers "what did the spend BUY", which is the question that actually
// decides a redesign.
//
// Why this file exists at all: every query here was first written ad-hoc in a
// conversation, and three of them had bugs that PRODUCED the conclusion — an
// import closure read against the local tree instead of the PR's tree (37% vs
// the real 47%), a test that passed via array order rather than the property it
// claimed to check, and two irreconcilable corpus totals. The corrected versions
// live here so the next analysis does not re-derive them, and re-introduce the
// bugs. See the 2026-07-25 sharpening in the "verify your own verification
// method" memory.
//
// Scope note: reads only this workspace's project subtree.

import { resolve, join } from "node:path";
import { existsSync, readFileSync, statSync } from "node:fs";
import { canonicalRows, METRICS_PATH } from "./rows.mjs";
import { findRunDirs, readRunDir, isPeerReviewRun } from "./agent-cost.mjs";
import { projectDir } from "./paths.mjs";

const PROJECT_DIR = projectDir();
// METRICS_PATH, not a locally rebuilt path: the log lives outside the plugin
// directory (which updates replace wholesale). Rebuilding it here pointed at a
// file that no longer exists — found 2026-07-30 by widening replay-rows.mjs to
// treat CONSTRUCTING the path as reading it.
const METRICS = METRICS_PATH;

const only = (() => {
  const i = process.argv.indexOf("--only");
  return i === -1 ? null : new Set(process.argv[i + 1].split(","));
})();
const want = (name) => !only || only.has(name);
const n = (x) => Number(x || 0).toLocaleString("en-US");
const pct = (a, b) => (b > 0 ? `${((100 * a) / b).toFixed(0)}%` : "—");
const head = (t) => console.log(`\n${"=".repeat(74)}\n${t}\n`);

// --- shared loads -----------------------------------------------------------
// This file had the canonical filter right, written out inline. Moved to the
// shared helper not because it was wrong but because a SECOND copy of "which
// rows are comparable" is the failure mode: two definitions drift, and the one
// that drifts is the one nobody re-reads.
const rows = canonicalRows();

// Every paneled finding across canonical rows. This is the unit for stage
// analysis — NOT the run, because stages fire per finding.
const panels = rows.flatMap((r) =>
  r.findings.filter((f) => f.panelRan).map((f) => ({ ...f, pr: r.pr, ts: r.ts }))
);

// Transcript-derived per-agent cost, grouped by kind and by PR. Loaded once.
const agents = [];
for (const d of findRunDirs(PROJECT_DIR)) {
  let run;
  try {
    run = readRunDir(d);
  } catch {
    continue;
  }
  if (!isPeerReviewRun(run)) continue;
  const pr =
    (run.agents
      .map((a) => /PR #(\d+)/.exec(a.promptHead || ""))
      .find(Boolean) || [])[1] || "?";
  for (const a of run.agents) agents.push({ ...a, pr, runDir: d, mtimeMs: statSync(d).mtimeMs });
}
const byKind = (kind, prFilter) =>
  agents.filter((a) => a.kind === kind && (!prFilter || prFilter(a.pr)));
const meanIn = (list) => (list.length ? list.reduce((s, a) => s + a.inputTokens, 0) / list.length : 0);
const meanTurns = (list) => (list.length ? list.reduce((s, a) => s + a.turns, 0) / list.length : 0);

if (!agents.length) {
  console.log(`\nNo transcripts on disk under ${PROJECT_DIR}. They are deleted over time.`);
  console.log(`The durable copy of what they showed is specs/2026-07-25-cost-architecture-analysis.md\n`);
}

// --- 1. share of spend, with and without the dominant PR --------------------
// Every average in the first draft of the analysis was inflated 1.8x by PR
// #1911, which is 70% of the corpus. Always print both.
if (want("share")) {
  head("SHARE OF INPUT BY AGENT KIND — pooled, and excluding the dominant PR");
  const dominant = Object.entries(
    agents.reduce((acc, a) => ((acc[a.pr] = (acc[a.pr] || 0) + a.inputTokens), acc), {})
  ).sort((x, y) => y[1] - x[1])[0];
  console.log(`  dominant PR: #${dominant?.[0]} with ${pct(dominant?.[1], agents.reduce((s, a) => s + a.inputTokens, 0))} of all input\n`);
  for (const [label, filter] of [
    ["POOLED", null],
    [`EXCLUDING #${dominant?.[0]}`, (pr) => pr !== dominant?.[0]]
  ]) {
    const set = agents.filter((a) => !filter || filter(a.pr));
    const tot = set.reduce((s, a) => s + a.inputTokens, 0);
    const kinds = {};
    for (const a of set) (kinds[a.kind] = kinds[a.kind] || []).push(a);
    console.log(`  ${label}  (${(tot / 1e6).toFixed(0)}M)`);
    for (const [k, list] of Object.entries(kinds).sort(
      (x, y) => meanIn(y[1]) * y[1].length - meanIn(x[1]) * x[1].length
    ).slice(0, 8))
      console.log(
        `    ${k.padEnd(15)}${pct(list.reduce((s, a) => s + a.inputTokens, 0), tot).padStart(4)}   ${(meanIn(list) / 1e6).toFixed(2)}M/agente   ${meanTurns(list).toFixed(1)} turnos`
      );
    console.log();
  }
}

// --- 2. what each extra stage buys ------------------------------------------
if (want("stages")) {
  head("WHAT EACH EXTRA STAGE BUYS (marginal cost per outcome changed)");
  const esc = panels.filter((f) => f.escalated);
  const ref = panels.filter((f) => f.refined);
  const deb = panels.filter((f) => f.debate && f.debate.fired);
  const w = (v) => (typeof v.confidence === "number" ? v.confidence : 0.5);
  const frac = (vs) => {
    const R = vs.filter((v) => v.verdict === "REFUTED").reduce((s, v) => s + w(v), 0);
    const K = vs.filter((v) => v.verdict === "NOT_REFUTED").reduce((s, v) => s + w(v), 0);
    return R + K === 0 ? null : R / (R + K);
  };
  // Escalation counterfactual: buildPanel returns [...claudes, ...externals] and
  // the escalation Claude is appended, so the LAST claude vote is the escalation
  // vote. Restricted to escalated===true because the both-externals-down fallback
  // also yields 2 Claude votes and there slot 1 is NOT an escalation.
  let changed = 0,
    same = 0,
    uncomputable = 0;
  for (const f of esc) {
    const T = f.threshold || 0.45;
    if (!Array.isArray(f.votes)) {
      uncomputable++;
      continue;
    }
    const cl = f.votes.map((v, i) => ({ v, i })).filter((x) => x.v.source === "claude");
    if (cl.length < 2) {
      uncomputable++;
      continue;
    }
    const without = f.votes.filter((_, i) => i !== cl[cl.length - 1].i);
    const a = frac(f.votes),
      b = frac(without);
    if (a === null || b === null) {
      uncomputable++;
      continue;
    }
    a >= T === (b >= T) ? same++ : changed++;
  }
  const cost = {
    refuter: meanIn(byKind("refuter")),
    strengthen: meanIn(byKind("strengthen")),
    defense: meanIn(byKind("defense")),
    adjudicator: meanIn(byKind("adjudicator"))
  };
  const line = (name, fires, unit, wins, label, note) => {
    const tot = fires * unit;
    console.log(
      `  ${name.padEnd(12)}${String(fires).padStart(4)} disparos   ${(tot / 1e6).toFixed(0).padStart(4)}M   ${String(wins).padStart(3)} ${label.padEnd(24)} ${wins ? (tot / wins / 1e6).toFixed(1) + "M c/u" : "—"}${note ? `  ${note}` : ""}`
    );
  };
  line("escalacion", esc.length, cost.refuter, changed, "decisiones dadas vuelta", `(${uncomputable} no computables — falta votes[].round)`);
  line("debate", deb.length, cost.defense + cost.adjudicator, deb.filter((f) => f.debate.outcome === "flipped").length, "veredictos flipeados");
  line("refine", ref.length, cost.strengthen + cost.refuter, ref.filter((f) => f.survived).length, "findings rescatados");
  console.log(`\n  NO SUMABLES: refine y el triage-previo atacan poblaciones solapadas.`);
  console.log(`  referencia: una lente entera cuesta ${(meanIn(byKind("lens")) / 1e6).toFixed(2)}M`);

  // Which refine trigger pays. Post-refine the panel is replaced, so round-0
  // signals are partly gone — the "no atribuible" bucket is that loss, not noise.
  const core = (f) => Array.isArray(f.votes) && f.votes.some((v) => v.coreStands === true);
  const buckets = { "coreStands+borderline": [], "solo coreStands": [], "solo borderline": [], "no atribuible": [] };
  for (const f of ref) {
    const c = core(f),
      b = !!f.borderline;
    buckets[c && b ? "coreStands+borderline" : c ? "solo coreStands" : b ? "solo borderline" : "no atribuible"].push(f);
  }
  console.log(`\n  REFINE por señal (severidad y votos son POST-refine — ver §11.ante 2):`);
  for (const [k, v] of Object.entries(buckets))
    console.log(`    ${k.padEnd(24)}${String(v.length).padStart(3)} disparos   ${String(v.filter((f) => f.survived).length).padStart(2)} rescates   ${pct(v.filter((f) => f.survived).length, v.length)}`);
}

// --- 3. is the fan-out over-provisioned? ------------------------------------
if (want("fanout")) {
  head("FAN-OUT — ¿alcanzaba con el primer votante?");
  const withRf = panels.filter((f) => typeof f.refuteFraction === "number");
  const unan = withRf.filter((f) => f.refuteFraction === 0 || f.refuteFraction === 1);
  console.log(`  paneles: ${withRf.length}   unanimes (0.00 o 1.00): ${unan.length} (${pct(unan.length, withRf.length)})`);
  const hard = withRf.filter((f) => f.refuteFraction > 0.35 && f.refuteFraction <= 0.55);
  console.log(`  zona dificil (0.36-0.55): ${hard.length} (${pct(hard.length, withRf.length)})`);
  let ok = 0,
    bad = [];
  for (const f of withRf) {
    if (!Array.isArray(f.votes) || !f.votes.length) continue;
    const T = f.threshold || 0.45;
    const solo = (f.votes[0].verdict === "REFUTED" ? 1 : 0) >= T;
    solo === f.refuteFraction >= T ? ok++ : bad.push(f.votes[0]);
  }
  console.log(`\n  el 1er voto solo (slot 0 = refuter Claude ronda 0) daria la MISMA decision en ${ok}/${ok + bad.length} (${pct(ok, ok + bad.length)})`);
  const conf = (v) => (typeof v.confidence === "number" ? v.confidence : 0.5);
  const mean = (a) => (a.length ? a.reduce((s, x) => s + x, 0) / a.length : 0);
  const rightConf = withRf
    .filter((f) => Array.isArray(f.votes) && f.votes.length)
    .filter((f) => ((f.votes[0].verdict === "REFUTED" ? 1 : 0) >= (f.threshold || 0.45)) === (f.refuteFraction >= (f.threshold || 0.45)))
    .map((f) => conf(f.votes[0]));
  console.log(`  confianza del 1er voto: acertando ${mean(rightConf).toFixed(2)}  errando ${mean(bad.map(conf)).toFixed(2)}`);
  console.log(`  => si esos dos numeros son parecidos, la confianza NO sirve como compuerta`);
  console.log(`     y el fan-out no se puede recortar a ciegas. Bajar el PRECIO del votante, no el numero.`);
}

// --- 4. break-even for a pre-assembled context ------------------------------
// The single most decision-changing query: a package is billed EVERY turn, so a
// generous package needs a large turn drop just to break even. This is what
// demoted pre-assembly for lenses and promoted it for refuters.
if (want("breakeven")) {
  head("BREAK-EVEN DEL CONTEXTO PRE-ARMADO (el paquete se cobra cada turno)");
  const FLOOR = 20600; // measured: first-turn context of the shortest-prompt mechanical agents
  console.log(`  piso por turno medido: ${n(FLOOR)} tokens\n`);
  console.log(`  ${"clase".padEnd(22)}${"paquete".padStart(9)}${"hoy".padStart(20)}${"con paquete".padStart(14)}   veredicto`);
  for (const [kind, pkg, label] of [
    ["refuter", 13000, "refuter (3 archivos)"],
    ["lens", 135000, "lens (PR cohesivo)"],
    ["lens", 202000, "lens (PR disperso)"]
  ]) {
    const list = byKind(kind);
    if (!list.length) continue;
    const perAgent = meanIn(list),
      turns = meanTurns(list);
    const newPerTurn = FLOOR + pkg;
    const be = perAgent / newPerTurn;
    console.log(
      `  ${label.padEnd(22)}${(pkg / 1000).toFixed(0).padStart(7)}k   ${(Math.round(perAgent / turns / 1000) + "k/turno · " + turns.toFixed(0) + "t").padStart(17)}${(Math.round(newPerTurn / 1000) + "k/turno").padStart(14)}   ${be >= turns ? "GANA sin bajar turnos" : `necesita ${turns.toFixed(0)}→<${be.toFixed(0)} turnos (−${(100 * (1 - be / turns)).toFixed(0)}%)`}`
    );
  }
}

// --- 5. redundant file access -----------------------------------------------
if (want("redundancy")) {
  head("ACCESOS A UN PATH YA ACCEDIDO (techo de redundancia, no piso)");
  console.log(`  CUENTA ACCESOS, NO BYTES: dos sed -n sobre rangos distintos del mismo`);
  console.log(`  archivo cuentan como repeticion sin serlo. La redundancia en BYTES esta sin medir.`);
  console.log(`  El path se captura sin exigir extension de archivo, asi que entran tokens que`);
  console.log(`  no son archivos: da ~43% donde una version que exige \\.(ts|js|prisma|json) da 37%.`);
  console.log(`  Tomar 37-43% como una banda, no como un punto.\n`);
  for (const kind of ["lens", "refuter"]) {
    let total = 0,
      uniq = 0,
      nAgents = 0;
    for (const a of byKind(kind)) {
      const counts = {};
      const f = join(a.runDir, `agent-${a.agentId}.jsonl`);
      if (!existsSync(f)) continue;
      nAgents++;
      for (const line of readFileSync(f, "utf8").split("\n")) {
        if (!line.trim()) continue;
        let rec;
        try {
          rec = JSON.parse(line);
        } catch {
          continue;
        }
        const c = rec.message && rec.message.content;
        if (!Array.isArray(c)) continue;
        for (const b of c) {
          if (!b || b.type !== "tool_use") continue;
          let t = null;
          if (b.name === "Read" && b.input && b.input.file_path) t = b.input.file_path;
          else if (b.name === "Bash" && b.input && b.input.command) {
            const m = /(?:git show [0-9a-f]{6,}:|cat |head -\d+ |sed -n [^ ]+ )([^\s|&;'"]+)/.exec(String(b.input.command));
            if (m) t = m[1];
          }
          if (!t) continue;
          const k = t.replace(/^.*?((?:src|prisma|scripts)\/.*)$/, "$1");
          counts[k] = (counts[k] || 0) + 1;
        }
      }
      const vals = Object.values(counts);
      total += vals.reduce((s, x) => s + x, 0);
      uniq += vals.length;
    }
    console.log(`  ${kind.padEnd(9)} n=${String(nAgents).padStart(3)}  accesos ${String(total).padStart(5)}  archivos distintos ${String(uniq).padStart(5)}  redundantes ${pct(total - uniq, total)}`);
  }
}

// --- 6. re-run share --------------------------------------------------------
if (want("reruns")) {
  head("RE-EJECUCIONES — cuanto del gasto fue recomputar el mismo PR");
  const byPr = {};
  for (const a of agents) (byPr[a.pr] = byPr[a.pr] || { in: 0, runs: new Set() }).in += a.inputTokens, byPr[a.pr].runs.add(a.runDir);
  const tot = Object.values(byPr).reduce((s, v) => s + v.in, 0);
  // Computed over EVERY entry exactly once, separately from the display slice.
  // The first version of this file accumulated inside the top-8 display loop and
  // then again over `.slice(8)` of the UNSORTED object, double-counting the
  // overlap and reporting 93% where the correct figure is 83%.
  const rework = Object.values(byPr).reduce((s, v) => s + (v.in - v.in / v.runs.size), 0);
  const ranked = Object.entries(byPr).sort((x, y) => y[1].in - x[1].in);
  console.log(`  PR      runs    input     %`);
  for (const [pr, v] of ranked.slice(0, 8))
    console.log(`  ${pr.padEnd(8)}${String(v.runs.size).padStart(3)}  ${(v.in / 1e6).toFixed(0).padStart(6)}M   ${pct(v.in, tot).padStart(4)}`);
  if (ranked.length > 8) console.log(`  (+${ranked.length - 8} PRs mas)`);
  console.log(`\n  re-ejecuciones ≈ ${(rework / 1e6).toFixed(0)}M de ${(tot / 1e6).toFixed(0)}M (${pct(rework, tot)})`);
  console.log(`  CAVEAT: asume que todos los runs de un PR cuestan igual, y buena parte`);
  console.log(`  fueron ablaciones donde re-correr ERA el objetivo. NO es un ahorro disponible.`);
}

// --- 7. funnel --------------------------------------------------------------
if (want("funnel")) {
  head("EMBUDO");
  const s = rows.reduce(
    (acc, r) => {
      acc.runs++;
      acc.lens += r.lensCount || 0;
      acc.cand += r.candidates || 0;
      acc.paneled += r.paneled || 0;
      acc.killed += r.killed || 0;
      acc.blocking += r.findings.filter((f) => f.blocking).length;
      return acc;
    },
    { runs: 0, lens: 0, cand: 0, paneled: 0, killed: 0, blocking: 0 }
  );
  console.log(`  ${s.runs} runs · ${s.lens} lentes → ${s.cand} candidatos (${(s.cand / Math.max(1, s.lens)).toFixed(2)}/lente)`);
  console.log(`  → ${s.paneled} paneles (${pct(s.paneled, s.cand)}) → ${s.killed} matados (${pct(s.killed, s.paneled)}) → ${s.blocking} blocking (${(s.blocking / Math.max(1, s.runs)).toFixed(1)}/run)`);
  console.log(`\n  Un tercio de lo que llega a panel muere ahi: el panel no es ceremonia.`);
}

console.log(`\nLecturas obligadas antes de actuar sobre esto:`);
console.log(`  specs/2026-07-25-cost-architecture-analysis.md  — §11.ante refuta partes del propio analisis`);
console.log(`  specs/2026-07-24-cost-measurement-protocol.md   — superado, pero la disciplina A/B sigue valiendo\n`);
