// refuter-package.mjs — can a refuter be handed what it needs instead of going
// to fetch it?
//
// The measured problem. A refuter costs 1.12M input tokens and 23 turns to emit
// one yes/no about one claim, and in those 23 turns it opens ~3 distinct files.
// That is ~8 turns per file: it is not exploring, it is grepping and re-reading a
// small set, and every turn re-pays the whole conversation. The floor per turn is
// 20.6k (system prompt + tool schemas) and the measured average is ~54k.
//
// Why the refuter and not the lens. Both have the same pathology, but the
// arithmetic only closes here. A package is billed EVERY turn, so a 135k lens
// package takes cost/turn from 71k to 156k and needs turns to fall 54% just to
// break even — which is exactly what a declared tool-call budget failed to
// achieve. A refuter's package is small enough to stay under 54k/turn even
// carried on every turn, so it can win at CONSTANT turns.
//
// WHAT THIS SCRIPT MEASURED, against the estimate that motivated it. The
// analysis assumed a 13k package (20.6k + 13k = 34k/turn, a claimed ~16x). The
// real files are 5-7k each, so:
//
//   cited-in-pr             ~20k  →  41k/turn   55% of accesses covered
//   cited-anywhere          ~29k  →  49k/turn   75% covered
//   cited-anywhere+schema   ~41k  →  61k/turn   78% covered  — LOSES money
//
// So the mechanism works and the size estimate did not. At constant turns the
// best variant saves ~24% per refuter turn, which is 6-8% of corpus input — a
// real, quality-neutral saving, and roughly a fifth of what was projected.
//
// The counter-intuitive part is the ranking: the SMALLEST package wins. Going
// from 55% to 78% coverage costs more per turn than the fetches it avoids, so
// coverage past a point is negative value. Package size dominates coverage —
// which is the same lesson the lens case taught, arriving from the other side.
//
// AND THE OBVIOUS NEXT MOVE IS ALSO REFUTED. "Package regions, not whole files"
// followed from the size lesson and does not survive its own measurement:
//
//   · 85% of a refuter's RANGED reads (309 of 363) are in files other than the
//     one the finding lives in. A region cut around the finding's line addresses
//     the other 15%.
//   · The own file averages 630 lines / ~4.9k tokens — it is already the small
//     part of a ~20k package. Cutting it to +/-80 lines saves ~3.2k, i.e. ~16% of
//     the package, and costs 43% of the own-file ranged reads.
//   · The distribution does not support a tight radius anyway: median distance 0
//     but p75 = 235 lines and p90 = 351, so +/-300 (which keeps 85% of the file
//     and saves almost nothing) is what it would take to cover 81%.
//
// So the package is dominated by the CITED files, and those are exactly where
// the positioned reading happens — cutting them costs coverage where it counts.
// `cited-in-pr` at ~20k is close to the floor this mechanism offers, and the
// lever is what it measured: -6 to -8% of corpus input, not an order of magnitude.
//
// What this file measures, for zero tokens: given only what the harness already
// knows at dispatch time, would the package have contained the files the refuter
// actually opened? Coverage is scored against the reads recorded in the
// transcripts of runs already on disk. It cannot tell you whether the refuter
// would have reached the same VERDICT from the package — that needs one Tier-1
// agent, and this is the evidence that decides whether to spend it.
//
// Three assembler variants are scored side by side rather than one, because the
// difference between them IS the finding: the harness's existing `citedFiles`
// only resolves paths that are in the PR, and a refuter's job takes it to call
// sites and schema that are not.
//
// Usage: node eval/refuter-package.mjs [--verbose] [--json]

import { readFileSync, existsSync, readdirSync } from "node:fs";
import { join, resolve, basename } from "node:path";
import { execFileSync } from "node:child_process";
import { findRunDirs } from "./agent-cost.mjs";
import { readRun, parseRefuterBatchCases, parseSingleRefuterCase } from "./refine-attribution.mjs";
import { projectDir, reviewRepo, isMainModule } from "./paths.mjs";

