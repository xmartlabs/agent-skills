#!/usr/bin/env node
// rows.mjs — the one place that reads metrics.jsonl, and the one place that
// decides which rows an analysis is allowed to treat as comparable.
//
// WHY IT EXISTS. 17 of 48 rows predate the canonical shape and come in three
// mutually incompatible forms; `references/metrics-schema.md` and the lever
// ledger both say plainly that they are not aggregatable without normalisation.
// That warning lived in PROSE, and on 2026-07-27 a count showed the result: of
// the scripts that read metrics.jsonl, five respected the distinction and six
// did not — and the six were the cost analyses, the ones whose numbers ended up
// in a 44KB architecture document and a lever ledger. One of the six was written
// that same day, by someone who had cited the warning twelve hours earlier and
// still read the file raw.
//
// A caveat that has to be remembered is a caveat that will be forgotten. This
// module makes the choice explicit at the call site: you ask for `canonical` or
// you ask for `all`, and either way you had to decide.
//
// The canonical test is `mode` + a `findings` array — the shape sentinel
// score.mjs has always used. It is reproduced here rather than redefined,
// because a second definition of "comparable" is the problem, not the fix.

import { readFileSync } from "node:fs";
import { METRICS_PATH, isMainModule } from "./paths.mjs";

// Re-exported because every analysis imports it from here, not from paths.mjs —
// this module is the documented chokepoint for reading the measurement log, and
// `replay-rows.mjs` asserts nothing bypasses it. The path itself moved out of
// the skill directory when the harness became a plugin: that directory is a
// cache replaced on every update, and this file is the one thing RELEASES.md
// calls irreproducible.
export { METRICS_PATH };

function parseAll(path = METRICS_PATH) {
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    return [];
  }
  const out = [];
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    try {
      out.push(JSON.parse(t));
    } catch {
      // A row that will not parse is data loss, not a row. Counted by summary().
      out.push(null);
    }
  }
  return out;
}

/** Every row, including legacy shapes and unparseable lines (as null). */
export function allRows(path) {
  return parseAll(path).filter(Boolean);
}

/** The shape sentinel: what score.mjs has always meant by "canonical". */
export const isCanonical = (r) => !!(r && r.mode && Array.isArray(r.findings));

/**
 * Rows an analysis may compare against each other. Anything you intend to
 * average, rate, or plot belongs here — not in allRows().
 */
export function canonicalRows(path) {
  return allRows(path).filter(isCanonical);
}

/** The complement, so a script can REPORT what it excluded instead of hiding it. */
export function legacyRows(path) {
  return allRows(path).filter((r) => !isCanonical(r));
}

/**
 * Canonical rows minus re-runs of the same commit. PR 1957 has four rows for one
 * headSha; counting them four times weights that commit 4x in every rate. Same
 * de-duplication score.mjs applies, for the same reason.
 */
export function independentRows(path) {
  const seen = new Set();
  return canonicalRows(path).filter((r) => {
    if (r.rerunOf) return false;
    const key = `${r.repo || ""}#${r.pr}@${r.headSha}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/**
 * One row per commit, keeping the LATEST — the best run available for that tree.
 *
 * Not the same question as `independentRows()`, and the difference is not
 * cosmetic. An aggregate wants ONE SAMPLE per commit and does not care which, so
 * dropping every `rerunOf` row is fine there. The corpus wants THE BEST RUN of a
 * PR, and dropping every `rerunOf` row means the FIRST run wins forever: PR #1796
 * kept being scored against a 2026-07-21 row with no lens, no evidence and no
 * refutationType, while a v20 re-run of the same commit sat in the log ignored.
 *
 * Both live here so the two selections stay one decision made twice, rather than
 * two definitions that drift.
 */
export function latestPerCommit(path) {
  const byCommit = new Map();
  for (const r of canonicalRows(path)) byCommit.set(`${r.repo || ""}#${r.pr}@${r.headSha}`, r);
  return [...byCommit.values()];
}

/** One line a script can print so its reader knows what was left out. */
export function summary(path) {
  const parsed = parseAll(path);
  const rows = parsed.filter(Boolean);
  const canonical = rows.filter(isCanonical).length;
  return {
    total: rows.length,
    canonical,
    legacy: rows.length - canonical,
    unparseable: parsed.length - rows.length,
    independent: independentRows(path).length,
    line: `${rows.length} rows: ${canonical} canonical, ${rows.length - canonical} legacy (excluded from comparisons)`,
  };
}

if (isMainModule(import.meta.url)) {
  const s = summary();
  console.log(`  ${s.line}`);
  console.log(`  independent (canonical, one per commit): ${s.independent}`);
  if (s.unparseable) console.log(`  UNPARSEABLE LINES: ${s.unparseable}`);
}
