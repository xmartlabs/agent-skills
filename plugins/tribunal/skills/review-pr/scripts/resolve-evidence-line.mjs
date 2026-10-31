#!/usr/bin/env node
// resolve-evidence-line.mjs — deterministic evidence/line resolver.
//
// Given a real diff and a candidate finding's {file, line, evidence}, checks
// whether the evidence quote (already mandatory per SKILL.md:129) actually
// appears in the diff, and where. Zero LLM tokens — pure string/diff
// matching. Stage 1 (shadow mode) per specs/2026-07-24-evidence-diff-resolver-spec.md:
// the match verdict never changes a finding. The one correction it makes is a
// line number taken from the patch instead of the file (patchLineCorrection).
//
// Usage (CLI): node resolve-evidence-line.mjs <diff-path> <candidates-json-path|->
//   Candidates JSON: [{ "id": "...", "file": "...", "line": N, "evidence": "..." }]
//   stdout: { "ran": true, "results": [{id, ...resolution}] }
//        or { "ran": false, "reason": "invalid-diff" }
//
// Usage (module): import { resolveEvidence } from "./resolve-evidence-line.mjs";

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { relayLines } from "../../../rust/crates/tribunal-context/src/js/relay.mjs";

// A strict `import.meta.url === file://${process.argv[1]}` comparison breaks
// when the script is invoked through a symlink (e.g. the ~/.claude/skills/...
// symlink into this plugin's real, version-keyed cache directory): argv[1]
// keeps the symlink path while import.meta.url resolves to the real one, so
// they never match, main() silently never runs, and the script exits 0 having
// done nothing. Resolve both sides to their real path before comparing.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

const SENTINEL = "No relevant quotes found";
const WINDOW = 10;
const SPECIFICITY_MIN_CHARS = 8;
const GENERIC_QUOTES = new Set([
  "}",
  "{",
  ");",
  ";",
  "return;",
  "return null;",
  "else {",
  "} else {",
  "});",
  "})",
]);

// Same predicates SKILL.md:125 already specifies for the diff-fetch step —
// a truncated/invalid diff must never be trusted enough to call anything
// "not-found" against it (the PR #1911 failure mode).
export function diffIsUsable(diffText) {
  if (!diffText || diffText.length < 200) return false;
  if (/^\s*(Full diff available at:|Output too large)/m.test(diffText))
    return false;
  if (diffText.includes("Preview (first ")) return false;
  if (!/^diff --git /m.test(diffText)) return false;
  return true;
}

function normalizeLine(line) {
  let s = line;
  if (s.length > 0 && (s[0] === "+" || s[0] === "-" || s[0] === " ")) {
    s = s.slice(1);
  }
  return s.replace(/\r$/, "").trim().replace(/\s+/g, " ");
}

function normalizePositiveInt(v) {
  const n = Number(v);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function firstNonBlankLine(text) {
  const parts = text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
  return parts.length ? parts[0] : "";
}

function isLowSpecificity(normalizedEvidence) {
  if (normalizedEvidence.length < SPECIFICITY_MIN_CHARS) return true;
  if (GENERIC_QUOTES.has(normalizedEvidence)) return true;
  return false;
}

// Parses `diff --git` blocks into per-file line records. Added/context lines
// carry a real new-file line number (hunk-header derived, not a diff
// offset — the #1911 postmortem's specific complaint); removed lines carry
// an old-file line number instead, since they have no place in the new file.
function parseDiff(diffText) {
  const filesMap = new Map();
  const lines = diffText.split("\n");
  const gitHeaderRe = /^diff --git a\/(.+?) b\/(.+)$/;
  const hunkRe = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

  let currentFile = null;
  let oldLineNum = null;
  let newLineNum = null;

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    // The 1-based line of the patch itself, which is what a lens cites when it
    // takes a line number from the patch it read instead of from the file.
    const diffLine = i + 1;
    const gitMatch = gitHeaderRe.exec(raw);
    if (gitMatch) {
      currentFile = gitMatch[2];
      if (!filesMap.has(currentFile)) filesMap.set(currentFile, []);
      oldLineNum = null;
      newLineNum = null;
      continue;
    }
    if (currentFile == null) continue;

    const hunkMatch = hunkRe.exec(raw);
    if (hunkMatch) {
      oldLineNum = parseInt(hunkMatch[1], 10);
      newLineNum = parseInt(hunkMatch[2], 10);
      continue;
    }
    if (oldLineNum == null || newLineNum == null) continue; // not in a hunk yet

    const records = filesMap.get(currentFile);
    if (raw.startsWith("+")) {
      records.push({
        newLine: newLineNum,
        oldLine: null,
        kind: "added",
        normalized: normalizeLine(raw),
        diffLine,
      });
      newLineNum++;
    } else if (raw.startsWith("-")) {
      records.push({
        newLine: null,
        oldLine: oldLineNum,
        kind: "removed",
        normalized: normalizeLine(raw),
        diffLine,
      });
      oldLineNum++;
    } else if (raw.startsWith(" ") || raw === "") {
      records.push({
        newLine: newLineNum,
        oldLine: oldLineNum,
        kind: "context",
        normalized: normalizeLine(raw),
        diffLine,
      });
      newLineNum++;
      oldLineNum++;
    }
    // Anything else ("\ No newline at end of file", etc.) is not a content line.
  }
  return filesMap;
}

