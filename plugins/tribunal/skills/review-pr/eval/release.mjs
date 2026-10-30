#!/usr/bin/env node
// release.mjs — the gate a frozen version has to pass, and the ledger of what
// each freeze actually contained.
//
// WHAT THIS IS. A floor: the battery was green at this exact content hash.
// Nothing more. The battery is the Rust engine's: fmt, clippy and the whole
// test suite, the pinned binary built from the Rust source being frozen, and
// the replays of the JavaScript the Rust path still runs.
//
// WHAT THIS IS NOT — and the distinction is the whole reason the file says it
// out loud in its own output. This is NOT a quality score, and a green run here
// is NOT evidence that a change improved the review. Two measured facts forbid
// that reading: candidate generation reproduces at only 14-16% between identical
// runs (specs/2026-07-24-candidate-generation-noise-floor.md), so no single run
// can attribute a difference to a code change; and the corpus's only two
// labelled false negatives sit on a pre-canonical row that score.mjs skips, so
// the TP/FP/FN line below is thinner than it looks. The honest artifact of a
// freeze is the signed row it appends — including the defects known to be open
// at that moment — not a number that says "better".
//
// Usage:
//   node eval/release.mjs             # run the gate, print the verdict
//   node eval/release.mjs --record --tag tribunal/v44 --notes-file <path>
//
// Exits non-zero if anything in the battery is red. `--record` on a red gate is
// refused: a RELEASES.md row is a claim that the battery passed.
//
// `--record` also refuses without `--notes-file`, and that refusal is the point.
// This script used to write `see Known open` into the notes cell and print a
// reminder to fill it in; `release.sh` commits in the very next step, so the
// reminder always arrived after the commit it was about. Every row in
// RELEASES.md was therefore repaired by hand afterwards, or not at all. A cell
// nothing can write correctly is not a default, it is a defect with a note
// attached, so the gate now demands the text instead of asking for it later.
//
// `--tag` exists for the same reason one layer over. The tag for THIS freeze is
// created two steps AFTER the record, so deriving it from `git describe` can
// only ever yield `untagged` or, worse, the PREVIOUS release's tag — the exact
// mislabelling the comment on the derivation below was written to avoid, and
// which it avoided by naming nothing. release.sh knows the tag it is about to
// create; it passes it in.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { harnessVersion } from "../scripts/harness-version.mjs";
import { resolveBinary } from "../scripts/rust-engine.mjs";
import { HARNESS_DATA, RELEASES_PATH } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SKILL = resolve(HERE, "..");
const CLAUDE = resolve(SKILL, "../..");
const record = process.argv.includes("--record");
const argValue = (name) => {
  const prefixed = process.argv.find((a) => a.startsWith(`--${name}=`));
  if (prefixed) return prefixed.slice(name.length + 3);
  const i = process.argv.indexOf(`--${name}`);
  const next = i === -1 ? null : process.argv[i + 1];
  return next && !next.startsWith("--") ? next : null;
};
const portable = process.argv.includes("--portable");
const dataPresent = existsSync(HARNESS_DATA);
// The gate has two levels. Full needs the data directory (TRIBUNAL_HARNESS_DATA),
// which is not in this repository and holds the client terms replay-frontier
// scans for. Without it the scan is skipped, and a row from that run must say
// so, because "green" would otherwise claim more than was checked. So that level
// is never silent: it needs --portable, and the row's battery cell names it.
if (!dataPresent && !portable) {
  console.error(
    `\nrelease gate refused: the test-data directory ${HARNESS_DATA} does not exist.\n` +
      "  Set TRIBUNAL_HARNESS_DATA to it for the full gate, or pass --portable to run without\n" +
      "  the client-name scan, recorded as such in the row.",
  );
  process.exit(2);
}
if (dataPresent && portable) {
  console.error("\n--portable refused: the test-data directory exists, so run the full gate.");
  process.exit(2);
}
const tagArg = argValue("tag");
const notesFileArg = argValue("notes-file");

// Refuse BEFORE the battery runs. The battery takes tens of seconds and this is
// a usage error; finding out after paying for it is the kind of ordering the
// release path exists to make impossible.
if (record && !notesFileArg) {
  console.error(
    "\n--record refused: --notes-file <path> is required.\n" +
      "  The notes cell is what makes a freeze row worth keeping — what changed, what was\n" +
      "  measured, what stayed open. It cannot be defaulted and then fixed later, because\n" +
      "  release.sh commits immediately after this step.\n" +
      "  Use ./release.sh <N>, which collects it in $EDITOR and passes it here.",
  );
  process.exit(2);
}
// Same reasoning one step further: an unreadable or comment-only notes file is
// also a usage error, so it must not cost a battery run to discover either.
const readNotes = (path) =>
  readFileSync(path, "utf8")
    .split("\n")
    .filter((l) => !l.startsWith("#"))
    .join("\n")
    .trim()
    .replace(/\s*\n\s*/g, " ")
    .replace(/\|/g, "\\|");
