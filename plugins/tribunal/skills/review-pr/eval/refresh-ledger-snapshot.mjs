#!/usr/bin/env node
// refresh-ledger-snapshot.mjs — replace the frozen ledger the replays read
// (review-harness-data/tribunal/ledger/) with a newer copy of this machine's
// own ledger.
//
// The replays that re-decide recorded runs read the snapshot, never DATA_DIR,
// so the battery gives the same answer on every machine. The price is that a
// run recorded after the snapshot does not exist for them, and a replay that
// names one fails with "row not found". This script is how such a run comes in.
//
// It accepts an extension and nothing else. Every line already in the snapshot
// must still be in the new copy, at the same position, because the replays pin
// exact answers over those rows (replay-gates' six uncited PRE_DECIDED votes,
// the five fidelity-critic false positives in replay-fidelity-payload). A copy
// that lost or reordered a row would move those answers for a reason that has
// nothing to do with the code under test. One exception: the snapshot's last
// metrics row may have been patched since, because persist-review-record.mjs
// rewrites the newest row in place after a post. That line is accepted when its
// pr and headSha still match.
//
// Paths that only mean something on the operator's machine (the home directory,
// the per-user temp directory a run's artifactDir sits in, a Claude Code
// session dir named after the home) are rewritten to `~/` and `/tmp` on the way in, before the comparison, so the snapshot in the
// repo names nobody's machine. eval/replay-harness-data-paths.mjs fails if one
// gets in anyway.
//
// It never commits. Without --write it prints what it would copy and stops.
// After --write, run the battery and commit the snapshot on its own, as
// review-harness-data/README.md asks.
//
// Usage: node eval/refresh-ledger-snapshot.mjs [--from <dir>] [--write]

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DATA_DIR, isMainModule, TRIBUNAL_DATA } from "./paths.mjs";

export const SNAPSHOT_DIR = resolve(TRIBUNAL_DATA, "ledger");

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const MAC_USER_TMP = /\/var\/folders\/[A-Za-z0-9_]{2}\/[A-Za-z0-9_]+\/T/g;

export function scrubMachinePaths(text, { home = homedir(), tmp = tmpdir() } = {}) {
  let out = text;
  const tmps = new Set([tmp.replace(/\/+$/, "")]);
  try {
    tmps.add(realpathSync(tmp).replace(/\/+$/, ""));
  } catch {}
  for (const t of [...tmps].sort((a, b) => b.length - a.length))
    if (t && t !== "/tmp") out = out.replace(new RegExp(escapeRe(t), "g"), "/tmp");
  out = out.replace(MAC_USER_TMP, "/tmp");
  if (home && home !== "/") {
    out = out.replace(new RegExp(escapeRe(home.replace(/\/+$/, "")) + "/", "g"), "~/");
    // Claude Code names its per-session dirs after the cwd with every
    // non-alphanumeric turned into "-", so the home survives as "-Users-<name>".
    const slug = encodedHome(home);
    out = out.replace(new RegExp(`(?:/private)?/tmp/claude-\\d+/${escapeRe(slug)}[^/\\s"'\\\\]*/`, "g"), "/tmp/");
    out = out.replace(new RegExp(escapeRe(slug), "g"), "-~");
  }
  return out;
}
export const encodedHome = (home = homedir()) => home.replace(/\/+$/, "").replace(/[^A-Za-z0-9]/g, "-");
const linesOf = (text) => text.split("\n").filter((l) => l.trim() !== "");

function parseAll(lines) {
  const rows = [];
  for (let i = 0; i < lines.length; i++) {
    try {
      rows.push(JSON.parse(lines[i]));
    } catch {
      return { error: `line ${i + 1} is not JSON` };
    }
  }
  return { rows };
}

const runOf = (r) => ({ pr: r?.pr ?? null, headSha: r?.headSha ?? null, mode: r?.mode ?? null, ts: r?.ts ?? null, harnessVersion: r?.harnessVersion ?? null });

