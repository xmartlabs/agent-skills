#!/usr/bin/env node
// replay-confidence-provenance.mjs — telling the head's 0.5 from ours.
//
// The defect (criterion SO-9, confirmed twice on 2026-09-07): a default written
// into a field and then read back as data. `classify-verdict.sh` emits
// `CONFIDENCE: 0.5` when the model declared nothing, and `applyArtifactTruth`
// wrote that straight onto the vote. A head that deliberately said "half" and a
// head that said nothing at all produced identical votes, identical weights in
// the tally, and identical rows — and the harness's own comment described the
// 0.5 as "the same default tallyVotes applies, never a new one", which is true
// and is exactly what made it invisible.
//
// The weighting is deliberately unchanged. An undeclared confidence still
// counts as 0.5, because moving it would move panel outcomes and that is a
// different decision from the one this fix makes. What changes is that the row
// can now be asked the question at all.
//
// The interesting cases here are the 29 real artifacts captured from the PR
// #1911 run, not the synthetic ones: they are what actual Codex and Gemini
// output looks like, including the formats that broke the original parser.
//
// Usage: node eval/replay-confidence-provenance.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { FIXTURES_DIR, lib, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(FIXTURES_DIR, "verdict-parsing");
const CLASSIFIER = lib("classify-verdict.sh");
const BATCH = lib("classify-artifacts.sh");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const classify = (file) => {
  const out = execFileSync("bash", [CLASSIFIER, file], { encoding: "utf8" });
  const pick = (k) => (out.match(new RegExp(`^${k}:\\s*(.+)$`, "m")) || [])[1]?.trim() ?? null;
  return {
    failclass: pick("FAILCLASS"),
    verdict: pick("VERDICT"),
    confidence: pick("CONFIDENCE"),
    declared: pick("CONFIDENCE_DECLARED"),
    raw: out,
  };
};

const tmp = mkdtempSync(join(tmpdir(), "tribunal-conf-"));
const artifact = (name, text) => {
  const p = join(tmp, name);
  writeFileSync(p, text);
  return p;
};

console.log(`\nconfidence provenance — classifier: ${CLASSIFIER}`);

console.log(`\n--- a declared confidence is marked declared ---`);
{
  const r = classify(artifact("a.out", "Reasoning.\nVERDICT: REFUTED\nCONFIDENCE: 0.9\n"));
  check("the verdict still parses", r.verdict === "REFUTED");
  check("the value is the model's", r.confidence === "0.9");
  check("and it is marked declared", r.declared === "yes");
}

console.log(`\n--- the two 0.5s are now different answers ---`);
{
  const said = classify(artifact("b.out", "VERDICT: NOT_REFUTED\nCONFIDENCE: 0.5\n"));
  const silent = classify(artifact("c.out", "VERDICT: NOT_REFUTED\n"));

  check("a model that said 0.5 is marked declared", said.declared === "yes");
  check("a model that said nothing still defaults to 0.5", silent.confidence === "0.5");
  check("…and is marked undeclared", silent.declared === "no");
  check(
    "the field is the ONLY thing separating them — the defect in one line",
    said.confidence === silent.confidence && said.declared !== silent.declared,
  );
}

console.log(`\n--- an unusable declaration is undeclared, not clamped ---`);
{
  // The script treats anything outside 0..1 as undeclared rather than guessing,
  // because "CONFIDENCE: 95" means 0.95 or nothing and picking one invents a
  // weight. That decision predates this change; the flag now makes it visible.
  for (const [label, body] of [
    ["a percentage", "VERDICT: REFUTED\nCONFIDENCE: 95\n"],
    ["a number above 1", "VERDICT: REFUTED\nCONFIDENCE: 7\n"],
    ["prose instead of a number", "VERDICT: REFUTED\nCONFIDENCE: high\n"],
  ]) {
    const r = classify(artifact(`d-${label.replace(/\W/g, "")}.out`, body));
    check(`${label} falls back to 0.5`, r.confidence === "0.5", r.confidence);
    check(`${label} is reported as undeclared`, r.declared === "no", r.declared);
  }
  check(
    "a boundary value of exactly 1 is a real declaration",
    classify(artifact("e.out", "VERDICT: REFUTED\nCONFIDENCE: 1\n")).declared === "yes",
  );
  check(
    "and so is 0",
    classify(artifact("f.out", "VERDICT: REFUTED\nCONFIDENCE: 0\n")).declared === "yes",
  );
}