const PROJECT = projectDir();
const REPO = reviewRepo();

// ---------------------------------------------------------------------------
// Pure
// ---------------------------------------------------------------------------

// The same pattern citedFiles() uses in peer-review.js. Kept as its own export
// so the two can be compared rather than assumed identical: this module's whole
// point is that the harness's version is restricted to PR files and a refuter's
// reads are not.
const PATH_TOKEN = /[\w./-]+\.(?:ts|tsx|js|jsx|mts|cts|prisma|sql|json)\b/g;

export function citedTokens(text) {
  const hits = String(text || "").match(PATH_TOKEN);
  return hits ? [...new Set(hits)] : [];
}

// A read as recorded in a transcript, reduced to a repo-relative path. Returns
// null for anything that is not a repo file — a temp path, a URL fragment, a
// flag that happened to look like a path. Extensions are REQUIRED here: the
// looser form used in the redundancy count admits non-files and inflates the
// number (37% vs 43% on the same data), and this measurement compares reads to a
// file list, where a non-file is not a miss but noise.
export function normalizeReadPath(raw) {
  // A trailing `:<line>` is stripped first. A finding's `file` carries one
  // (`scripts/orders/cancel-orders.ts:233`) and so does `git show <sha>:<path>`
  // output, and a path with a line number resolves against no file list and no
  // git tree — it silently becomes an uncovered read and an unsized package
  // entry, which biases coverage DOWN and package size DOWN at the same time.
  const s = String(raw || "")
    .trim()
    .replace(/:\d+(?:-\d+)?$/, "");
  if (!s) return null;
  const m = /((?:src|prisma|scripts|test|tests)\/[\w./-]+\.(?:ts|tsx|js|jsx|mts|cts|prisma|sql|json))\b/.exec(s);
  if (m) return m[1];
  // A bare filename with a known extension still counts — `schema.prisma` shows
  // up on its own constantly — but anything without an extension does not.
  const bare = /^([\w.-]+\.(?:ts|tsx|js|jsx|mts|cts|prisma|sql|json))$/.exec(basename(s));
  return bare ? bare[1] : null;
}

// Which files an agent opened, in order, with a count per path. Covers the two
// ways it happens: the Read tool, and bash (`git show <sha>:<path>`, cat, head,
// sed -n, grep -n … <path>).
// Reads that name a LINE RANGE, which is what decides whether a package can be
// built out of regions instead of whole files. Two forms carry one, and together
// they are most of the positioned reads a refuter makes (measured over the
// corpus: 1,360 `sed -n 'A,Bp'` + 369 `Read` with an offset, against 919
// whole-file `cat`/`Read`):
//
//   Read  {file_path, offset, limit}
//   Bash  sed -n '100,180p' <path>
//
// `grep -n` is deliberately NOT counted as a ranged read. It is the LOCATE step
// — 1,827 of them, the single most common call — and it returns a line number,
// not a region of content. Counting it as a read of the line it happens to
// report would credit a region package with content the agent never received.
export function extractRangedReads(text) {
  const out = [];
  for (const line of String(text || "").split("\n")) {
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
      if (!b || b.type !== "tool_use" || !b.input) continue;
      if (b.name === "Read" && b.input.file_path && Number(b.input.offset) > 0) {
        const p = normalizeReadPath(b.input.file_path);
        const start = Number(b.input.offset);
        const limit = Number(b.input.limit) > 0 ? Number(b.input.limit) : 2000;
        if (p) out.push({ path: p, start, end: start + limit - 1 });
        continue;
      }
      if (b.name === "Bash" && b.input.command) {
        const cmd = String(b.input.command);
        const re = /sed -n\s+['"]?(\d+),(\d+)p['"]?\s+([^\s|&;'"]+)/g;
        let m;
        while ((m = re.exec(cmd))) {
          const p = normalizeReadPath(m[3]);
          if (p) out.push({ path: p, start: Number(m[1]), end: Number(m[2]) });
        }
      }
    }
  }
  return out;
}

