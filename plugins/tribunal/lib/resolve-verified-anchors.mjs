#!/usr/bin/env node
// resolve-verified-anchors.mjs — does the code a `verifiedBy` anchor quotes
// actually exist at the commit it claims?
//
// `lib/verified.mjs` gates a blocking finding on having anchors, and checks
// their SHAPE: a non-empty path, a positive integer line, a quote of at least
// ten characters. All three fields assert facts about the repository and none
// of them was ever checked against it, so `{file: "src/x.ts", line: 1, quote:
// "const enum foo = 1"}` satisfied the gate completely. That module says as much
// in its own header — "it cannot tell whether an anchor actually supports the
// claim; it only makes skipping the lookup impossible to do silently." That
// justifies not judging SUPPORT. It does not justify never checking the quoted
// code exists.
//
// Why this reads GitHub and not a local clone. The first version ran
// `git show <ref>:<path>` against `process.cwd()`. On its first live run (PR
// #2083, 2026-09-08) all three anchors came back `file-not-at-ref` — every
// blocking finding apparently citing code that does not exist. The files were
// all there. The review's cwd is the workspace root, which is not a git
// repository at all, so every lookup failed and the failure was reported as a
// fact about the findings. Reading through `gh api` removes the dependency on
// where the process happens to be standing, and is the same pinned read the
// harness already requires of its own agents.
//
// Why this is NOT the diff resolver. `skills/review-pr/scripts/resolve-evidence-line.mjs`
// matches against the DIFF, which is right for `evidence` — that field quotes
// the line the diff changed. `verifiedBy` quotes the opposite thing on purpose
// (verified.mjs: "the code that has to be true for the claim to hold", the enum
// body, the call site, the schema field), which is usually code the diff never
// touched. Reusing the diff resolver here would report `not-found` on every
// correct anchor.
//
// Shadow mode. Nothing here blocks a finding, and PR #2083 is the argument for
// that: a defect in this file would otherwise have demoted every blocking
// finding in every review, silently and with a plausible-looking reason. The
// counts decide whether it ever becomes a gate.
//
// Usage:
//   node lib/resolve-verified-anchors.mjs <findings.json> <ref> --repo <owner/name> [--cross-repo <owner/name>@<ref>]...
//   … | node lib/resolve-verified-anchors.mjs - <ref> --repo <owner/name>
//
// Prints ONE line of JSON:
//   {ran, ref, repo, anchors, resolved, unresolved, suspectLookup, results:
//    [{findingIndex, anchorIndex, file, claimedLine, status, resolvedLine,
//      delta, note}]}
// Exit code is always 0: the classification is the output, not the status.

import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { relayLines } from "../rust/crates/tribunal-context/src/js/relay.mjs";

// Whitespace is the difference that matters least and breaks matching most.
// Quotes are transcribed by a model out of a file it read, so leading indent,
// run-together spaces and the three dash characters all drift. Nothing else is
// normalised: a changed identifier is a real mismatch and must stay one.
export function normalise(s) {
  return String(s == null ? "" : s)
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—]/g, "-")
    .replace(/\s+/g, " ")
    .trim();
}

// Statuses that mean "the lookup did not work", as opposed to "the lookup
// worked and the code is not there". Conflating the two is the whole reason
// this file was rewritten.
export const LOOKUP_FAILURES = new Set(["ref-not-found", "lookup-failed"]);

export function fileAtRef(repo, ref, file) {
  const path = String(file)
    .split("/")
    .map((seg) => encodeURIComponent(seg))
    .join("/");
  try {
    const text = execFileSync(
      "gh",
      [
        "api",
        "-H",
        "Accept: application/vnd.github.raw",
        `repos/${repo}/contents/${path}?ref=${encodeURIComponent(ref)}`,
      ],
      { encoding: "utf8", maxBuffer: 32 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] },
    );
    return { ok: true, text };
  } catch (err) {
    const stderr = String((err && err.stderr) || "");
    // A bad ref is a fact about the RUN, not about the anchor. Reported apart
    // because when it happens it happens to every anchor at once, and reading
    // that as "every finding cites missing code" is the failure this file has
    // already produced once.
    if (/No commit found for the ref/i.test(stderr))
      return { ok: false, status: "ref-not-found", detail: `no commit ${ref} in ${repo}` };
    if (/\(HTTP 404\)/.test(stderr))
      return { ok: false, status: "file-not-at-ref", detail: `${file} is not in ${repo} at ${ref}` };
    return {
      ok: false,
      status: "lookup-failed",
      detail: (stderr.trim() || String(err && err.message) || "gh failed").slice(0, 200),
    };
  }
}

