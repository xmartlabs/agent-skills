#!/usr/bin/env node
// replay-verified-gate.mjs — the verifiedBy evidence gate, run twice against
// the same fixtures: once via workflows/tribunal.js's inlined VERIFIED-GATE
// region (what the Workflow path actually executes), once via the real,
// importable lib/verified.mjs (what the in-context path runs for real via
// Bash — see references/in-context-review.md's "verifiedBy evidence gate"
// paragraph). Workflow scripts have no filesystem/import access (see
// VERIFIED-GATE's own header comment), so the two copies cannot share code —
// this file is what keeps them from drifting instead. check-mirror.mjs pins
// the one concrete constant (MIN_QUOTE_CHARS) both copies must share; this
// file proves the two copies actually AGREE on behaviour across the fixtures,
// plus the regression this gate exists for.
//
// Usage: node eval/replay-verified-gate.mjs

import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW, lib } from "./paths.mjs";
import * as real from "../../../lib/verified.mjs";

const { api } = loadRegion(WORKFLOW, "VERIFIED-GATE", [
  "verifiedGateIsGatedFinding",
  "verifiedGateFindingProblems",
  "verifiedGateDemoteUnanchored",
]);
const {
  verifiedGateIsGatedFinding: wfIsGated,
  verifiedGateFindingProblems: wfProblems,
  verifiedGateDemoteUnanchored: wfDemote,
} = api;

let failures = 0;
function check(name, cond, detail) {
  if (!cond) {
    failures++;
    console.error(`FAIL ${name}${detail ? " — " + detail : ""}`);
  } else {
    console.log(`ok   ${name}`);
  }
}

// A finding is "gated" only when it's about to be published Blocking:
// adjudicated and not a nit.
const anchor = {
  file: "prisma/schema.prisma",
  line: 1374,
  quote: "enum upload_type {",
};

// --- isGatedFinding: workflow region vs real module agree -------------------
{
  const cases = [
    { severity: "nit", adjudicated: true },
    { severity: "important", adjudicated: true },
    { severity: "important", adjudicated: false },
    { severity: "critical", adjudicated: true },
    { severity: "critical", adjudicated: false },
  ];
  for (const c of cases) {
    const wf = wfIsGated(c);
    const rl = real.isGatedFinding(c);
    check(
      `isGatedFinding agrees for severity=${c.severity} adjudicated=${c.adjudicated}`,
      wf === rl,
      `workflow=${wf} real=${rl}`,
    );
  }
  check(
    "a nit is never gated even when adjudicated",
    !wfIsGated({ severity: "nit", adjudicated: true }),
  );
  check(
    "important+adjudicated IS gated",
    wfIsGated({ severity: "important", adjudicated: true }),
  );
}

// --- checkVerifiedBy / findingProblems: both copies, both directions --------
{
  const anchoredOk = {
    severity: "important",
    adjudicated: true,
    file: "a.ts",
    line: 1,
    title: "t",
    verifiedBy: [anchor],
  };
  check(
    "anchored gated finding passes (workflow)",
    wfProblems(anchoredOk).length === 0,
  );
  check(
    "anchored gated finding passes (real)",
    real.findingProblems(anchoredOk).length === 0,
  );

  const missingEntirely = {
    severity: "important",
    adjudicated: true,
    file: "a.ts",
    line: 1,
    title: "t",
  };
  check(
    "missing verifiedBy is caught (workflow)",
    wfProblems(missingEntirely).length === 1,
  );
  check(
    "missing verifiedBy is caught (real)",
    real.findingProblems(missingEntirely).length === 1,
  );

  const emptyArray = {
    severity: "critical",
    adjudicated: true,
    file: "a.ts",
    line: 1,
    title: "t",
    verifiedBy: [],
  };
  check(
    "empty verifiedBy is caught (workflow)",
    wfProblems(emptyArray).length === 1,
  );
  check(
    "empty verifiedBy is caught (real)",
    real.findingProblems(emptyArray).length === 1,
  );

  const labelNotQuote = {
    severity: "important",
    adjudicated: true,
    file: "a.ts",
    line: 1,
    title: "t",
    verifiedBy: [{ file: "b.ts", line: 2, quote: "yes" }],
  };
  check(
    "a label instead of a code quote is caught (workflow)",
    wfProblems(labelNotQuote).length === 1,
  );
  check(
    "a label instead of a code quote is caught (real)",
    real.findingProblems(labelNotQuote).length === 1,
  );

  const missingLine = {
    severity: "important",
    adjudicated: true,
    file: "a.ts",
    line: 1,
    title: "t",
    verifiedBy: [{ file: "b.ts", quote: "some real code here" }],
  };
  check(
    "a missing line number is caught (workflow)",
    wfProblems(missingLine).length === 1,
  );
  check(
    "a missing line number is caught (real)",
    real.findingProblems(missingLine).length === 1,
  );

  const nitNoAnchor = {
    severity: "nit",
    adjudicated: true,
    file: "a.ts",
    line: 3,
    title: "v",
  };
  const notAdjudicated = {
    severity: "important",
    adjudicated: false,
    file: "a.ts",
    line: 1,
    title: "u",
  };
  check(
    "a nit is never gated regardless of verifiedBy (workflow)",
    wfProblems(nitNoAnchor).length === 0,
  );
  check(
    "a nit is never gated regardless of verifiedBy (real)",
    real.findingProblems(nitNoAnchor).length === 0,
  );
  check(
    "an unadjudicated finding is never gated (workflow)",
    wfProblems(notAdjudicated).length === 0,
  );
  check(
    "an unadjudicated finding is never gated (real)",
    real.findingProblems(notAdjudicated).length === 0,
  );
}