export function planRefresh({ fromDir = DATA_DIR, toDir = SNAPSHOT_DIR, scrub = scrubMachinePaths } = {}) {
  if (!existsSync(toDir)) return { ok: false, errors: [`no snapshot at ${toDir}`], files: [] };
  const names = readdirSync(toDir).filter((n) => n.endsWith(".jsonl")).sort();
  if (!names.length) return { ok: false, errors: [`${toDir} holds no .jsonl files`], files: [] };
  const errors = [];
  const files = [];
  for (const name of names) {
    const src = join(fromDir, name);
    const dst = join(toDir, name);
    if (!existsSync(src)) {
      errors.push(`${name}: missing from ${fromDir}`);
      continue;
    }
    const srcBuf = Buffer.from(scrub(readFileSync(src, "utf8")), "utf8");
    const dstBuf = readFileSync(dst);
    const srcLines = linesOf(srcBuf.toString("utf8"));
    const dstLines = linesOf(dstBuf.toString("utf8"));
    const parsed = parseAll(srcLines);
    if (parsed.error) {
      errors.push(`${name}: ${parsed.error} in ${src}; a corrupt ledger is never copied`);
      continue;
    }
    if (srcLines.length < dstLines.length) {
      errors.push(`${name}: ${src} has ${srcLines.length} lines, fewer than the snapshot's ${dstLines.length}`);
      continue;
    }
    let patchedLast = false;
    let divergedAt = -1;
    for (let i = 0; i < dstLines.length; i++) {
      if (srcLines[i] === dstLines[i]) continue;
      const isLast = i === dstLines.length - 1;
      if (isLast && name === "metrics.jsonl") {
        let before = null;
        try {
          before = JSON.parse(dstLines[i]);
        } catch {}
        const after = parsed.rows[i];
        if (before && before.pr === after?.pr && before.headSha === after?.headSha) {
          patchedLast = true;
          continue;
        }
      }
      divergedAt = i;
      break;
    }
    if (divergedAt >= 0) {
      errors.push(`${name}: line ${divergedAt + 1} differs from the snapshot; only an extension of it is accepted`);
      continue;
    }
    const added = parsed.rows.slice(dstLines.length);
    files.push({
      name,
      src,
      dst,
      bytes: srcBuf,
      snapshotLines: dstLines.length,
      sourceLines: srcLines.length,
      added: added.length,
      addedRuns: name === "metrics.jsonl" ? added.map(runOf) : [],
      patchedLast,
      unchanged: srcBuf.equals(dstBuf),
      shaBefore: sha256(dstBuf),
      shaAfter: sha256(srcBuf),
    });
  }
  return { ok: errors.length === 0, errors, files, fromDir, toDir };
}

export function applyRefresh(plan) {
  if (!plan.ok) throw new Error("refusing to apply a plan that has errors");
  for (const f of plan.files) if (!f.unchanged) writeFileSync(f.dst, f.bytes);
  return plan.files.filter((f) => !f.unchanged).map((f) => f.name);
}

if (isMainModule(import.meta.url)) {
  const args = process.argv.slice(2);
  const fromIdx = args.indexOf("--from");
  const fromDir = fromIdx >= 0 ? resolve(args[fromIdx + 1] || "") : DATA_DIR;
  const write = args.includes("--write");
  const plan = planRefresh({ fromDir });
  console.log(`from ${plan.fromDir || fromDir}`);
  console.log(`to   ${plan.toDir || SNAPSHOT_DIR}`);
  for (const f of plan.files) {
    const note = f.unchanged ? "unchanged" : `+${f.added} line(s)${f.patchedLast ? ", last row patched in place" : ""}`;
    console.log(`  ${f.name}: ${f.snapshotLines} -> ${f.sourceLines} (${note})`);
    console.log(`    sha256 ${f.shaBefore.slice(0, 16)} -> ${f.shaAfter.slice(0, 16)}`);
    for (const r of f.addedRuns) console.log(`    new run: #${r.pr} ${String(r.headSha).slice(0, 8)} ${r.mode} ${r.ts} ${r.harnessVersion}`);
  }
  if (!plan.ok) {
    for (const e of plan.errors) console.error(`REFUSED ${e}`);
    process.exit(1);
  }
  if (plan.files.every((f) => f.unchanged)) {
    console.log("snapshot already matches the source; nothing to copy");
    process.exit(0);
  }
  if (!write) {
    console.log("dry run: pass --write to copy, then run the battery and commit the snapshot on its own");
    process.exit(0);
  }
  const copied = applyRefresh(plan);
  console.log(`copied ${copied.join(", ")}. Run the battery, then commit review-harness-data/tribunal/ledger/ in its own commit.`);
}
