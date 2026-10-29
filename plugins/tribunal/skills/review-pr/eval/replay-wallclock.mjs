#!/usr/bin/env node
// replay-wallclock.mjs — the wall-clock resolution, run as the SHIPPING python.
//
// WHY IT EXISTS. `wallClockSeconds` was null on 3 of 9 workflow rows and read
// 46903s (13h) on a fourth, and nobody could say why, because the logic lived in
// one line of python inside a bash heredoc inside a JS template string — a place
// no test could reach. This file reaches it: the `# ===== BEGIN PY-WALLCLOCK =====`
// region is sliced out of workflows/peer-review.js and executed by python3 with
// fixture inputs. There is no second copy to drift, same contract as the JS pure
// regions.
//
// The two things it pins, both of which were wrong before:
//   1. The start epoch comes from the inflight-runs trail, not from an agent's
//      prose return. Diagnosed 2026-07-27: the `started` line was present for all
//      three null rows, so the bash always ran and only the RETURN failed.
//   2. A value above the active-run bound is KEPT and flagged, never nulled. The
//      46903s row reproduced exactly from its own marker's ts — the clock was
//      right and that run really did span 13h. Nulling it would have destroyed
//      honest data, which the first draft of this change did.
//
// Usage: node eval/replay-wallclock.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { lib, RECORDED_INFLIGHT_PATH } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
// The PY-WALLCLOCK region moved out of the peer-review.js prompt and into
// lib/persist-run.sh on 2026-07-28, when 9.5 KB of fixed bash was taken off the
// persist agent's retyping bill. This replay found the move by failing loudly —
// which is the behaviour the "refuse on absence" note below asks for, working.
const WORKFLOW = lib("persist-run.sh");
const SKILL = resolve(HERE, "..");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nwall-clock resolution — PY-WALLCLOCK region of lib/persist-run.sh`);

// --- Slice the region. Refuse loudly on absence: a replay that silently tests
// nothing is worse than one that fails.
const src = readFileSync(WORKFLOW, "utf8");
const BEGIN = "# ===== BEGIN PY-WALLCLOCK =====";
const END = "# ===== END PY-WALLCLOCK =====";
const b = src.indexOf(BEGIN);
const e = src.indexOf(END, b);
if (b === -1 || e === -1) {
  console.log(
    `  FAIL  the PY-WALLCLOCK region is missing from ${WORKFLOW} — renamed or deleted`,
  );
  process.exit(1);
}
const region = src.slice(b + BEGIN.length, e);

// The region must not carry any un-substituted JS interpolation: if it did, the
// shipping code and this replay would be running different text.
check(
  "the region is pure python, with no JS interpolation left inside it",
  !/\$\{/.test(region),
  "found a ${...} placeholder inside the markers — move it above BEGIN",
);

const dir = mkdtempSync(join(tmpdir(), "pr-wallclock-"));
const ISO = (epoch) =>
  new Date(epoch * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");

// Run the region with fixture inputs and return the fields it wrote onto `rec`.
function run({ lines, pr = 1968, headSha = "abc123", hint = null, end, repo = null }) {
  const inflight = join(dir, `inflight-${Math.abs(end)}-${lines.length}-${pr}.jsonl`);
  writeFileSync(inflight, lines.join("\n") + (lines.length ? "\n" : ""));
  const prelude = [
    "import json, datetime, os, time",
    "rec = {}",
    `START_EPOCH_HINT = ${hint === null ? "None" : hint}`,
    `PR = ${pr}`,
    `HEAD_SHA = ${headSha === null ? "None" : `'${headSha}'`}`,
    // The run's repo, defined above BEGIN by persist-run.sh from the record.
    "REPO = os.environ.get('PR_REPO') or None",
    // The path reaches python through the ENVIRONMENT, never interpolated into
    // the source — the convention the shipping code already states at
    // tribunal.js:1184 ("Values reach python through the ENVIRONMENT"). Here it
    // is not tidiness: `mkdtempSync(tmpdir())` returns `C:\Users\...` on
    // Windows, and `\U` inside a non-raw literal is a parse error, so every case
    // in this file died with `truncated \UXXXXXXXX escape` before the region ran
    // once. A replay that cannot parse its own prelude tests nothing, and
    // `release.mjs` refuses to record a freeze while the battery is red — so
    // this one line kept freezes from being cuttable on that platform at all.
    "INFLIGHT = os.environ['PR_INFLIGHT']",
    `END_EPOCH = ${end}`,
    "ACTIVE_RUN_BOUND = 4 * 3600",
  ].join("\n");
  const tail =
    "\nprint(json.dumps({'s': rec.get('wallClockSeconds'), 'note': rec.get('wallClockNote'), 'src': rec.get('wallClockSource')}))";
  const out = execFileSync("python3", ["-c", prelude + "\n" + region + tail], {
    encoding: "utf8",
    env: { ...process.env, PR_INFLIGHT: inflight, PR_REPO: repo || "" },
  });
  return JSON.parse(out.trim().split("\n").pop());
}

const started = (o) => JSON.stringify({ event: "started", ...o });
const T0 = 1769000000;

console.log(`\n--- the epoch comes from the file, precisely when it is there ---`);
{
  const r = run({
    lines: [started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    end: T0 + 600,
  });
  check("an exact headSha match yields the delta", r.s === 600, JSON.stringify(r));
  check("...and names its source", r.src === "inflight-exact", JSON.stringify(r));
  check("...with no note, because nothing is wrong", r.note === null, JSON.stringify(r));
}

console.log(`\n--- a line with no epoch field is still recoverable from its ts ---`);
// This is the arm that makes the fix retroactive: every started line ever written
// carries `ts`, so rows that predate the `epoch` field are computable too.
{
  const r = run({
    lines: [started({ pr: 1968, headSha: "abc123", ts: ISO(T0) })],
    end: T0 + 853,
  });
  check("ts alone yields the delta", r.s === 853, JSON.stringify(r));
  check("...still from the trail, not the hint", r.src === "inflight-exact", JSON.stringify(r));
}

console.log(`\n--- the prose return is a fallback, never the source ---`);
{
  const r = run({ lines: [], hint: T0, end: T0 + 900 });
  check("with no trail, the hint is used", r.s === 900, JSON.stringify(r));
  check("...and says so, so a reader can tell", r.src === "return-value", JSON.stringify(r));

  const withBoth = run({
    lines: [started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    hint: T0 - 99999,
    end: T0 + 600,
  });
  check(
    "the trail outranks a disagreeing hint",
    withBoth.s === 600 && withBoth.src === "inflight-exact",
    JSON.stringify(withBoth),
  );
}

console.log(`\n--- nothing at all is null WITH a note, not a bare null ---`);
{
  const r = run({ lines: [], hint: null, end: T0 });
  check("seconds is null", r.s === null, JSON.stringify(r));
  check("the note explains the null", typeof r.note === "string" && /no start epoch/.test(r.note), JSON.stringify(r));
  check("the source is null too", r.src === null, JSON.stringify(r));
}

console.log(`\n--- an over-bound value is KEPT and flagged, never nulled ---`);
// The regression this arm exists to prevent is my own first draft, which nulled
// it. 46903s was correct data.
{
  const r = run({
    lines: [started({ pr: 1911, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    pr: 1911,
    end: T0 + 46903,
  });
  check("the integer survives", r.s === 46903, JSON.stringify(r));
  check(
    "the note says elapsed, not active",
    typeof r.note === "string" && /elapsed, not active/.test(r.note),
    JSON.stringify(r),
  );
  const under = run({
    lines: [started({ pr: 1911, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    pr: 1911,
    end: T0 + 4 * 3600,
  });
  check("exactly at the bound is not flagged", under.s === 14400 && under.note === null, JSON.stringify(under));
}

console.log(`\n--- refusals: a backwards clock is not a duration ---`);
{
  const r = run({
    lines: [started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    end: T0 - 10,
  });
  // The marker is later than END, so it is skipped as "not this run's start" and
  // there is no other candidate — the honest answer is null, not a negative.
  check("a marker after the end is not used", r.s === null, JSON.stringify(r));
  const viaHint = run({ lines: [], hint: T0, end: T0 - 10 });
  check("a negative delta from the hint is refused", viaHint.s === null, JSON.stringify(viaHint));
  check(
    "...and the refusal is stated",
    typeof viaHint.note === "string" && /refused/.test(viaHint.note),
    JSON.stringify(viaHint),
  );
}

console.log(`\n--- picking the right marker among many ---`);
{
  const r = run({
    lines: [
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0 - 5000), epoch: T0 - 5000 }),
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 }),
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0 + 9999), epoch: T0 + 9999 }),
    ],
    end: T0 + 600,
  });
  check("the last marker at or before the end wins", r.s === 600, JSON.stringify(r));

  const other = run({
    lines: [
      started({ pr: 1968, headSha: "OTHERSHA", ts: ISO(T0 - 5000), epoch: T0 - 5000 }),
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 }),
    ],
    end: T0 + 600,
  });
  check(
    "a different commit's marker cannot lend its epoch",
    other.s === 600 && other.src === "inflight-exact",
    JSON.stringify(other),
  );

  const loose = run({
    lines: [started({ pr: 1968, headSha: "OTHERSHA", ts: ISO(T0), epoch: T0 })],
    end: T0 + 600,
  });
  check(
    "with no exact match it degrades to pr-only, and labels it",
    loose.s === 600 && loose.src === "inflight-pr",
    JSON.stringify(loose),
  );

  const wrongPr = run({
    lines: [started({ pr: 1234, headSha: "abc123", ts: ISO(T0), epoch: T0 })],
    end: T0 + 600,
  });
  check("another PR's marker is never used", wrongPr.s === null, JSON.stringify(wrongPr));
}

console.log(`\n--- degenerate input degrades, never throws ---`);
{
  const r = run({
    lines: [
      "{ not json at all",
      "",
      started({ pr: 1968, headSha: "abc123", ts: "not-a-timestamp" }),
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: "1769000000" }),
      JSON.stringify({ event: "completed", pr: 1968, ts: ISO(T0 + 1) }),
      started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: T0 }),
    ],
    end: T0 + 600,
  });
  check("unparseable and malformed lines are skipped, not fatal", r.s === 600, JSON.stringify(r));

  const stringEpoch = run({
    lines: [started({ pr: 1968, headSha: "abc123", ts: ISO(T0), epoch: "nope" })],
    end: T0 + 600,
  });
  check(
    "a non-integer epoch falls through to the ts",
    stringEpoch.s === 600,
    JSON.stringify(stringEpoch),
  );

  const missing = run({ lines: [], hint: T0, end: T0 + 42 });
  check("an absent trail file is not an error", missing.s === 42, JSON.stringify(missing));
}

console.log(`\n--- the three historical nulls become computable ---`);
// Not a golden of the new code's own opinion: these deltas were derived from the
// real inflight-runs.jsonl against each row's own recorded `ts`, and each one is
// a row that shipped `wallClockSeconds: null`.
{
  const trail = readFileSync(RECORDED_INFLIGHT_PATH, "utf8")
    .split("\n")
    .filter(Boolean);
  const cases = [
    { pr: 1935, rowTs: "2026-07-25T01:59:52Z", expect: 1036 },
    { pr: 1968, rowTs: "2026-07-27T18:05:39Z", expect: 853 },
    { pr: 1968, rowTs: "2026-07-27T20:48:27Z", expect: 794 },
  ];
  for (const c of cases) {
    const end = Math.floor(new Date(c.rowTs).getTime() / 1000);
    const r = run({ lines: trail, pr: c.pr, headSha: null, hint: null, end });
    check(
      `pr${c.pr} @ ${c.rowTs} resolves to ${c.expect}s instead of null`,
      r.s === c.expect,
      JSON.stringify(r),
    );
  }
}

console.log(`\n--- one PR number in two repos: each run takes its own repo's marker ---`);
{
  const end = T0 + 600;
  const lines = [
    started({ pr: 1968, repo: "acme/api", headSha: "abc123", epoch: T0 + 100 }),
    started({ pr: 1968, repo: "acme/web", headSha: "abc123", epoch: T0 + 300 }),
  ];
  const api = run({ lines, end, repo: "acme/api" });
  check("the api run ignores the web repo's later marker", api.s === 500 && api.src === "inflight-exact", JSON.stringify(api));
  const web = run({ lines, end, repo: "acme/web" });
  check("the web run takes its own marker", web.s === 300, JSON.stringify(web));
  const legacy = run({ lines: [started({ pr: 1968, headSha: "abc123", epoch: T0 + 200 })], end, repo: "acme/api" });
  check("a marker written before markers carried a repo still matches", legacy.s === 400, JSON.stringify(legacy));
  const unkeyed = run({ lines, end });
  check("a caller that passes no repo keeps the old behaviour (latest marker)", unkeyed.s === 300, JSON.stringify(unkeyed));
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall wall-clock expectations hold");
