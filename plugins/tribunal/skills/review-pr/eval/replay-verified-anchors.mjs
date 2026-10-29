#!/usr/bin/env node
// replay-verified-anchors.mjs — resolving a `verifiedBy` anchor against the
// code it claims to quote.
//
// The defect (criterion SO-17). `verifiedBy[]` gates whether a finding may
// publish as Blocking, and all three of its fields assert facts about the
// repository: a path, a line, and a word-for-word quote at HEAD.
// `lib/verified.mjs` checked only their shape, so a wholly fabricated anchor
// satisfied the gate.
//
// The SECOND defect, which is why this file was rewritten (PR #2083,
// 2026-09-08, the resolver's first live run). It read the file with `git show
// <ref>:<path>` against `process.cwd()`. A review's cwd is the workspace root,
// which is not a git repository, so every lookup failed and every anchor came
// back `file-not-at-ref` — a metrics row stating that all three blocking
// findings cited code that does not exist. The files were all there. Reading
// through `gh api` removes the dependency on where the process is standing.
//
// So this replay stubs `gh` on PATH rather than building a git repository. That
// is deliberate: the bug was in how the lookup was invoked, and a fixture that
// bypasses the invocation cannot see it. The stub answers exactly as the real
// `gh` does, including the two 404 shapes that mean different things.
//
// Usage: node eval/replay-verified-anchors.mjs

import { chmodSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { WORKFLOW } from "./paths.mjs";
import { clientProfileByReviewer, loadClientProfiles } from "./client-profiles.mjs";
import {
  LOOKUP_FAILURES,
  fileAtRef,
  locateQuote,
  normalise,
  resolveAnchor,
  resolveVerifiedAnchors,
  splitCrossRepoAnchor,
} from "../../../lib/resolve-verified-anchors.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// --- a fake `gh` that answers like the real one -----------------------------
// Serves files from a fixture directory keyed by repo+ref, and reproduces the
// three real failure shapes measured against github.com on 2026-09-08:
//   missing file  -> exit 1, stderr "gh: Not Found (HTTP 404)"
//   missing ref   -> exit 1, stderr "gh: No commit found for the ref <sha> (HTTP 404)"
//   anything else -> exit 1, some other stderr
const shim = mkdtempSync(join(tmpdir(), "gh-shim-"));
mkdirSync(join(shim, "bin"), { recursive: true });
const FIXTURES = join(shim, "fixtures");
mkdirSync(FIXTURES, { recursive: true });

const REPO = "acme/widgets";
const HEAD = "9ea21de88bb3b335272302eb49df878f5f9ef145";
const BASE = "1111111111111111111111111111111111111111";

const TYPES_AT_HEAD = `// a header comment added at head, shifting everything below it
export enum upload_type {
  spreadsheet = 'spreadsheet',
  manual = 'manual',
}

export function classify(row) {
  return row.type;
}
`;
const TYPES_AT_BASE = `export enum upload_type {
  spreadsheet = 'spreadsheet',
  manual = 'manual',
}
`;

const BE_REPO = clientProfileByReviewer("backend").profile.repo;
const FE_REPO = clientProfileByReviewer("frontend").profile.repo;
const BE_NAME = BE_REPO.split("/").pop();
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const DEV_REF = "development";
const SCHEMA_AT_DEV = `enum invoice_result_status {
  pending
  success
  error
}
`;

const put = (repo, ref, file, text) => {
  const dir = join(FIXTURES, repo, ref, ...file.split("/").slice(0, -1));
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(FIXTURES, repo, ref, file), text);
};
put(REPO, HEAD, "src/types.ts", TYPES_AT_HEAD);
put(REPO, HEAD, "src/added.ts", "export const NEW = 1;\n");
put(REPO, BASE, "src/types.ts", TYPES_AT_BASE);
put(BE_REPO, DEV_REF, "prisma/schema.prisma", SCHEMA_AT_DEV);
put(FE_REPO, HEAD, "src/types.ts", TYPES_AT_HEAD);

