#!/usr/bin/env node
// release.mjs — the gate a frozen version has to pass, and the ledger of what
// each freeze actually contained.
//
// WHAT THIS IS. A floor: the offline battery was green at this exact content
// hash. Nothing more.
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
import { appendFileSync, existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { batteryVersion, harnessVersion, surfaceFiles } from "../scripts/harness-version.mjs";
import { HARNESS_DATA, NEEDS_DATA_PATH, RELEASES_PATH } from "./paths.mjs";

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
const needsData = new Set(
  existsSync(NEEDS_DATA_PATH)
    ? readFileSync(NEEDS_DATA_PATH, "utf8")
        .split("\n")
        .filter((l) => l.startsWith("tribunal "))
        .map((l) => l.slice("tribunal ".length).trim())
    : [],
);
// The gate has two levels. Full needs the data directory (TRIBUNAL_HARNESS_DATA),
// which holds real past runs and is not in this repository. Without it only the
// portable replays can run, and a row from that run must say so, because "green"
// would otherwise claim more than was checked. So that level is never silent: it
// needs --portable, and the row's battery cell names it.
if (!dataPresent && !portable) {
  console.error(
    `\nrelease gate refused: the test-data directory ${HARNESS_DATA} does not exist.\n` +
      "  Set TRIBUNAL_HARNESS_DATA to it for the full gate, or pass --portable to run only the\n" +
      `  replays that need no data (${needsData.size} are skipped), recorded as such in the row.`,
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

const { id, entries } = harnessVersion();
// The battery has its own id because the row's central claim is about the
// battery, and the harness hash never covered it: `pr-harness-4af7ff70ff8e` is
// the same id `main` carried while `replay-embedded-bash` was red, so "green at
// this hash" could not be re-verified by checking the hash out. Folding eval/
// into the harness hash would have fixed that by breaking something else, since
// a test-only edit would then move the field that exists to make runs
// comparable. Two ids, and the row records both.
const battery = batteryVersion();
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
const results = [run("check-mirror", ["eval/check-mirror.mjs"])];
let skippedForData = 0;
for (const f of readdirSync(join(SKILL, "eval")).filter((f) => /^replay-.*\.mjs$/.test(f)).sort()) {
  if (portable && needsData.has(f)) {
    skippedForData++;
    continue;
  }
  results.push(run(f.replace(/\.mjs$/, ""), [`eval/${f}`]));
}
// The corpus score REPORTS, it does not gate. It is too thin to block on today
// and pretending otherwise would be the kind of gate that lies (see the header).
const score = portable ? { out: "" } : run("score", ["eval/score.mjs"], { blocking: false });

console.log("\n  battery");
for (const r of results) console.log(`    ${r.ok ? "ok  " : "FAIL"}  ${r.label}`);

const red = results.filter((r) => !r.ok);
const totals = (score.out.match(/^\s*TP=.*$/m) || ["  (score unavailable)"])[0].trim();
console.log(`\n  corpus    ${totals}  [reported, not gated]`);

if (red.length) {
  console.log(`\nGATE RED — ${red.length} failing:`);
  for (const r of red) {
    console.log(`\n--- ${r.label} ---`);
    console.log(r.out.trim().split("\n").slice(-12).join("\n"));
  }
  if (record) console.log("\n--record refused: a RELEASES.md row asserts the battery passed.");
  process.exit(1);
}

console.log(`\nGATE GREEN — ${results.length}/${results.length} offline checks pass at ${id}${portable ? ` (portable gate: ${skippedForData} replays that need the data directory were not run)` : ""}.`);
console.log(
  "This means nothing regressed offline. It does NOT mean the review got better:\n" +
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
const gateCell = portable ? `${results.length}/${results.length} portable (${skippedForData} need data, not run)` : `${results.length}/${results.length} full`;
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