// A quote may span several lines. Match it against a sliding window of the same
// number of lines rather than line by line, so a two-line anchor is found where
// it really is instead of failing on its first line alone.
export function locateQuote(text, quote) {
  const needle = normalise(quote);
  if (!needle) return [];
  const lines = String(text).split("\n");
  const span = Math.max(1, String(quote).trim().split("\n").length);
  const hits = [];
  for (let i = 0; i + span <= lines.length; i++) {
    const window = normalise(lines.slice(i, i + span).join(" "));
    if (window.includes(needle)) hits.push(i + 1);
  }
  // A multi-line quote whose span guess is wrong (the model reflowed it) gets a
  // second pass over a wider window.
  if (!hits.length && span > 1) {
    for (let i = 0; i < lines.length; i++) {
      const window = normalise(lines.slice(i, i + span + 2).join(" "));
      if (window.includes(needle)) hits.push(i + 1);
    }
  }
  return hits;
}

// A `verifiedBy` anchor from a review of one repo can legitimately
// cite code in its sibling — root CLAUDE.md's own cross-project convention
// names the FE↔BE contract as something a reviewer is expected to check, and
// the FE/BE methodology files both instruct crossing into the other repo when
// a change touches that contract. A finding that does this prefixes the
// anchor's `file` with the sibling's directory name, e.g.
// `<backend-repo>/prisma/schema.prisma` from an FE review — the same
// prefix convention the workspace root CLAUDE.md already uses for KB anchors.
// Passed straight to `fileAtRef` against the review's OWN repo and ref, that
// path is guaranteed not to exist: PR #1553 (2026-09-09) had all four of its
// BE-citing anchors reported `file-not-at-ref`, at the exact right line
// numbers, in a repo that was never the one holding them. The sibling has no
// reviewed commit of its own to pin to, so this resolves against its stable
// branch — the same ref root CLAUDE.md's own cross-project rule names for
// exactly this situation ("Docs cite the BE at development, not your working
// tree").
//
// Which repos are siblings, and the ref each resolves at, come from the project
// profile (profile.crossRepoAnchors), passed as repeated
// `--cross-repo <owner/name>@<ref>` flags. With none, every anchor resolves
// against the review's own repo.
export function splitCrossRepoAnchor(file, repo, crossRepoAnchors = []) {
  if (typeof file !== "string" || !Array.isArray(crossRepoAnchors)) return null;
  for (const sibling of crossRepoAnchors) {
    if (!sibling || typeof sibling.repo !== "string" || sibling.repo === repo) continue;
    const prefix = `${sibling.repo.split("/").pop()}/`;
    if (file.startsWith(prefix))
      return { repo: sibling.repo, ref: sibling.ref, file: file.slice(prefix.length) };
  }
  return null;
}

export function resolveAnchor(anchor, ref, repo, crossRepoAnchors = []) {
  const file = anchor && typeof anchor.file === "string" ? anchor.file : null;
  const claimedLine = Number.isInteger(anchor?.line) ? anchor.line : null;
  const base = { file, claimedLine, resolvedLine: null, delta: null, note: null };

  if (!file) return { ...base, status: "malformed", note: "no file on the anchor" };
  if (!anchor.quote || !normalise(anchor.quote))
    return { ...base, status: "malformed", note: "no quote on the anchor" };
  if (!repo) return { ...base, status: "lookup-failed", note: "no repo was given to look in" };

  const crossRepo = splitCrossRepoAnchor(file, repo, crossRepoAnchors);
  const lookupRepo = crossRepo ? crossRepo.repo : repo;
  const lookupRef = crossRepo ? crossRepo.ref : ref;
  const lookupFile = crossRepo ? crossRepo.file : file;

  const fetched = fileAtRef(lookupRepo, lookupRef, lookupFile);
  if (!fetched.ok)
    return {
      ...base,
      status: fetched.status,
      note: crossRepo
        ? `${fetched.detail} (resolved as a cross-repo anchor into ${lookupRepo}@${lookupRef})`
        : fetched.detail,
    };

  const hits = locateQuote(fetched.text, anchor.quote);
  if (!hits.length)
    return {
      ...base,
      status: "not-found",
      note: `the quoted code does not appear in ${lookupFile} at ${lookupRepo}@${lookupRef}`,
    };

  // Nearest hit to the claimed line, so a quote that legitimately appears twice
  // resolves to the occurrence the anchor meant rather than the first one.
  const resolvedLine =
    claimedLine == null
      ? hits[0]
      : hits.reduce((best, h) =>
          Math.abs(h - claimedLine) < Math.abs(best - claimedLine) ? h : best,
        );
  const delta = claimedLine == null ? null : resolvedLine - claimedLine;
  const multiHitNote =
    hits.length > 1
      ? `the quote appears ${hits.length} times in the file; resolved to the occurrence nearest the claimed line`
      : null;
  const crossRepoNote = crossRepo ? `resolved as a cross-repo anchor into ${lookupRepo}@${lookupRef}` : null;
  return {
    ...base,
    status: delta === 0 || delta === null ? "found" : "found-shifted",
    resolvedLine,
    delta,
    note:
      multiHitNote && crossRepoNote
        ? `${multiHitNote} (${crossRepoNote})`
        : multiHitNote || crossRepoNote,
  };
}