writeFileSync(
  join(shim, "bin", "gh"),
  `#!/usr/bin/env node
const { existsSync, readFileSync } = require("node:fs");
const { join } = require("node:path");
const FIX = ${JSON.stringify(FIXTURES)};
const KNOWN_REFS = ${JSON.stringify([HEAD, BASE, DEV_REF])};
const url = process.argv.find((a) => a.startsWith("repos/"));
if (!url) { process.stderr.write("gh: bad invocation\\n"); process.exit(1); }
const m = url.match(/^repos\\/([^/]+\\/[^/]+)\\/contents\\/(.+?)\\?ref=(.+)$/);
if (!m) { process.stderr.write("gh: bad invocation\\n"); process.exit(1); }
const [, repo, rawPath, ref] = m;
if (repo === "acme/unreachable") { process.stderr.write("gh: connection refused\\n"); process.exit(1); }
if (!KNOWN_REFS.includes(ref)) {
  process.stderr.write("gh: No commit found for the ref " + ref + " (HTTP 404)\\n");
  process.exit(1);
}
const file = rawPath.split("/").map(decodeURIComponent).join("/");
const p = join(FIX, repo, ref, file);
if (!existsSync(p)) { process.stderr.write("gh: Not Found (HTTP 404)\\n"); process.exit(1); }
process.stdout.write(readFileSync(p, "utf8"));
`,
);
chmodSync(join(shim, "bin", "gh"), 0o755);
process.env.PATH = `${join(shim, "bin")}:${process.env.PATH}`;

const at = (anchor, ref = HEAD, repo = REPO) => resolveAnchor(anchor, ref, repo);
const PROFILES = loadClientProfiles();
const crossFor = (r) => PROFILES[r].crossRepoAnchors;

console.log(`\nverifiedBy anchor resolution — gh stubbed at ${join(shim, "bin")}`);

console.log(`\n--- the stub answers like the real gh ---`);
{
  // If this block is wrong every result below is meaningless, so it is asserted
  // rather than assumed.
  const ok = at({ file: "src/types.ts", line: 2, quote: "export enum upload_type {" });
  check("a known file at a known ref is readable", ok.status === "found", JSON.stringify(ok));
  const missing = at({ file: "src/nope.ts", line: 1, quote: "whatever it says here" });
  check("a missing file maps to file-not-at-ref", missing.status === "file-not-at-ref");
  const badRef = at({ file: "src/types.ts", line: 2, quote: "export enum upload_type {" }, "deadbeef");
  check("a missing ref maps to ref-not-found, not to file-not-at-ref", badRef.status === "ref-not-found", JSON.stringify(badRef));
  const dead = at({ file: "src/types.ts", line: 2, quote: "export enum upload_type {" }, HEAD, "acme/unreachable");
  check("any other gh failure maps to lookup-failed", dead.status === "lookup-failed", JSON.stringify(dead));
}

console.log(`\n--- the fabrication the shape gate could not see ---`);
{
  const fabricated = at({ file: "src/types.ts", line: 1, quote: "const enum foo = 1" });
  check("a fabricated quote is not found", fabricated.status === "not-found");
  check(
    "…and 'the quote is absent' stays distinct from 'the file is absent'",
    fabricated.status !== "file-not-at-ref",
  );
  const nearMiss = at({ file: "src/types.ts", line: 2, quote: "export enum upload_TYPE {" });
  check("a single changed identifier is a real mismatch", nearMiss.status === "not-found");
}

console.log(`\n--- "at the ref" is load-bearing ---`);
{
  const atHead = at({ file: "src/added.ts", line: 1, quote: "export const NEW = 1;" });
  const atBase = at({ file: "src/added.ts", line: 1, quote: "export const NEW = 1;" }, BASE);
  check("a file the PR adds resolves at head", atHead.status === "found");
  check("…and correctly does not at the base commit", atBase.status === "file-not-at-ref");
  const untouched = at({ file: "src/types.ts", line: 3, quote: "manual = 'manual'," });
  check(
    "code the diff never touched still resolves — the reason this is not the diff resolver",
    ["found", "found-shifted"].includes(untouched.status),
    JSON.stringify(untouched),
  );
}

console.log(`\n--- drift is reported, not treated as failure ---`);
{
  const shifted = at({ file: "src/types.ts", line: 1, quote: "export enum upload_type {" });
  check("a right quote at a wrong line is found-shifted", shifted.status === "found-shifted", JSON.stringify(shifted));
  check("the drift is quantified", shifted.delta === 1);
}