// How far a read sits from the finding's line. 0 when the window contains it —
// which is the case a region package would have served without any fetch.
export function distanceFromLine(read, line) {
  if (!Number.isFinite(line) || line <= 0) return null;
  if (line >= read.start && line <= read.end) return 0;
  return line < read.start ? read.start - line : line - read.end;
}

export function extractReads(text) {
  const counts = new Map();
  const order = [];
  for (const line of String(text || "").split("\n")) {
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
      if (!b || b.type !== "tool_use" || !b.input) continue;
      const raws = [];
      if (b.name === "Read" && b.input.file_path) raws.push(b.input.file_path);
      else if (b.name === "Grep" && b.input.path) raws.push(b.input.path);
      else if (b.name === "Bash" && b.input.command) {
        const cmd = String(b.input.command);
        // Every path-shaped token in the command, not just the first: one bash
        // call routinely touches several files, and taking only the first
        // undercounts what the agent actually needed.
        for (const t of cmd.match(/[\w./:-]+\.(?:ts|tsx|js|jsx|mts|cts|prisma|sql|json)\b/g) || [])
          raws.push(t);
      }
      for (const raw of raws) {
        const p = normalizeReadPath(raw);
        if (!p) continue;
        if (!counts.has(p)) order.push(p);
        counts.set(p, (counts.get(p) || 0) + 1);
      }
    }
  }
  return order.map((p) => ({ path: p, count: counts.get(p) }));
}

// Resolve a cited token against a file list, the way citedFiles does: exact
// match, or a suffix match on a full path (`utilities.ts` → `src/lib/helpers/utilities.ts`).
// An ambiguous suffix (two files with the same basename) resolves to NOTHING
// rather than to the first hit — guessing here would credit the package with a
// file the refuter never opened.
export function resolveToken(token, files) {
  const exact = files.find((f) => f === token);
  if (exact) return exact;
  const suffix = files.filter((f) => f.endsWith(`/${token}`));
  return suffix.length === 1 ? suffix[0] : null;
}

export const VARIANTS = ["cited-in-pr", "cited-anywhere", "cited-anywhere+schema"];

// Deterministic, ~0 tokens: what would be handed to a refuter for this finding.
// `prFiles` is the PR's changed-file list (the harness has it at dispatch);
// `repoFiles` is the tree at the reviewed commit.
export function assemblePackage(finding, { prFiles = [], repoFiles = [], variant = "cited-in-pr" } = {}) {
  // A misspelled variant THROWS rather than falling through. The variant name
  // was only ever compared against "cited-in-pr", so anything unrecognised
  // silently behaved as the widest one — a typo in a caller would have reported
  // one variant's numbers under another variant's name, which is the failure
  // mode this whole module exists to avoid.
  if (!VARIANTS.includes(variant))
    throw new Error(`unknown package variant "${variant}" — expected one of ${VARIANTS.join(", ")}`);
  const reasons = new Map();
  const add = (p, why) => {
    if (!p) return;
    if (!reasons.has(p)) reasons.set(p, why);
  };

  const ownRaw = normalizeReadPath(finding.file) || String(finding.file || "").replace(/:\d+$/, "");
  const own = resolveToken(ownRaw, prFiles) || resolveToken(ownRaw, repoFiles) || ownRaw;
  add(own, "the finding's own file");

  const tokens = citedTokens(`${finding.description || ""} ${finding.evidence || ""} ${finding.title || ""}`);
  for (const t of tokens) {
    const inPr = resolveToken(t, prFiles);
    if (inPr) {
      add(inPr, "cited, in the PR");
      continue;
    }
    if (variant === "cited-in-pr") continue;
    const anywhere = resolveToken(t, repoFiles);
    if (anywhere) add(anywhere, "cited, elsewhere in the repo");
  }

  // schema.prisma is the single most-read file in the corpus and a refuter
  // reaches for it whenever a claim touches a column, an enum or a relation —
  // often without the description naming the file. Adding it unconditionally
  // would be the wrong trade (85 KB, ~21k tokens, more than the whole rest of a
  // package), so this variant adds it only on a prisma-shaped claim.
  if (variant === "cited-anywhere+schema") {
    const text = `${finding.description || ""} ${finding.title || ""}`.toLowerCase();
    if (/prisma|schema|migration|column|enum|@map|@default|@relation/.test(text)) {
      const schema = repoFiles.find((f) => f.endsWith("prisma/schema.prisma"));
      add(schema, "prisma-shaped claim");
    }
  }

  return { paths: [...reasons.keys()], reasons };
}