export function resolveVerifiedAnchors(findings, ref, repo, crossRepoAnchors = []) {
  if (!Array.isArray(findings))
    return { ran: false, reason: "findings-not-an-array", ref: ref ?? null, repo: repo ?? null, results: [] };
  if (!ref || typeof ref !== "string")
    return { ran: false, reason: "no-ref", ref: ref ?? null, repo: repo ?? null, results: [] };
  if (!repo || typeof repo !== "string")
    return { ran: false, reason: "no-repo", ref, repo: repo ?? null, results: [] };

  const results = [];
  findings.forEach((f, findingIndex) => {
    const anchors = Array.isArray(f?.verifiedBy) ? f.verifiedBy : [];
    anchors.forEach((a, anchorIndex) => {
      results.push({ findingIndex, anchorIndex, ...resolveAnchor(a, ref, repo, crossRepoAnchors) });
    });
  });
  const resolved = results.filter(
    (r) => r.status === "found" || r.status === "found-shifted",
  ).length;

  // The shape this file's own first live run produced: every anchor failing the
  // same way, because the lookup was misconfigured rather than because every
  // finding was fabricated. When it recurs, say so instead of handing a reader
  // a list that reads like mass fabrication.
  const failedLookup = results.filter((r) => LOOKUP_FAILURES.has(r.status)).length;
  const allMissing =
    results.length > 1 && results.every((r) => r.status === "file-not-at-ref");
  const suspectLookup = failedLookup > 0 || allMissing;

  return {
    ran: true,
    ref,
    repo,
    anchors: results.length,
    resolved,
    // Deliberately counted, not just derivable: "how many blocking findings
    // rest on an anchor nobody could find" is the number this exists to
    // produce, and a reader should not have to compute it.
    unresolved: results.length - resolved,
    // True when the failures look like a broken lookup rather than a finding
    // problem. Read `unresolved` only after reading this.
    suspectLookup,
    results,
  };
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

if (isMain) {
  // --relay (v81): the Workflow verifies the copy-back by checksum (relay.mjs).
  const relay = process.argv.includes("--relay");
  const emit = (obj) => process.stdout.write(relay ? relayLines(obj) : JSON.stringify(obj));
  const args = process.argv.slice(2).filter((a) => a !== "--relay");
  const flags = args.filter((a) => a.startsWith("--"));
  const positional = args.filter((a) => !a.startsWith("--"));
  const [path, ref] = positional;
  const repoFlagIdx = args.indexOf("--repo");
  const repo =
    repoFlagIdx >= 0 && args[repoFlagIdx + 1] && !args[repoFlagIdx + 1].startsWith("--")
      ? args[repoFlagIdx + 1]
      : (flags.find((f) => f.startsWith("--repo=")) || "").split("=")[1] || null;
  const crossRepoAnchors = [];
  args.forEach((a, i) => {
    const v =
      a === "--cross-repo" ? args[i + 1] : a.startsWith("--cross-repo=") ? a.slice(13) : null;
    const at = typeof v === "string" ? v.lastIndexOf("@") : -1;
    if (at > 0) crossRepoAnchors.push({ repo: v.slice(0, at), ref: v.slice(at + 1) });
  });

  if (!path || !ref || !repo) {
    emit({ ran: false, reason: "usage", ref: ref ?? null, repo, results: [] });
    process.exit(0);
  }
  let findings;
  try {
    findings = JSON.parse(readFileSync(path === "-" ? 0 : path, "utf8"));
  } catch {
    emit({ ran: false, reason: "findings-unparseable", ref, repo, results: [] });
    process.exit(0);
  }
  emit(
    resolveVerifiedAnchors(
      Array.isArray(findings) ? findings : findings?.findings,
      ref,
      repo,
      crossRepoAnchors,
    ),
  );
}
