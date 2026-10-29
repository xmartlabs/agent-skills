// corpus-candidates.mjs — turn "label everything" into "confirm or reject these".
//
// The ground-truth corpus is the only instrument that can answer whether the
// panel's kills are CORRECT, and it holds one labelled entry. It is also the only
// question a model cannot close for itself: a finding adjudicated by another
// agent is model grading model, which is what the panel already does and exactly
// what a quality question puts in doubt. Ground truth has to come from outside —
// a hotfix, a commit by the author that fixes the flagged thing, or a human
// saying so.
//
// What this script does is cheap and mechanical: for every finding this harness
// has posted or marked blocking, go and look at what happened to that file
// AFTERWARDS, and print the finding with that evidence attached. It proposes
// nothing and labels nothing. A human reads the pairs and decides.
//
// The signal it looks for, per finding:
//
//   · open PR    — commits pushed to the PR's own branch after the review, that
//                  touch the flagged file. A likely `author-fixed`: the reviewer
//                  said X about file F and the author then changed F.
//   · merged PR  — commits on the base branch after the merge that touch the
//                  flagged file. A likely `production-incident` or a follow-up
//                  fix, and the strongest label available.
//
// Both are HINTS, not labels. A commit touching the file may be unrelated, and
// silence may mean the finding was wrong OR that nobody got to it. That is why
// the output is a queue for a person and not a corpus entry.
//
// Usage: node eval/corpus-candidates.mjs [--repo <owner/name>] [--pr <n>] [--limit <n>] [--json]

import { readFileSync, existsSync } from "node:fs";
import { canonicalRows, summary } from "./rows.mjs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { METRICS_PATH } from "./paths.mjs";
import { operatorConfigPath, resolveRepo } from "../../../lib/operator.mjs";

const METRICS = METRICS_PATH;

const argv = process.argv.slice(2);
const arg = (name) => {
  const i = argv.indexOf(name);
  return i === -1 ? null : argv[i + 1];
};
// --repo, else defaultRepo from the operator config. No cwd fallback: these
// numbers are about the repo the ledger rows describe, not wherever this ran.
const resolveLedgerRepo = (flagValue) => {
  const r = resolveRepo({ flag: flagValue || "", cwdRepoOf: () => null });
  if (r.repo) return r.repo;
  const why = r.error.startsWith("no repo")
    ? `no repo: pass --repo <owner/name> or set "defaultRepo" in ${operatorConfigPath()}`
    : r.error;
  console.error(`corpus-candidates: ${why}`);
  process.exit(2);
};
const REPO = resolveLedgerRepo(arg("--repo"));
const onlyPr = arg("--pr") ? Number(arg("--pr")) : null;
const limit = arg("--limit") ? Number(arg("--limit")) : 40;
const asJson = argv.includes("--json");

const gh = (args) => {
  try {
    return JSON.parse(execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }));
  } catch {
    return null;
  }
};

if (!existsSync(METRICS)) {
  console.error("no metrics.jsonl");
  process.exit(1);
}
// canonicalRows(): 5 of the 48 rows predate the canonical shape and are
// documented as not comparable. Reading the file raw is how that warning got
// ignored by six of eleven consumers — see eval/rows.mjs.
const rows = canonicalRows();

// One entry per (pr, file, title) — the same finding raised by several runs is
// ONE thing to adjudicate, not several. Keeps the most recent run's verdict.
const byKey = new Map();
for (const r of rows) {
  if (!r.pr || (onlyPr && r.pr !== onlyPr)) continue;
  for (const f of r.findings || []) {
    if (!f.posted && !f.blocking) continue;
    const key = `${r.pr}@@${f.file}@@${f.title}`;
    const prev = byKey.get(key);
    if (prev && String(prev.ts) > String(r.ts)) continue;
    byKey.set(key, {
      pr: r.pr,
      ts: r.ts,
      headSha: r.headSha,
      title: f.title || "(legacy row: this finding has no title)",
      file: f.file || "",
      severity: f.severity,
      survived: !!f.survived,
      refuteFraction: f.refuteFraction,
      posted: !!f.posted,
      blocking: !!f.blocking
    });
  }
}

const prMeta = new Map();
const metaFor = (pr) => {
  if (prMeta.has(pr)) return prMeta.get(pr);
  const m = gh([
    "pr",
    "view",
    String(pr),
    "--repo",
    REPO,
    "--json",
    "number,state,mergedAt,baseRefName,headRefName,mergeCommit"
  ]);
  prMeta.set(pr, m);
  return m;
};