// Coverage is scored in ACCESSES, not in distinct files: a file the refuter
// opened six times is six turns the package would have removed, and counting it
// once understates the mechanism. Both numbers are reported.
export function scoreCoverage(pkgPaths, reads) {
  const pkg = new Set(pkgPaths);
  const inPkg = reads.filter((r) => pkg.has(r.path));
  const outPkg = reads.filter((r) => !pkg.has(r.path));
  const accesses = reads.reduce((s, r) => s + r.count, 0);
  const accessesCovered = inPkg.reduce((s, r) => s + r.count, 0);
  return {
    distinctRead: reads.length,
    distinctCovered: inPkg.length,
    accesses,
    accessesCovered,
    outOfPackage: outPkg.map((r) => r.path),
    fullyCovered: reads.length > 0 && outPkg.length === 0,
    readNothing: reads.length === 0
  };
}

// ---------------------------------------------------------------------------
// fs shell
// ---------------------------------------------------------------------------

const treeCache = new Map();
function repoFilesAt(ref) {
  if (treeCache.has(ref)) return treeCache.get(ref);
  let files = [];
  try {
    files = execFileSync("git", ["-C", REPO, "ls-tree", "-r", "--name-only", ref], {
      encoding: "utf8",
      maxBuffer: 1 << 26,
      stdio: ["ignore", "pipe", "ignore"]
    })
      .split("\n")
      .filter(Boolean);
  } catch {
    files = [];
  }
  treeCache.set(ref, files);
  return files;
}

const sizeCache = new Map();
function fileBytes(ref, path) {
  const key = `${ref}:${path}`;
  if (sizeCache.has(key)) return sizeCache.get(key);
  let bytes = null;
  try {
    bytes = Number(
      execFileSync("git", ["-C", REPO, "cat-file", "-s", `${ref}:${path}`], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"]
      }).trim()
    );
  } catch {
    bytes = null;
  }
  sizeCache.set(key, bytes);
  return bytes;
}

const lineCache = new Map();
function fileLineCount(ref, path) {
  const key = `${ref}:${path}`;
  if (lineCache.has(key)) return lineCache.get(key);
  let lines = null;
  try {
    lines = execFileSync("git", ["-C", REPO, "show", `${ref}:${path}`], {
      encoding: "utf8",
      maxBuffer: 1 << 26,
      stdio: ["ignore", "pipe", "ignore"]
    }).split("\n").length;
  } catch {
    lines = null;
  }
  lineCache.set(key, lines);
  return lines;
}

const REFUTER_BATCH = /^You are refuting \d+ INDEPENDENT code-review findings on PR #/;
const REFUTER_SINGLE = /^REFUTE — do not confirm — this code review finding/;

function firstPromptAndText(file) {
  const text = readFileSync(file, "utf8");
  let prompt = "";
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
    const s = typeof c === "string" ? c : Array.isArray(c) ? c.map((x) => x?.text || "").join(" ") : "";
    if (s.trim()) {
      prompt = s;
      break;
    }
  }
  return { prompt, text };
}