// --- demoteUnanchored: downgrades, never drops, on both copies -------------
{
  const findings = [
    {
      severity: "important",
      adjudicated: true,
      file: "a.ts",
      line: 1,
      title: "anchored",
      verifiedBy: [anchor],
    },
    {
      severity: "important",
      adjudicated: true,
      file: "b.ts",
      line: 2,
      title: "unanchored",
    },
    {
      severity: "nit",
      adjudicated: true,
      file: "c.ts",
      line: 3,
      title: "nit, no anchor needed",
    },
  ];
  for (const [label, demote] of [
    ["workflow", wfDemote],
    ["real", real.demoteUnanchored],
  ]) {
    const out = demote(findings);
    check(
      `${label}: same count in and out (never drops)`,
      out.length === findings.length,
    );
    check(
      `${label}: anchored finding keeps adjudicated:true`,
      out[0].adjudicated === true && !out[0].verifiedByGateNote,
    );
    check(
      `${label}: unanchored finding demoted to adjudicated:false`,
      out[1].adjudicated === false,
    );
    check(
      `${label}: unanchored finding carries a note explaining why`,
      typeof out[1].verifiedByGateNote === "string" &&
        out[1].verifiedByGateNote.length > 0,
    );
    check(
      `${label}: nit passes through untouched`,
      out[2].adjudicated === true && !out[2].verifiedByGateNote,
    );
  }
}

// --- regression pin: the incident this gate exists to catch ----------------
{
  const pr2042Finding = {
    severity: "important",
    adjudicated: true,
    file: "src/services/v2/batchInvoice.ts",
    line: 1049,
    title:
      "routing relies on a string prefix instead of the typed discriminator",
  };
  check(
    "the PR #2042 false positive would have been demoted by this gate (workflow)",
    wfProblems(pr2042Finding).length === 1,
  );
  check(
    "the PR #2042 false positive would have been demoted by this gate (real)",
    real.findingProblems(pr2042Finding).length === 1,
  );
  const demoted = real.demoteUnanchored([pr2042Finding])[0];
  check(
    "and would have been reported as an open question, not Blocking",
    demoted.adjudicated === false &&
      demoted.verifiedByGateNote.includes("verifiedBy is missing"),
  );
}

// --- CLI shape: lib/verified.mjs's stdin/stdout contract --------------------
// The in-context path invokes this via `node lib/verified.mjs -` per
// references/in-context-review.md. Confirm the file the mirror rule pins
// (paths.mjs's lib("verified.mjs")) is the same file this replay imported —
// a silent path drift here would mean the replay tests one file while the
// in-context instructions invoke another.
{
  const pathsLibPath = lib("verified.mjs");
  const { fileURLToPath } = await import("node:url");
  const { resolve } = await import("node:path");
  const importedFrom = resolve(
    fileURLToPath(new URL("../../../lib/verified.mjs", import.meta.url)),
  );
  check(
    "the module this replay imported IS the file paths.mjs resolves for the CLI",
    resolve(pathsLibPath) === importedFrom,
    `paths.mjs: ${pathsLibPath}, imported: ${importedFrom}`,
  );
}

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nreplay-verified-gate: all checks passed");