console.log(`\n--- whitespace tolerance is narrow and deliberate ---`);
{
  check("indentation is normalised", normalise("   a   b  ") === "a b");
  check("smart quotes fold to straight ones", normalise("‘x’") === "'x'");
  check("an em dash folds to a hyphen", normalise("a — b") === "a - b");
  check(
    "an anchor quoted with different indentation still resolves",
    at({ file: "src/types.ts", line: 3, quote: "      spreadsheet = 'spreadsheet'," }).status !== "not-found",
  );
  check("an empty quote locates nothing", locateQuote("a\nb\n", "   ").length === 0);
}

console.log(`\n--- a broken lookup must not read as mass fabrication (PR #2083) ---`);
{
  // The exact shape the live run produced: three anchors on real files, every
  // one reported missing, because the lookup could not work at all.
  const findings = [
    { title: "a", verifiedBy: [{ file: "src/types.ts", line: 2, quote: "export enum upload_type {" }] },
    { title: "b", verifiedBy: [{ file: "src/added.ts", line: 1, quote: "export const NEW = 1;" }] },
  ];

  const healthy = resolveVerifiedAnchors(findings, HEAD, REPO);
  check("a healthy run resolves both", healthy.resolved === 2 && healthy.unresolved === 0);
  check("…and does not cry wolf", healthy.suspectLookup === false, JSON.stringify(healthy.suspectLookup));

  const badRepo = resolveVerifiedAnchors(findings, HEAD, "acme/unreachable");
  check("an unreachable repo leaves nothing resolved", badRepo.unresolved === 2);
  check("…and is flagged as a suspect lookup", badRepo.suspectLookup === true);
  check(
    "…via a status that says the lookup failed, not that the code is missing",
    badRepo.results.every((r) => LOOKUP_FAILURES.has(r.status)),
    JSON.stringify(badRepo.results.map((r) => r.status)),
  );

  const badRef = resolveVerifiedAnchors(findings, "deadbeef", REPO);
  check("a wrong ref is flagged too", badRef.suspectLookup === true);
  check(
    "…and named as the ref's fault, which is a fact about the run",
    badRef.results.every((r) => r.status === "ref-not-found"),
  );

  // The subtler case: gh works, the ref is fine, and every single anchor is
  // still missing. That is either mass fabrication or a repo mix-up, and the
  // second is likelier, so it is flagged rather than asserted either way.
  const wrongRepoShape = resolveVerifiedAnchors(
    [
      { verifiedBy: [{ file: "src/gone-a.ts", line: 1, quote: "something plausible here" }] },
      { verifiedBy: [{ file: "src/gone-b.ts", line: 1, quote: "something plausible here" }] },
    ],
    HEAD,
    REPO,
  );
  check(
    "every anchor missing at once is flagged as suspect, not reported as fabrication",
    wrongRepoShape.suspectLookup === true &&
      wrongRepoShape.results.every((r) => r.status === "file-not-at-ref"),
  );
  check(
    "…but ONE missing anchor among several found ones is not",
    resolveVerifiedAnchors(
      [
        { verifiedBy: [{ file: "src/types.ts", line: 2, quote: "export enum upload_type {" }] },
        { verifiedBy: [{ file: "src/gone.ts", line: 1, quote: "something plausible here" }] },
      ],
      HEAD,
      REPO,
    ).suspectLookup === false,
  );
}

console.log(`\n--- refusals, and the argument the CLI needs ---`);
{
  check("a missing ref refuses", resolveVerifiedAnchors([], null, REPO).reason === "no-ref");
  check("a missing repo refuses rather than guessing one", resolveVerifiedAnchors([], HEAD, null).reason === "no-repo");
  check(
    "…which is the whole bug: with no repo it must not silently read the cwd",
    resolveVerifiedAnchors([], HEAD, null).ran === false,
  );
  check(
    "a non-array of findings refuses",
    resolveVerifiedAnchors("nope", HEAD, REPO).reason === "findings-not-an-array",
  );
  check(
    "a malformed anchor is its own status",
    resolveVerifiedAnchors([{ verifiedBy: [{ line: 1, quote: "xxxxxxxxxx" }] }], HEAD, REPO).results[0].status ===
      "malformed",
  );
}

