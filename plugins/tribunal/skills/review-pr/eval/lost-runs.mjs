// lost-runs.mjs — which runs died before writing their metrics row.
//
// Every run brackets itself in `inflight-runs.jsonl`: `mark-run-started` appends
// a `started` entry in the Context phase, and the persist step appends
// `completed` only after `persist-result.txt` confirms `ok`. A `started` with no
// `completed` is a run whose measurement was lost — usually a session cutoff
// mid-run, not a script crash.
//
// The bug this replaces. The original check kept `starts` in a dict keyed by
// `(pr, headSha)` and `done` in a SET of the same key:
//
//     if e['event'] == 'started': starts[key] = e      # later start overwrites
//     else: done.add(key)                              # any completion covers all
//
// Both halves lose runs, and they lose them in the case that matters. PR #1935
// has ten rows at ONE headSha: with that shape, one completed run marks the key
// done forever, so a later run at the same commit that died reports nothing —
// and the dict has already discarded every start but the last. It reported 0
// lost runs on a day when a run had died mid-flight, which is how this was
// found.
//
// The fix is to COUNT rather than to flag: n starts and m completions for a key
// means n − m lost, and the unmatched ones are the most recent starts, since
// completions arrive in order.

// Pure: the file's lines -> what is missing. Order matters, so entries are
// matched chronologically per key rather than by set membership.
// A start with no completion is either a lost run or a run STILL IN FLIGHT, and
// the trail cannot tell them apart — the marker for "finished" is the only thing
// that distinguishes them, and an in-flight run has not written it yet. Reported
// with an age instead of guessed at: anything younger than the longest run this
// harness has ever taken is flagged `possiblyRunning`, so a check performed
// DURING a review does not cry wolf. Longest recorded run is ~26 min; 60 gives
// generous headroom without swallowing a real loss for long.
import { INFLIGHT_PATH, isMainModule } from "./paths.mjs";
export const IN_FLIGHT_GRACE_MIN = 60;

export function findLostRuns(lines, nowMs = null) {
  const byKey = new Map();
  let malformed = 0;
  for (const line of lines) {
    if (!String(line || "").trim()) continue;
    let e;
    try {
      e = JSON.parse(line);
    } catch {
      malformed++;
      continue;
    }
    if (!e || (e.event !== "started" && e.event !== "completed")) {
      malformed++;
      continue;
    }
    // A null headSha is its own bucket, not a wildcard: a run that died before
    // resolving the SHA must not be paired with one that resolved it.
    const key = `${e.pr}@@${e.headSha === undefined ? null : e.headSha}`;
    if (!byKey.has(key)) byKey.set(key, { starts: [], completions: [] });
    byKey.get(key)[e.event === "started" ? "starts" : "completions"].push(e);
  }

  // Matching is CHRONOLOGICAL and LIFO, not positional. Reviews run one at a
  // time, so a completion belongs to the most recent start still open — the run
  // that just finished. Pairing the first n starts with the n completions
  // instead (the obvious counting version) misattributes every completion once
  // any earlier run has died: on this workspace it declared a run that had
  // finished ten minutes earlier LOST, while treating a start from five hours
  // before — which really had died — as completed. Same total, wrong runs named,
  // and the names are the entire output.
  const lost = [];
  const summary = [];
  let lastCompletedMs = null;
  for (const [key, { starts, completions }] of byKey) {
    for (const c of completions) {
      const t = Date.parse(c.ts || "");
      if (Number.isFinite(t) && (lastCompletedMs === null || t > lastCompletedMs)) lastCompletedMs = t;
    }
    const [pr] = key.split("@@");
    summary.push({ key, pr: Number(pr), started: starts.length, completed: completions.length });
    const events = [
      ...starts.map((e) => ({ e, t: String(e.ts || ""), kind: "start" })),
      ...completions.map((e) => ({ e, t: String(e.ts || ""), kind: "done" }))
    ].sort((a, b) => (a.t < b.t ? -1 : a.t > b.t ? 1 : a.kind === "start" ? -1 : 1));
    const open = [];
    for (const ev of events) {
      if (ev.kind === "start") open.push(ev.e);
      else if (open.length) open.pop();
    }
    for (const e of open) {
      const t = Date.parse(e.ts || "");
      const ageMin = nowMs && Number.isFinite(t) ? (nowMs - t) / 60000 : null;
      lost.push({
        ...e,
        ageMinutes: ageMin === null ? null : Math.round(ageMin),
        possiblyRunning: ageMin !== null && ageMin >= 0 && ageMin < IN_FLIGHT_GRACE_MIN
      });
    }
  }
  return { lost, summary, malformed, lastCompletedMs };
}