const candidates = [...byKey.values()].slice(0, limit);
const out = [];
for (const c of candidates) {
  const meta = metaFor(c.pr);
  if (!meta) {
    out.push({ ...c, evidence: null, note: "gh could not read this PR" });
    continue;
  }
  // The file the reviewer named, without any `:line` — the commits API needs a
  // path, and a path with a line number matches nothing (silently, returning an
  // empty list that would read as "nobody touched it").
  const path = String(c.file || "").replace(/:\d+$/, "");
  const merged = !!meta.mergedAt;
  // The PR's own merge commit shares the merge timestamp, so `since=mergedAt`
  // includes it — and a PR "fixed by itself" is not evidence of anything. It did
  // not fire on today's data (both hits are hours or a month later), which is
  // exactly why it is worth excluding before it does.
  const ownMerge = meta.mergeCommit && meta.mergeCommit.oid ? meta.mergeCommit.oid.slice(0, 8) : null;
  const ref = merged ? meta.baseRefName : meta.headRefName;
  const since = merged ? meta.mergedAt : c.ts;
  const commits =
    path && since
      ? gh([
          "api",
          `repos/${REPO}/commits?path=${encodeURIComponent(path)}&sha=${encodeURIComponent(ref)}&since=${encodeURIComponent(since)}`,
          "--jq",
          "[.[] | {sha: .sha[0:8], date: .commit.author.date, msg: (.commit.message | split(\"\\n\")[0])}]"
        ])
      : null;
  out.push({
    ...c,
    state: meta.state,
    mergedAt: meta.mergedAt || null,
    watched: { ref, since, path },
    evidence: (Array.isArray(commits) ? commits : []).filter((x) => x.sha !== ownMerge),
    likelyTier: merged
      ? (Array.isArray(commits) ? commits : []).filter((x) => x.sha !== ownMerge).length
        ? "production-incident | follow-up fix — READ THE COMMITS"
        : "merged clean, nothing touched it since"
      : (Array.isArray(commits) ? commits : []).filter((x) => x.sha !== ownMerge).length
        ? "author-fixed? — commits landed on the branch after the review"
        : "pending — no commit touched this file since the review"
  });
}

if (asJson) {
  console.log(JSON.stringify(out, null, 2));
  process.exit(0);
}

console.log(`\nGROUND-TRUTH CANDIDATES — ${out.length} finding(s) with an outcome to check\n`);
console.log(`Nothing here is a label. Each row is "the review said X about this file;`);
console.log(`here is what happened to that file afterwards". A person decides.\n`);

const groups = {};
for (const o of out) (groups[o.likelyTier] = groups[o.likelyTier] || []).push(o);
for (const [tier, items] of Object.entries(groups).sort((a, b) => b[1].length - a[1].length)) {
  console.log(`\n=== ${tier}  (${items.length}) ===`);
  for (const o of items) {
    console.log(
      `\n  PR #${o.pr} ${o.state}  ${o.severity}  ${o.survived ? "SURVIVED" : "KILLED"}` +
        `${o.refuteFraction != null ? ` rf=${Number(o.refuteFraction).toFixed(2)}` : ""}`
    );
    console.log(`  ${String(o.title).slice(0, 100)}`);
    console.log(`  ${o.file || "(no file recorded — legacy row, not adjudicable as-is)"}`);
    for (const c of (o.evidence || []).slice(0, 4))
      console.log(`      ${c.sha} ${String(c.date).slice(0, 10)}  ${String(c.msg).slice(0, 76)}`);
    if ((o.evidence || []).length > 4) console.log(`      … ${o.evidence.length - 4} more`);
  }
}

console.log(`\n\nHOW TO TURN ONE OF THESE INTO GROUND TRUTH`);
console.log(`  A killed finding + a later commit that fixes exactly it  -> knownReal (the panel`);
console.log(`    had a false negative). This is the label that can price the refuter package's`);
console.log(`    one verdict flip, and nothing else can.`);
console.log(`  A surviving finding + the PR merged unchanged and no incident -> knownBogus,`);
console.log(`    but only if it was dismissed with a reason that held — "nobody fixed it" alone`);
console.log(`    is not evidence.`);
console.log(`  Write it to eval/corpus/pr-<n>.json with the sha in \`evidence\`. An annotation with no`);
console.log(`  verifiable pointer is an opinion; see eval/README.md.\n`);