console.log(`\n--- cross-repo anchors resolve into the sibling, not the review's own repo (PR #1553) ---`);
{
  const anchor = { file: `${BE_NAME}/prisma/schema.prisma`, line: 1, quote: "enum invoice_result_status {" };

  check(
    "the prefix is recognised for a repo with a known sibling",
    JSON.stringify(splitCrossRepoAnchor(anchor.file, FE_REPO, crossFor(FE_REPO))) ===
      JSON.stringify({ repo: BE_REPO, ref: DEV_REF, file: "prisma/schema.prisma" }),
  );
  check(
    "a file with no sibling prefix is left alone",
    splitCrossRepoAnchor("src/foo.ts", FE_REPO, crossFor(FE_REPO)) === null,
  );
  check(
    "a repo with no known sibling is left alone, not guessed at",
    splitCrossRepoAnchor(anchor.file, "acme/widgets", []) === null,
  );
  // INTENDED DIFFERENCE: the sibling is no longer known from a constant, so
  // without the profile's anchors the prefix is not guessed.
  check(
    "with no crossRepoAnchors passed, the sibling prefix is not guessed",
    splitCrossRepoAnchor(anchor.file, FE_REPO) === null,
  );

  const resolved = resolveAnchor(anchor, HEAD, FE_REPO, crossFor(FE_REPO));
  check(
    "an FE review resolves a BE-prefixed anchor by reading the BE repo at development, not the FE repo at its own head",
    resolved.status === "found",
    JSON.stringify(resolved),
  );
  check("the note says which repo it actually resolved against", new RegExp(`${escapeRe(BE_NAME)}@${DEV_REF}`).test(resolved.note ?? ""));

  // Control: the exact call PR #1553 made before this fix existed — the raw
  // BE-prefixed path, looked up against the FE repo at the FE head, with none
  // of the cross-repo redirection above. This is what "file-not-at-ref" for a
  // real, correctly-cited line looked like, and it is the failure this fix
  // removes rather than papers over.
  const withoutFix = fileAtRef(FE_REPO, HEAD, anchor.file);
  check(
    "…and confirms that WITHOUT the redirection this exact anchor 404s, which is what PR #1553 actually saw",
    withoutFix.ok === false && withoutFix.status === "file-not-at-ref",
    JSON.stringify(withoutFix),
  );

  const findings = [
    { title: "BE contract gap", verifiedBy: [anchor] },
    { title: "same-repo finding", verifiedBy: [{ file: "src/types.ts", line: 2, quote: "export enum upload_type {" }] },
  ];
  const mixed = resolveVerifiedAnchors(findings, HEAD, FE_REPO, crossFor(FE_REPO));
  check("a cross-repo anchor and a same-repo anchor both resolve in the same run", mixed.resolved === 2 && mixed.unresolved === 0, JSON.stringify(mixed.results.map((r) => r.status)));
}

console.log(`\n--- wiring: the dispatch must hand it the repo ---`);
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "the run passes --repo, without which every anchor fails",
    /resolve-verified-anchors\.mjs \$\{anchorsFile\} "\$\{ref\}" --repo "\$\{repo\}"/.test(src),
  );
  check(
    "the run passes the profile's cross-repo anchors to the resolver",
    /--relay\$\{crossRepoFlags\(PROFILE\)\}/.test(src),
  );
  check("it reads at the head ref", /const ref = prContext\.headRefOid;/.test(src));
  check(
    "v81: the anchors go down verified and come back as a verified relay",
    src.includes("...verifiedWrite(anchorsFile, payload,") &&
      /resolve-verified-anchors\.mjs \$\{anchorsFile\} "\$\{ref\}" --repo "\$\{repo\}" --relay/.test(src) &&
      /const out = anchorRelay\.ok \? anchorRelay\.data : null;/.test(src) &&
      !/VERIFIED_ANCHOR_SCHEMA/.test(src),
  );
  check("the metrics row carries suspectLookup", /suspectLookup: brokenLookup,/.test(src));
  check(
    "the log leads with the suspicion instead of the count",
    /the pattern says the LOOKUP failed, not the anchors/.test(src),
  );
  check(
    "the run recomputes the flag rather than trusting the relay",
    /out\.results\.some\(\(r\) => LOOKUP_FAILED\.has\(r\.status\)\)/.test(src),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall verified-anchor expectations hold");
