// relay-file.mjs — hand a script's relay to the workflow as a file, not as a copy.
//
// A verified relay (relay.mjs) still has a model copy the JSON line into its
// answer, and haiku fails at that on long or text-heavy lines: fetch-pr was
// retried on sonnet in 5 of 58 runs (2026-10-20/21), twice for invalid JSON
// built from human review text. The Workflow runtime cannot read a file, but
// workflow({scriptPath}) runs a script from disk and returns its value
// unchanged: 26 KB and 421 KB of quotes, backslashes, newlines, emoji and
// U+2028 came back identical (2026-10-21, wf_4ae9220d-4b8). So the relay goes
// into a script whose only statement returns it, the mechanic answers with the
// path, and the workflow loads it and checks the checksum as before.
//
//   <command printing a relay> | node relay-file.mjs
//
// Reads the command's whole output and prints it unchanged, preceded by
// `RELAY-FILE <path>` when the output ends in a relay whose checksum matches
// its JSON line. With no such relay it adds nothing and exits 1, so a failed
// command still reads as failed. A relay over RELAY_FILE_MAX_BYTES gets no file
// (the runtime refuses a script over 512 KB) and is copied as before.
//
// The file goes in the .git directory of the repo the review runs in. The
// runtime loads only a file the session can read, which the system temp
// directory is not (2026-10-21, wf_192b806b-a67), and .git is inside the working
// directory while git, linters and type checkers all skip it. A pull request
// cannot place a file there either: git refuses a path with a .git component.
// When .git is not a directory (a linked worktree, or no repo) nothing is
// written and the relay is copied as before.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { RELAY_CRC_PREFIX, relayCrc } from "../rust/crates/tribunal-context/src/js/relay.mjs";

export const RELAY_FILE_PREFIX = "RELAY-FILE ";
export const RELAY_FILE_MAX_BYTES = 480 * 1024;
export const RELAY_FILE_KEEP_MS = 24 * 60 * 60 * 1000;

// The checksum line and the JSON line after it, when the checksum is the JSON
// line's own; null otherwise.
export function findRelay(output) {
  const lines = String(output).split("\n");
  for (let i = lines.length - 2; i >= 0; i--) {
    if (!lines[i].startsWith(RELAY_CRC_PREFIX)) continue;
    const crc = lines[i].slice(RELAY_CRC_PREFIX.length).trim();
    const json = lines[i + 1];
    if (relayCrc(json) !== crc) return null;
    try {
      JSON.parse(json);
    } catch {
      return null;
    }
    return { crc, json };
  }
  return null;
}

// The data travels as a JSON string literal handed to JSON.parse, so no key or
// value in it is read as code.
export function relayFileScript(crc, json) {
  return `export const meta = { name: "tribunal-relay-data", description: "A script's relay, loaded by the workflow instead of copied by a model." };\nreturn { relayCrc: ${JSON.stringify(crc)}, data: JSON.parse(${JSON.stringify(json)}) };\n`;
}

// The .git directory of the repo at cwd, or null when there is none to write in.
export function relayDirFor(cwd = process.cwd()) {
  try {
    const top = execFileSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    const dir = join(top, ".git");
    return statSync(dir).isDirectory() ? dir : null;
  } catch {
    return null;
  }
}

// A day-old relay file is from a run that has long finished.
export function pruneRelayFiles(dir, now = Date.now(), keepMs = RELAY_FILE_KEEP_MS) {
  for (const name of readdirSync(dir)) {
    if (!/^tribunal-relay-[A-Za-z0-9]{6}$/.test(name)) continue;
    try {
      if (now - statSync(join(dir, name)).mtimeMs > keepMs) rmSync(join(dir, name), { recursive: true, force: true });
    } catch {}
  }
}

export function writeRelayFile(output, dir) {
  const relay = findRelay(output);
  if (!relay) return { ok: false };
  const script = relayFileScript(relay.crc, relay.json);
  if (!dir || Buffer.byteLength(script, "utf8") > RELAY_FILE_MAX_BYTES) return { ok: true, path: null };
  try {
    pruneRelayFiles(dir);
    const path = join(mkdtempSync(join(dir, "tribunal-relay-")), "relay.js");
    writeFileSync(path, script);
    return { ok: true, path };
  } catch {
    return { ok: true, path: null };
  }
}

function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isMainModule()) {
  const output = readFileSync(0, "utf8");
  const res = writeRelayFile(output, relayDirFor());
  process.stdout.write(`${res.path ? `${RELAY_FILE_PREFIX}${res.path}\n` : ""}${output}`);
  process.exit(res.ok ? 0 : 1);
}