// Same fuzzy file lookup `fileInDiff` uses in peer-review.js — a candidate's
// `file` and the diff's path are not always byte-identical (leading "./",
// repo-relative vs absolute).
function lookupFile(filesMap, file) {
  if (filesMap.has(file)) return filesMap.get(file);
  for (const [key, records] of filesMap) {
    if (key.endsWith(file) || file.endsWith(key)) return records;
  }
  return null;
}

function emptyResolution(matchKind, note) {
  return {
    matchKind,
    evidenceLocated: matchKind === "exact" || matchKind === "shifted" || matchKind === "elsewhere",
    claimedLine: null,
    resolvedLine: null,
    resolvedFile: null,
    matchedLineKind: null,
    ambiguousCount: 0,
    delta: null,
    lowConfidence: false,
    note,
  };
}

function buildMatch(kind, m, claimedLine, file, ambiguousCount, lowConfidence) {
  return {
    matchKind: kind,
    evidenceLocated: true,
    claimedLine,
    resolvedLine: m.newLine,
    resolvedFile: m.newLine != null ? file : null,
    matchedLineKind: m.kind,
    ambiguousCount,
    delta: m.newLine != null && claimedLine != null ? m.newLine - claimedLine : null,
    lowConfidence,
    note: m.newLine == null ? "matched a removed line — no new-file line to resolve to" : null,
  };
}

export function resolveEvidence(diffText, finding) {
  const evidenceRaw = finding && finding.evidence;

  if (
    typeof evidenceRaw !== "string" ||
    evidenceRaw.trim() === "" ||
    evidenceRaw.trim() === SENTINEL
  ) {
    return emptyResolution("skipped", null);
  }

  const isMultiLine = evidenceRaw.includes("\n");
  const evidenceLine = isMultiLine ? firstNonBlankLine(evidenceRaw) : evidenceRaw;
  const normalizedEvidence = normalizeLine(evidenceLine);
  const claimedLine = normalizePositiveInt(finding && finding.line);

  if (!normalizedEvidence || isLowSpecificity(normalizedEvidence)) {
    return { ...emptyResolution("low-specificity", null), claimedLine, lowConfidence: isMultiLine };
  }

  const filesMap = parseDiff(diffText);
  const records = lookupFile(filesMap, finding.file);

  if (!records || records.length === 0) {
    return {
      ...emptyResolution("not-found", "not-in-diff not distinguished in v1"),
      claimedLine,
      lowConfidence: isMultiLine,
    };
  }

  const allMatches = records.filter((r) => r.normalized === normalizedEvidence);

  if (allMatches.length === 0) {
    return {
      ...emptyResolution("not-found", "not-in-diff not distinguished in v1"),
      claimedLine,
      lowConfidence: isMultiLine,
    };
  }

  const inWindow =
    claimedLine != null
      ? allMatches.filter((m) => m.newLine != null && Math.abs(m.newLine - claimedLine) <= WINDOW)
      : [];

  const pool = inWindow.length > 0 ? inWindow : allMatches;

  if (pool.length > 1) {
    const exactAtClaim =
      claimedLine != null ? pool.filter((m) => m.newLine === claimedLine) : [];
    if (exactAtClaim.length === 1) {
      return buildMatch("exact", exactAtClaim[0], claimedLine, finding.file, allMatches.length, isMultiLine);
    }
    return {
      ...emptyResolution("ambiguous", null),
      evidenceLocated: true,
      claimedLine,
      ambiguousCount: allMatches.length,
      lowConfidence: isMultiLine,
    };
  }

  const m = pool[0];
  const kind =
    claimedLine != null && m.newLine === claimedLine
      ? "exact"
      : inWindow.length > 0
        ? "shifted"
        : "elsewhere";
  return buildMatch(kind, m, claimedLine, finding.file, allMatches.length, isMultiLine);
}