let notes = "";
if (record) {
  try {
    notes = readNotes(notesFileArg);
  } catch (e) {
    console.error(`\n--record refused: cannot read ${notesFileArg} — ${e.message}`);
    process.exit(2);
  }
  if (!notes) {
    console.error(`\n--record refused: ${notesFileArg} has no content outside its # comment lines.`);
    process.exit(2);
  }
}

function run(label, args, { blocking = true } = {}) {
  try {
    const out = execFileSync("node", args, { cwd: SKILL, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    return { label, ok: true, blocking, out };
  } catch (err) {
    return { label, ok: false, blocking, out: `${err.stdout || ""}${err.stderr || ""}` };
  }
}

// The binary is the default engine, so the battery tests the binary: the Rust
// suite, and that the pinned release was built from the Rust source being
// frozen. The Workflow's replays are not in it. The Workflow is a fallback that
// only shrinks until it is deleted (plugins/tribunal/CLAUDE.md), so a change to it runs
// them by hand (docs/maintaining.md). The replays kept here cover the
// JavaScript the Rust path still runs: the launcher and its pin, and the
// client-name ratchet over every plugin file.
const RUST = join(CLAUDE, "rust");
const RUST_PATH_REPLAYS = ["replay-rust-engine.mjs", "replay-frontier.mjs"];

function runIn(label, cmd, args, cwd) {
  try {
    const out = execFileSync(cmd, args, {
      cwd,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { label, ok: true, blocking: true, out };
  } catch (err) {
    // stderr first: cargo's progress lines go there, and the failing tests and
    // their assertions, which the report's tail has to show, go to stdout.
    const missing = err.code === "ENOENT" ? `${cmd} is not installed\n` : "";
    return { label, ok: false, blocking: true, out: `${err.stderr || ""}${err.stdout || ""}${missing}` };
  }
}

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
  );
}
const pluginPath = (abs) => relative(CLAUDE, abs).split(sep).join("/");

// Files outside rust/ that the binary compiles in, read from its include_str!
// and include_bytes! calls so the list cannot drift from the code.
function embeddedOutsideRust() {
  const out = new Set();
  for (const crate of readdirSync(join(RUST, "crates"))) {
    const src = join(RUST, "crates", crate, "src");
    if (!existsSync(src)) continue;
    for (const file of walk(src).filter((f) => f.endsWith(".rs"))) {
      for (const m of readFileSync(file, "utf8").matchAll(/include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)/g)) {
        const rel = pluginPath(resolve(dirname(file), m[1]));
        if (!rel.startsWith("rust/")) out.add(rel);
      }
    }
  }
  return [...out].sort();
}

// What changes the binary: everything under rust/ except the docs, the tests
// and the pin itself, plus the files it embeds from elsewhere in the plugin.
const shipsInBinary = (rel) =>
  !rel.endsWith(".md") && rel !== "rust/release.json" && !/^rust\/crates\/[^/]+\/tests\//.test(rel);

function pinCheck() {
  const label = "rust-pin";
  const fail = (why) => ({ label, ok: false, blocking: true, out: `${why}\n` });
  const git = (args) =>
    execFileSync("git", args, { cwd: CLAUDE, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  let tag;
  try {
    tag = JSON.parse(readFileSync(join(RUST, "release.json"), "utf8")).tag;
  } catch (e) {
    return fail(`cannot read rust/release.json: ${e.message}`);
  }
  if (!tag) return fail("rust/release.json pins no release");
  let commit;
  try {
    commit = git(["rev-parse", "--verify", "--quiet", `${tag}^{commit}`]);
  } catch {
    return fail(`the pinned tag ${tag} is not in this clone; run git fetch origin tag ${tag}`);
  }
  // Against the working tree, untracked files included: release.sh records the
  // row before it commits.
  const paths = ["rust", ...embeddedOutsideRust()];
  const changed = [
    ...git(["diff", "--name-only", "--relative", commit, "--", ...paths]).split("\n"),
    ...git(["ls-files", "--others", "--exclude-standard", "--", ...paths]).split("\n"),
  ]
    .filter(Boolean)
    .filter(shipsInBinary);
  if (changed.length) {
    return fail(
      `the pinned binary ${tag} was built without these changes:\n` +
        changed.map((p) => `  ${p}`).join("\n") +
        '\n  Release a new tribunal-rust binary and pin it (rust/README.md, "A release").',
    );
  }
  // A fresh data directory, so the archive is downloaded and checked against the
  // pin now rather than read from a cache an earlier run verified.
  const scratch = mkdtempSync(join(tmpdir(), "tribunal-gate-"));
  try {
    const env = { ...process.env, TRIBUNAL_DATA_DIR: scratch };
    delete env.TRIBUNAL_BIN;
    delete env.TRIBUNAL_RUST_MANIFEST;
    const r = resolveBinary({ env });
    if (!r.ok) return fail(`the pinned binary does not resolve: ${r.error}`);
    const printed = execFileSync(r.bin, ["--version"], { encoding: "utf8" }).trim();
    const want = tag.replace(/^tribunal-rust-v/, "");
    if (!printed.split(/\s+/).includes(want)) return fail(`${tag} prints "${printed}", not version ${want}`);
    return { label, ok: true, blocking: true, out: `${tag}: ${printed}, built from this source\n` };
  } catch (e) {
    return fail(`the pinned binary did not run: ${e.message}`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

const { id, entries } = harnessVersion();
// The battery has its own id because the row's central claim is about the
// battery, and the harness hash never covered it: `pr-harness-4af7ff70ff8e` is
// the same id `main` carried while `replay-embedded-bash` was red, so "green at
// this hash" could not be re-verified by checking the hash out. Two ids, and the
// row records both. The battery is the Rust tests, the replays above and this
// file; `rust-battery-` marks rows recorded since the battery moved off the
// Workflow's replays (`pr-battery-`).
const battery = (() => {
  const files = [
    ...walk(join(RUST, "crates")).filter((f) => /[/\\]tests[/\\]/.test(f)),
    ...RUST_PATH_REPLAYS.map((f) => join(HERE, f)),
    fileURLToPath(import.meta.url),
  ];
  const fileEntries = files
    .map((f) => ({ path: pluginPath(f), sha: createHash("sha256").update(readFileSync(f)).digest("hex") }))
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  const manifest = fileEntries.map((e) => `${e.path}:${e.sha}`).join("\n");
  return { id: `rust-battery-${createHash("sha256").update(manifest).digest("hex").slice(0, 12)}`, entries: fileEntries };
})();
console.log(
  `peer-review release gate\n  version   ${id}\n  surface   ${entries.length} files\n  battery   ${battery.id}  (${battery.entries.length} files)`,
);

// A freeze that does not cover the agent definitions is not a freeze. They were
// missing from the hash until 2026-07-27, so `peer-review/v1` through `v10`
// froze the workflow, the prose and `lib/` while leaving the 240-line prompt
// that shapes every lens free to change with no version bump. Checked here
// rather than trusted, because the omission was invisible for ten releases and
// the file claiming to cover "the behavioural surface" is the one that had it.
const agentFiles = entries.filter((e) => e.path.startsWith("agents/"));
if (!agentFiles.length) {
  console.log(
    "\nGATE RED — the surface contains no agent definition.\n" +
      "  Every lens, the bot vetting, the critic and the prior-finding verifier are\n" +
      "  dispatched into an agent whose prompt would then sit outside the freeze.",
  );
  process.exit(1);
}

// --- the battery ------------------------------------------------------------
// The Rust suite takes minutes, so each step says when it starts.
const steps = [
  () => runIn("cargo fmt", "cargo", ["fmt", "--all", "--check"], RUST),
  () => runIn("cargo clippy", "cargo", ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"], RUST),
  () => runIn("cargo test", "cargo", ["test", "--workspace", "--locked", "--no-fail-fast"], RUST),
  () => pinCheck(),
  ...RUST_PATH_REPLAYS.map((f) => () => run(f.replace(/\.mjs$/, ""), [`eval/${f}`])),
];
const results = [];
console.log("\n  running");
for (const step of steps) {
  const r = step();
  console.log(`    ${r.ok ? "ok  " : "FAIL"}  ${r.label}`);
  results.push(r);
}
// The corpus score REPORTS, it does not gate. It is too thin to block on today
// and pretending otherwise would be the kind of gate that lies (see the header).
const score = portable ? { out: "" } : run("score", ["eval/score.mjs"], { blocking: false });

const red = results.filter((r) => !r.ok);
const totals = (score.out.match(/^\s*TP=.*$/m) || ["  (score unavailable)"])[0].trim();
console.log(`\n  corpus    ${totals}  [reported, not gated]`);

if (red.length) {
  console.log(`\nGATE RED — ${red.length} failing:`);
  for (const r of red) {
    console.log(`\n--- ${r.label} ---`);
    // cargo test's tail is the doc tests that ran after the failure, so show
    // the failing tests, their panics and cargo's summary of them instead.
    const lines = r.out.trim().split("\n");
    const failing = lines.filter((l) => /^test \S+ \.\.\. FAILED$|panicked at/.test(l));
    const summary = lines.filter((l) => /^error: \d+ targets? failed|^ {4}`-p /.test(l));
    const shown = failing.length ? [...failing.slice(0, 30), ...summary] : lines.slice(-12);
    console.log(shown.join("\n"));
  }
  if (record) console.log("\n--record refused: a RELEASES.md row asserts the battery passed.");
  process.exit(1);
}

console.log(`\nGATE GREEN — ${results.length}/${results.length} checks pass at ${id}${portable ? " (portable gate: the client-name scan did not run)" : ""}.`);
console.log(
  "This means nothing regressed. It does NOT mean the review got better:\n" +
    "  candidate generation reproduces at 14-16% between identical runs, so a\n" +
    "  quality delta cannot be read from any single run, green gate or not.",
);

if (!record) {
  // "then tag it yourself" was the original rule, and v5/v6 sat unfrozen for
  // hours because of it. The freeze is record -> commit -> tag, one routine.
  console.log(
    "\n(run with --record to append the RELEASES.md row, then commit and tag — the tag is part of the freeze, not a separate favour)",
  );
  process.exit(0);
}

// --- the ledger row ---------------------------------------------------------
// The tag on HEAD describes what is COMMITTED. When the tree is dirty the
// version being recorded is not that commit, so reporting HEAD's tag would
// label this row with the PREVIOUS release — which is exactly what the first
// v2 recording did, stamping `peer-review/v1` onto v2's content. A row that
// cannot name its tag says `pending` instead of naming the wrong one.
let tag = tagArg ? `\`${tagArg}\`` : "untagged";
try {
  if (tagArg) throw new Error("supplied by the caller");
  const dirty = execFileSync("git", ["status", "--porcelain"], {
    cwd: CLAUDE,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
  tag = dirty
    ? "pending (uncommitted at record time)"
    : execFileSync("git", ["describe", "--tags", "--exact-match"], {
        cwd: CLAUDE,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      }).trim();
} catch {
  /* no repo, or HEAD carries no tag — recorded honestly rather than guessed */
}
// The date comes from the shell, not from a hardcoded constant, and the row is
// APPENDED — a freeze ledger that can be rewritten in place is not a ledger.
const date = new Date().toISOString().slice(0, 10);
const RELEASES = RELEASES_PATH;
if (!existsSync(RELEASES)) {
  writeFileSync(
    RELEASES,
    `# Frozen versions of the peer-review harness

Appended by \`node eval/release.mjs --record\`, one row per freeze. A row asserts
exactly one thing: **the offline battery was green at that content hash.** It is
not a quality claim — see the header of \`eval/release.mjs\` for why no single run
can support one.

The **Known open** section below the table is what earns this file: what is
understood to be broken or unmeasured *right now*. It is one living list, edited in
place — a freeze that claims nothing is open is almost always a freeze that did not
look.

| Version | Date | Tag | Battery | Corpus (not gated) | Notes |
|---|---|---|---|---|---|
`,
  );
}
// Insert INTO the freeze table, not at the end of the file. A plain append put the
// row after the per-version prose sections, where it rendered as loose text outside
// any table — found the first time a second version was recorded.
//
// Anchor on a FREEZE row (`| \`pr-harness-…`), not on "the last line starting with
// a pipe". The file now holds a second table above this one (the ⚠ legend) and the
// Known open section below it, so "last pipe line" is one added table away from
// silently filing the freeze in the wrong place — the same bug the paragraph above
// records, one structure later.
// `notes` was read and validated before the battery ran (see the guard at the
// top): one markdown table row, so newlines are already collapsed into one cell
// and literal pipes escaped, or the row would silently grow columns.
// The battery id goes in the BATTERY cell, not a seventh column: 56 rows already
// exist with six, and it belongs beside the count whose claim it makes checkable.
// The corpus totals are not written to the row: the corpus is external data.
const gateCell = portable ? `${results.length}/${results.length} portable (client-name scan not run)` : `${results.length}/${results.length} full`;
const row = `| \`${id}\` | ${date} | ${tag} | ${gateCell} @ \`${battery.id}\` | n/a | ${notes} |`;
const lines = readFileSync(RELEASES, "utf8").split("\n");
let last = -1;
for (let i = 0; i < lines.length; i++) if (/^\| `pr-harness-/.test(lines[i])) last = i;
if (last === -1) {
  appendFileSync(RELEASES, `${row}\n`);
} else {
  lines.splice(last + 1, 0, row);
  writeFileSync(RELEASES, lines.join("\n"));
}
console.log(`\nrecorded in RELEASES.md: ${tag}, ${notes.length} chars of notes.`);