if (isMainModule(import.meta.url)) {
  const VERBOSE = process.argv.includes("--verbose");
  const AS_JSON = process.argv.includes("--json");

  const observations = [];
  let runsSeen = 0;
  let refutersSeen = 0;
  let unresolvedTrees = 0;
  let approximatedPrFileLists = 0;
  let unresolvableReads = 0;

  for (const dir of findRunDirs(PROJECT)) {
    const run = readRun(dir);
    if (!run || !run.embeddedRow) continue;
    runsSeen++;
    const row = run.embeddedRow;
    // The PR's real changed-file list, as the fetch-pr agent returned it. An
    // earlier version of this script approximated it from the row's finding
    // paths, which is a far shorter list and made `cited-in-pr` — the variant
    // that models what the harness computes TODAY — look worse than it is. The
    // fallback is reported, not silently used.
    const prFiles = run.prFiles.length
      ? run.prFiles
      : (row.findings || []).map((f) => normalizeReadPath(f.file)).filter(Boolean);
    if (!run.prFiles.length) approximatedPrFileLists++;

    const ref = row.headSha || "development";
    let repoFiles = repoFilesAt(ref);
    if (!repoFiles.length) {
      repoFiles = repoFilesAt("development");
      unresolvedTrees++;
    }
    if (!repoFiles.length) continue;

    for (const file of readdirSync(dir)) {
      if (!/^agent-\w+\.jsonl$/.test(file)) continue;
      const { prompt, text } = firstPromptAndText(join(dir, file));
      const head = prompt.trim();
      const isBatch = REFUTER_BATCH.test(head);
      if (!isBatch && !REFUTER_SINGLE.test(head)) continue;
      const cases = isBatch
        ? parseRefuterBatchCases(prompt)
        : [parseSingleRefuterCase(prompt)].filter(Boolean);
      if (!cases.length) continue;
      refutersSeen++;

      // The prompt carries a truncated description, so the assembler is fed the
      // text the refuter itself was given — the same information the harness
      // has at dispatch. Nothing here reads the finding's full row.
      const enriched = cases.map((c) => {
        const bodyStart = prompt.indexOf(c.title);
        const body = bodyStart === -1 ? "" : prompt.slice(bodyStart, bodyStart + 2500);
        return { ...c, description: body };
      });

      // A read whose path resolves to no real file at the reviewed commit is
      // NOISE, not an uncovered read: a glob (`*.test.ts`), a bare basename the
      // regex clipped out of a longer command, a path that never existed. Left
      // in, each one counts as a miss and pushes coverage down for free.
      // Reads are also CANONICALISED to the full repo path before scoring. An
      // agent writes `cancel-orders.ts` as often as the full path, and comparing
      // a bare basename against a package that holds
      // `scripts/orders/cancel-orders.ts` scores a hit as a miss — the same file
      // appearing on both sides under two spellings.
      const allReads = extractReads(text);
      const byCanon = new Map();
      let noise = 0;
      for (const r of allReads) {
        const canon = resolveToken(r.path, repoFiles);
        if (!canon) {
          noise++;
          continue;
        }
        byCanon.set(canon, (byCanon.get(canon) || 0) + r.count);
      }
      const reads = [...byCanon.entries()].map(([path, count]) => ({ path, count }));
      unresolvableReads += noise;

      // Distance from each case's own line to every ranged read in that case's
      // own file. Only the OWN file: a cited file's relevant line is not known
      // at dispatch, so a region for it cannot be proposed, let alone scored.
      const ranged = extractRangedReads(text);
      const lineDistances = [];
      let rangedOwnFile = 0;
      let rangedOtherFile = 0;
      let ownFileSize = null;
      for (const c of enriched) {
        const own = normalizeReadPath(c.file);
        const lineMatch = /:(\d+)\s*$/.exec(String(c.file || ""));
        const line = lineMatch ? Number(lineMatch[1]) : NaN;
        if (!own || !Number.isFinite(line) || line <= 0) continue;
        const canonOwn = resolveToken(own, repoFiles) || own;
        if (!ownFileSize) {
          const bytes = fileBytes(ref, canonOwn);
          const lines = fileLineCount(ref, canonOwn);
          if (bytes && lines) ownFileSize = { lines, tokens: bytes / 4 };
        }
        for (const r of ranged) {
          const canonRead = resolveToken(r.path, repoFiles) || r.path;
          if (canonRead !== canonOwn) {
            rangedOtherFile++;
            continue;
          }
          rangedOwnFile++;
          const d = distanceFromLine(r, line);
          if (d !== null) lineDistances.push(d);
        }
      }
      const perVariant = {};
      for (const variant of VARIANTS) {
        const paths = new Set();
        for (const c of enriched)
          for (const p of assemblePackage(c, { prFiles, repoFiles, variant }).paths) paths.add(p);
        const list = [...paths];
        const bytes = list.map((p) => fileBytes(ref, p)).filter((b) => typeof b === "number");
        perVariant[variant] = {
          files: list.length,
          approxTokens: Math.round(bytes.reduce((s, b) => s + b, 0) / 4),
          unsizedFiles: list.length - bytes.length,
          ...scoreCoverage(list, reads)
        };
      }
      observations.push({
        pr: row.pr,
        runId: run.runId,
        agent: file,
        batchSize: cases.length,
        reads: reads.length,
        lineDistances,
        ownFileSize,
        rangedOwnFile,
        rangedOtherFile,
        perVariant
      });
    }
  }

  if (AS_JSON) {
    console.log(JSON.stringify({ runsSeen, refutersSeen, observations }, null, 2));
    process.exit(0);
  }

  const pct = (a, b) => (b > 0 ? `${((100 * a) / b).toFixed(0)}%` : "—");
  console.log(`\nREFUTERS MEASURED`);
  console.log(`  runs with their own row : ${runsSeen}`);
  console.log(`  refuter dispatches      : ${refutersSeen}`);
  console.log(`  tree resolved from 'development' instead of the reviewed sha : ${unresolvedTrees}`);
  console.log(`  runs whose PR file list had to be approximated : ${approximatedPrFileLists}`);
  console.log(`  read paths discarded as unresolvable noise     : ${unresolvableReads}`);

  const withReads = observations.filter((o) => o.reads > 0);
  console.log(`  dispatches that opened at least one file : ${withReads.length} of ${observations.length}`);
  const batches = observations.filter((o) => o.batchSize > 1).length;
  console.log(`  batched dispatches (package is the union of its cases) : ${batches}`);

  console.log(`\nCOVERAGE — would the package have held what the refuter opened?\n`);
  console.log(`  variant                  files  ~tokens   accesses covered   files covered   fully covered`);
  for (const v of VARIANTS) {
    const os = withReads.map((o) => o.perVariant[v]);
    const files = os.reduce((s, x) => s + x.files, 0) / (os.length || 1);
    const toks = os.reduce((s, x) => s + x.approxTokens, 0) / (os.length || 1);
    const acc = os.reduce((s, x) => s + x.accesses, 0);
    const accCov = os.reduce((s, x) => s + x.accessesCovered, 0);
    const dis = os.reduce((s, x) => s + x.distinctRead, 0);
    const disCov = os.reduce((s, x) => s + x.distinctCovered, 0);
    const full = os.filter((x) => x.fullyCovered).length;
    console.log(
      `  ${v.padEnd(24)} ${files.toFixed(1).padStart(5)} ${Math.round(toks).toLocaleString("en-US").padStart(8)}` +
        `   ${pct(accCov, acc).padStart(15)}   ${pct(disCov, dis).padStart(13)}   ${pct(full, os.length).padStart(13)}`
    );
  }

  // What is missing is the next rule of the assembler, and it is data rather
  // than intuition — the same argument the analysis makes for outOfPackageReads
  // as a runtime signal.
  const missing = new Map();
  for (const o of withReads)
    for (const p of o.perVariant["cited-anywhere+schema"].outOfPackage)
      missing.set(p, (missing.get(p) || 0) + 1);
  console.log(`\nMOST-READ FILES THE BEST VARIANT STILL MISSES`);
  for (const [p, n] of [...missing.entries()].sort((a, b) => b[1] - a[1]).slice(0, 15))
    console.log(`  ${String(n).padStart(4)}  ${p}`);

  console.log(`\nPER-TURN ARITHMETIC (floor 20.6k, measured refuter average 54k/turn)`);
  for (const v of VARIANTS) {
    const os = withReads.map((o) => o.perVariant[v]);
    const toks = os.reduce((s, x) => s + x.approxTokens, 0) / (os.length || 1);
    console.log(
      `  ${v.padEnd(24)} ~${Math.round(toks).toLocaleString("en-US")} tokens → ${Math.round(20600 + toks).toLocaleString("en-US")}/turn ` +
        `(${20600 + toks < 54000 ? "cheaper than today per turn" : "MORE EXPENSIVE per turn — needs turns to fall"})`
    );
  }

  // The bottom line, stated as a share of the whole bill rather than as a
  // per-turn ratio, because a per-turn ratio is what made this look like a 16x
  // win. Refuters are 24-33% of corpus input (24% excluding PR #1911, 33% inside
  // it), so the run-level saving is the per-turn saving times that share — and
  // only if turns hold, which is the assumption the analysis insisted on.
  console.log(`\nBOTTOM LINE, at CONSTANT turns (refuters are 24-33% of corpus input)`);
  for (const v of VARIANTS) {
    const os = withReads.map((o) => o.perVariant[v]);
    const toks = os.reduce((s, x) => s + x.approxTokens, 0) / (os.length || 1);
    const perTurn = 20600 + toks;
    const saving = 1 - perTurn / 54000;
    const lo = saving * 0.24;
    const hi = saving * 0.33;
    const sign = saving >= 0 ? "-" : "+";
    console.log(
      `  ${v.padEnd(24)} per-turn ${sign}${Math.abs(100 * saving).toFixed(0)}%` +
        `  →  corpus ${sign}${Math.abs(100 * lo).toFixed(1)}% to ${sign}${Math.abs(100 * hi).toFixed(1)}%`
    );
  }
  console.log(
    `\n  The measured package is ${Math.round(
      withReads.reduce((s, o) => s + o.perVariant["cited-in-pr"].approxTokens, 0) / (withReads.length || 1) / 1000
    )}-${Math.round(
      withReads.reduce((s, o) => s + o.perVariant["cited-anywhere"].approxTokens, 0) / (withReads.length || 1) / 1000
    )}k tokens, NOT the 13k the analysis assumed. 13k was "the 3 files a`
  );
  console.log(`  refuter reads" priced from nothing — the real files are 5-7k each.`);

  // --- regions instead of whole files ---------------------------------------
  //
  // The design corollary the whole-file measurement produced: package size
  // dominates coverage, so the next move is to send the REGION around the
  // finding rather than its entire file. What can be measured offline is how far
  // a refuter's ranged reads land from the finding's own line — i.e. whether a
  // window of ±R would have contained what it went to fetch.
  //
  // The limit, stated before the numbers: this covers reads in the finding's OWN
  // file that named a line range. A whole-file `cat` is excluded because it
  // reveals nothing about which region was needed, and `grep -n` is excluded
  // because it is the locate step, not a read of content. So this bounds R for
  // the own-file part of a package; it says nothing about the cited files, which
  // stay whole-file here.
  console.log(`\nREGIONS INSTEAD OF WHOLE FILES — how far do ranged reads land from the finding's line?`);
  const RADII = [40, 80, 150, 300];
  const dists = [];
  for (const o of observations)
    for (const d of o.lineDistances || []) dists.push(d);
  if (!dists.length) console.log(`  no ranged reads in the finding's own file — nothing to bound R with`);
  else {
    const sorted = [...dists].sort((a, b) => a - b);
    const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
    console.log(`  ranged reads in the finding's own file : ${dists.length}`);
    console.log(`  already inside the read window (dist 0) : ${pct(dists.filter((d) => d === 0).length, dists.length)}`);
    console.log(`  median ${at(0.5)} lines   p75 ${at(0.75)}   p90 ${at(0.9)}   max ${sorted[sorted.length - 1]}`);
    console.log(`\n  radius R   reads a +/-R window would already contain`);
    for (const R of RADII)
      console.log(`  +/-${String(R).padStart(4)}   ${pct(dists.filter((d) => d <= R).length, dists.length)}`);
  }

  // Where the positioned reading actually happens. This turned out to be the
  // finding of the section: a refuter does most of its ranged reading in files
  // that are NOT the one the finding lives in, so a region cut around the
  // finding's line addresses the minority of it.
  const ownRanged = observations.reduce((n, o) => n + (o.rangedOwnFile || 0), 0);
  const otherRanged = observations.reduce((n, o) => n + (o.rangedOtherFile || 0), 0);
  console.log(`\n  ranged reads in the finding's OWN file  : ${ownRanged}`);
  console.log(`  ranged reads in some OTHER file        : ${otherRanged} (${pct(otherRanged, ownRanged + otherRanged)})`);
  console.log(`  A region cut around the finding's line only addresses the first number.`);

  // And the cost side: a region is only worth it if it is much smaller than the
  // file it replaces. Measured against the own-file entries of the winning
  // variant, at the radius the distribution actually calls for.
  console.log(`\n  own-file size vs a region, at the radius the distribution calls for`);
  const ownSizes = [];
  for (const o of observations) if (o.ownFileSize) ownSizes.push(o.ownFileSize);
  if (!ownSizes.length) console.log(`    (own-file sizes not collected)`);
  else {
    const avgLines = ownSizes.reduce((s, x) => s + x.lines, 0) / ownSizes.length;
    const avgTok = ownSizes.reduce((s, x) => s + x.tokens, 0) / ownSizes.length;
    console.log(`    average own file: ${Math.round(avgLines)} lines, ~${Math.round(avgTok).toLocaleString("en-US")} tokens`);
    for (const R of RADII) {
      const frac = ownSizes.map((x) => Math.min(1, (2 * R + 1) / Math.max(1, x.lines)));
      const avgFrac = frac.reduce((s, x) => s + x, 0) / frac.length;
      console.log(
        `    +/-${String(R).padStart(4)}  keeps ${(100 * avgFrac).toFixed(0)}% of the file  ~${Math.round(avgTok * avgFrac).toLocaleString("en-US")} tokens` +
          `   covers ${pct(dists.filter((d) => d <= R).length, dists.length || 1)} of own-file ranged reads`
      );
    }
  }

  console.log(`\nWHAT THIS DOES NOT ESTABLISH`);
  console.log(`  · Whether a refuter given the package reaches the SAME VERDICT. That is the`);
  console.log(`    Tier-1 experiment; this only decides whether it is worth running.`);
  console.log(`  · Whether turns actually fall. The arithmetic above deliberately assumes they`);
  console.log(`    do NOT — the package has to win at constant turns or it does not win.`);
  console.log(`  · Coverage is scored against what the refuter READ, which is not the same as`);
  console.log(`    what it NEEDED: it may have read files it did not use, and a package that`);
  console.log(`    covers 100% of reads still cannot prove it covers 100% of need.\n`);
}
