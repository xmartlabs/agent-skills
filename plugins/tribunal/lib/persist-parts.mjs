#!/usr/bin/env node
// persist-parts.mjs — verify the persist payload's parts and join them on disk.
//
// The row reached disk as one heredoc per file, retyped whole by one mechanic.
// From 2026-10-18's A/B, 2 of 14 runs lost their row that way: at 16-18 KB of
// metrics.json, haiku and then sonnet each deleted a field, the payload checksum
// refused both copies, and nothing was written. The workflow now sends each body
// in parts of at most PERSIST_PART_BYTES, each copied by its own mechanic, and
// this script checks every part against its own checksum before anything is
// joined. A bad part names itself, so the retry rewrites that part only.
//
//   node persist-parts.mjs <vote-dir> <has-votes 0|1> [--wire] <file>=<crc:bytes>,<crc:bytes>... ...
//
// Parts are read from <vote-dir>/persist/<file>.<i>. With --wire they are in wire
// form (no backslash and no single quote, see relay.mjs, used where the Bash tool
// limits a command's length): each part is checked as written, then decoded.
// Parts are read from <vote-dir>/persist/<file>.<i>. On success each <file> is
// written to <vote-dir>/<file> and nothing is printed. On a bad part it prints
// one line in persist-run.sh's contract shape, `persisted` starting "REFUSED:
// payload checksum mismatch", and exits 3 with nothing joined.

import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkCopy, wireDecode } from "./relay.mjs";

export function parseSpec(args) {
  return args.map((arg) => {
    const at = arg.indexOf("=");
    return { file: arg.slice(0, at), parts: arg.slice(at + 1).split(",") };
  });
}

export function joinParts(voteDir, specs, wire = false) {
  const bad = [];
  const joined = [];
  for (const { file, parts } of specs) {
    let body = "";
    parts.forEach((crc, i) => {
      const name = `${file}.${i}`;
      let text;
      try {
        text = readFileSync(join(voteDir, "persist", name), "utf8");
      } catch {
        bad.push(name);
        return;
      }
      if (!checkCopy(text, crc).ok) {
        bad.push(name);
        return;
      }
      const piece = text.endsWith("\n") ? text.slice(0, -1) : text;
      body += wire ? wireDecode(piece) : piece;
    });
    joined.push([file, body]);
  }
  if (bad.length) return { ok: false, bad };
  for (const [file, body] of joined) writeFileSync(join(voteDir, file), body);
  return { ok: true };
}

export function refusalLine(bad, hasVotes) {
  const detail = `payload checksum mismatch in parts [${bad.join(", ")}], the relayed copy differs from what the workflow sent`;
  return JSON.stringify({
    reconciliation: hasVotes ? { checked: null, ok: null, counts: {}, mismatches: [], error: detail } : null,
    selfAudit: { passed: null, checked: 0, violations: [{ id: "payload-checksum", detail }] },
    persisted: `REFUSED: ${detail}. Nothing written.`,
    wallClockSeconds: null,
    wallClockNote: null,
    wallClockSource: null,
  });
}

function main() {
  const [voteDir, hasVotes, ...rest] = process.argv.slice(2);
  const wire = rest.includes("--wire");
  const specArgs = rest.filter((a) => a !== "--wire");
  if (!voteDir || !specArgs.length || specArgs.some((a) => !a.includes("="))) {
    process.stderr.write("usage: persist-parts.mjs <vote-dir> <has-votes 0|1> <file>=<crc:bytes>,... ...\n");
    process.exit(2);
  }
  const r = joinParts(voteDir, parseSpec(specArgs), wire);
  if (!r.ok) {
    process.stdout.write(`${refusalLine(r.bad, hasVotes === "1")}\n`);
    process.exit(3);
  }
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();
if (isMain) main();