// A lens that read the patch sometimes cites the patch's own line number
// instead of the file's. Measured 2026-10-22 over 298 lens findings checked
// against the file at the reviewed head: 68 cited the patch line, and some
// reached the final report as blocking findings pointing past the end of the
// file. The signature is exact, so this corrects only when all of it holds:
// the claimed line is a line of THIS file's diff, its text is one of the
// quote's lines, it stands for a different new-file line, and no diff line
// of this file already sits at the claimed number with a quote line on it.
// Unlike the first-line match above (FR2.5), every line of a multi-line quote
// counts here, because the patch line itself pins which one the lens meant.
// A lens also cites, in patch numbering, a line a few lines from its quote
// (20 of the 68). That is corrected too, but only when the quote sits within
// PATCH_LINE_NEAR lines of the claim read as a patch line and not within that
// distance of the claim read as a file line.
const PATCH_LINE_NEAR = 12;
export function patchLineCorrection(diffText, finding) {
  const claimedLine = normalizePositiveInt(finding && finding.line);
  const evidence = finding && finding.evidence;
  if (claimedLine == null || typeof evidence !== "string") return null;
  const quote = new Set(
    evidence
      .split("\n")
      .map(normalizeLine)
      .filter((l) => l && !isLowSpecificity(l)),
  );
  if (quote.size === 0) return null;
  const records = lookupFile(parseDiff(diffText), finding.file);
  if (!records) return null;
  const at = records.find((r) => r.diffLine === claimedLine);
  if (!at || at.newLine == null || at.newLine === claimedLine) return null;
  const quoted = records.filter((r) => quote.has(r.normalized));
  if (quoted.some((r) => r.newLine === claimedLine)) return null;
  if (quote.has(at.normalized)) return { correctedLine: at.newLine, patchLine: claimedLine };
  const nearAsPatch = quoted.some((r) => Math.abs(r.diffLine - claimedLine) <= PATCH_LINE_NEAR);
  const nearAsFile = quoted.some((r) => r.newLine != null && Math.abs(r.newLine - claimedLine) <= PATCH_LINE_NEAR);
  if (!nearAsPatch || nearAsFile) return null;
  return { correctedLine: at.newLine, patchLine: claimedLine };
}

// Batch entry point: guards the diff ONCE (FR1) so neither the CLI nor a
// direct module caller can produce per-finding verdicts against a diff that
// was never real (the #1911 failure mode — see the invalid-diff-banner
// fixture in eval/fixtures/evidence-resolver/). A patch-line citation is
// corrected first, and the quote is then resolved at the corrected line.
export function resolveEvidenceBatch(diffText, findings) {
  if (!diffIsUsable(diffText)) {
    return { ran: false, reason: "invalid-diff" };
  }
  const results = findings.map((c) => {
    try {
      const fix = patchLineCorrection(diffText, c);
      if (!fix) return { id: c.id, ...resolveEvidence(diffText, c) };
      return {
        id: c.id,
        ...resolveEvidence(diffText, { ...c, line: fix.correctedLine }),
        correctedLine: fix.correctedLine,
        patchLine: fix.patchLine,
      };
    } catch (err) {
      return { id: c.id, matchKind: "error", error: String(err && err.message) };
    }
  });
  return { ran: true, results };
}

async function main() {
  const argv = process.argv.slice(2);
  // --relay (v81): the Workflow verifies the copy-back by checksum (lib/relay.mjs).
  const relay = argv.includes("--relay");
  const emit = (obj) => process.stdout.write(relay ? relayLines(obj) : JSON.stringify(obj));
  const [diffPath, candidatesArg] = argv.filter((a) => a !== "--relay");
  if (!diffPath || !candidatesArg) {
    process.stderr.write(
      "usage: resolve-evidence-line.mjs <diff-path> <candidates-json-path|->\n",
    );
    process.exit(2);
  }

  let diffText;
  try {
    diffText = readFileSync(diffPath, "utf8");
  } catch {
    emit({ ran: false, reason: "diff-unreadable" });
    return;
  }

  let candidatesText;
  if (candidatesArg === "-") {
    candidatesText = readFileSync(0, "utf8");
  } else {
    candidatesText = readFileSync(candidatesArg, "utf8");
  }

  let candidates;
  try {
    candidates = JSON.parse(candidatesText);
  } catch {
    emit({ ran: false, reason: "candidates-unparseable" });
    return;
  }

  emit(resolveEvidenceBatch(diffText, candidates));
}

// Only run the CLI when invoked directly, not when imported (eval/replay-evidence-resolver.mjs).
if (isMainModule()) {
  main();
}