console.log(`\n--- a failed classification never claims a confidence at all ---`);
{
  const empty = classify(artifact("g.out", ""));
  check("an empty artifact is refused", empty.failclass === "refused");
  check("…and reports no confidence", empty.confidence === null);
  check("…and no provenance either, rather than a misleading 'no'", empty.declared === null);
}

console.log(`\n--- the 29 real PR #1911 artifacts ---`);
{
  let seen = 0;
  let declared = 0;
  const inconsistent = [];
  for (const set of ["recovered", "regression"]) {
    for (const f of readdirSync(join(FIXTURES, set)).filter((x) => x.endsWith(".txt"))) {
      const path = join(FIXTURES, set, f);
      const r = classify(path);
      if (r.failclass !== "none") continue;
      seen++;
      if (r.declared === "yes") declared++;
      // The invariant that must hold on every real artifact: `yes` iff the
      // reported number is one the file actually contains on a confidence line.
      const raw = readFileSync(path, "utf8");
      const hasDeclaration = new RegExp(
        `confidence[^\\n0-9]*${String(r.confidence).replace(".", "\\.")}`,
        "i",
      ).test(raw.replace(/[*\`_]/g, ""));
      if (r.declared === "yes" && !hasDeclaration) inconsistent.push(`${set}/${f}: said declared but the file has no such line`);
      if (r.declared === "no" && r.confidence !== "0.5") inconsistent.push(`${set}/${f}: undeclared but not 0.5`);
      if (r.declared === null) inconsistent.push(`${set}/${f}: no provenance on a successful classification`);
    }
  }
  check("the corpus is the whole one, not a slice", seen === 29, `${seen} classified cleanly`);
  check(
    "every real artifact carries a provenance flag consistent with its own text",
    inconsistent.length === 0,
    inconsistent.slice(0, 3).join(" | "),
  );
  check(
    "the corpus contains BOTH kinds — a test where every case is the same is not a test",
    declared > 0 && declared < seen,
    `${declared} declared / ${seen - declared} defaulted`,
  );
}

console.log(`\n--- the batch relay carries it across the shell boundary ---`);
{
  const dir = mkdtempSync(join(tmpdir(), "tribunal-batch-"));
  writeFileSync(join(dir, "s1-codex.out"), "VERDICT: REFUTED\nCONFIDENCE: 0.8\n");
  writeFileSync(join(dir, "s2-gemini.out"), "VERDICT: NOT_REFUTED\n");
  let parsed = null;
  try {
    const out = execFileSync("bash", [BATCH, dir, ""], {
      encoding: "utf8",
      env: { ...process.env, PATH: `${dirname(BATCH)}/../bin:${process.env.PATH}` },
    });
    parsed = JSON.parse(out);
  } catch (err) {
    check("classify-artifacts.sh runs", false, String(err.message).slice(0, 120));
  }
  if (parsed) {
    check("classify-artifacts.sh runs and emits JSON", parsed.total === 2);
    check(
      "a declared confidence survives the relay as true",
      parsed.entries["s1-codex"]?.confidenceDeclared === true,
      JSON.stringify(parsed.entries["s1-codex"]),
    );
    check(
      "a defaulted one survives as false, not as a missing field",
      parsed.entries["s2-gemini"]?.confidenceDeclared === false,
      JSON.stringify(parsed.entries["s2-gemini"]),
    );
    check(
      "…and both still carry the same 0.5-or-real confidence as before",
      parsed.entries["s1-codex"]?.confidence === 0.8 &&
        parsed.entries["s2-gemini"]?.confidence === 0.5,
    );
  }
}

console.log(`\n--- wiring: schema, prompt, vote, row ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "the relay schema declares the field, nullable for artifacts that predate it",
    /confidenceDeclared: \{ type: \["boolean", "null"\] \}/.test(src),
  );
  check("the relay prompt asks for it", /confidence, confidenceDeclared\}/.test(src));
  check(
    "applyArtifactTruth derives it rather than assuming",
    /typeof t\.confidenceDeclared === "boolean"/.test(src),
  );
  check("…and puts it on the corrected vote", /confidence,\n      confidenceDeclared,/.test(src));
  check(
    "both persisted vote rows carry it",
    (src.match(/confidenceDeclared: vote\.confidenceDeclared \?\? null/g) || []).length === 2,
  );
  check(
    "the weighting is deliberately untouched — this fix records, it does not re-weight",
    /const confidence = typeof t\.confidence === "number" \? t\.confidence : 0\.5;/.test(src),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall confidence-provenance expectations hold");