// The runs of the last `days` days that started and never wrote a row, not
// counting one that may still be running. A persist step that fails on every
// run looks like this and like nothing else: on Windows from v81 to v94 the
// trail kept growing and metrics.jsonl stopped, for four days, because nothing
// compared the two until a person did.
//
// A loss that some later run completed after is left out. The persist step
// worked after it, so it was a one-off cutoff (an aborted relaunch, a measurement
// run stopped by hand), and reporting it before every review for a week only
// trained people to skip the line. A broken persist step completes nothing after
// it breaks, so its losses still all come back.
export function recentLost(lines, nowMs, days = 7) {
  const cutoff = nowMs - days * 86400000;
  const { lost, lastCompletedMs } = findLostRuns(lines, nowMs);
  return lost.filter((e) => {
    const t = Date.parse(e.ts || "");
    if (e.possiblyRunning || !(t >= cutoff)) return false;
    return lastCompletedMs === null || !(lastCompletedMs > t);
  });
}

if (isMainModule(import.meta.url)) {
  const { readFileSync, existsSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const { homedir } = await import("node:os");
  const path = INFLIGHT_PATH;
  if (!existsSync(path)) {
    console.log(`no ${path} — nothing to check`);
    process.exit(0);
  }
  const ri = process.argv.indexOf("--recent");
  if (ri !== -1) {
    const days = Number(process.argv[ri + 1]) > 0 ? Number(process.argv[ri + 1]) : 7;
    const recent = recentLost(readFileSync(path, "utf8").split("\n"), Date.now(), days);
    if (recent.length)
      console.log(
        `WARNING: ${recent.length} run(s) in the last ${days} day(s) started and never wrote a metrics row (PR ${[...new Set(recent.map((e) => e.pr))].join(", ")}), and no run has completed since. The persist step may be failing. Details: node eval/lost-runs.mjs`,
      );
    process.exit(0);
  }
  const { lost, summary, malformed } = findLostRuns(readFileSync(path, "utf8").split("\n"), Date.now());
  for (const s of summary.sort((a, b) => b.started - a.started))
    console.log(
      `  pr ${String(s.pr).padStart(5)}  started ${String(s.started).padStart(3)}  completed ${String(
        s.completed
      ).padStart(3)}${s.started > s.completed ? `   <- ${s.started - s.completed} LOST` : ""}`
    );
  if (malformed) console.log(`\n  ${malformed} unparseable line(s) — reported, not skipped silently`);
  const inFlight = lost.filter((e) => e.possiblyRunning);
  const reallyLost = lost.filter((e) => !e.possiblyRunning);
  console.log(reallyLost.length ? `\nLOST RUNS (${reallyLost.length}):` : `\nno lost runs`);
  for (const e of reallyLost)
    console.log(`  ${e.ageMinutes === null ? "age unknown" : e.ageMinutes + " min ago"}  ${JSON.stringify(e)}`);
  if (inFlight.length) {
    console.log(`\nSTARTED RECENTLY, NO COMPLETION YET (${inFlight.length}) — probably still running, not lost:`);
    for (const e of inFlight) console.log(`  ${e.ageMinutes} min ago  pr ${e.pr} ${String(e.headSha).slice(0, 8)}`);
  }
  process.exit(0);
}
