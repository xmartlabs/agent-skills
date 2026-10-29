// relay.mjs — the disk side of the workflow's verified relays.
//
// workflows/tribunal.js has no filesystem, so data crosses between it and these
// scripts only as text a mechanic agent copies. Both directions are checked by
// CRC32 of the UTF-8 bytes plus the byte length:
//
//   JS -> disk: the workflow sends a heredoc and its checksum, and `check`
//               refuses the copy on disk if it differs.
//   disk -> JS: a script prints `relayLines(obj)`, the mechanic copies the
//               checksum and the JSON line into its answer, and the workflow
//               recomputes the checksum over JSON.stringify(data).
//
// Measured 2026-10-18 over 12 stored Workflow runs: the heredoc copies were
// exact 20 of 20, while copying a script's stdout into a schema lost fields in
// 10 of 10 non-empty runs, and one script's output shrank to 15-25% of its size
// in 6 of 10.
// The copy-back is the lossy direction, so it gets the same check.
//
// The workflow carries an identical CRC in its VERIFIED-RELAY region;
// eval/replay-verified-relay.mjs pins the two to the same values.
//
//   node relay.mjs check <file> <crc:bytes>
//   node relay.mjs assemble <file> <crc:bytes>
//
// `assemble` is the form for a body too long for one Bash command. The Bash
// tool on Windows fails a command over about 7,300 characters, so the workflow
// sends the body as several `printf` chunks appended to <file>.wire, in
// commands of their own. The wire text has no backslash (a Bash tool was
// reported to turn `\\` into `\` on Windows) and no single quote, so the
// text needs no quoting inside the printf: wireEncode writes each `\` as
// U+29F5 `b`, each `'` as U+29F5 `q` and each U+29F5 as U+29F5 `e`. `assemble` decodes <file>.wire,
// refuses it unless the decoded body matches the checksum, and only then
// writes <file>, with the newline a heredoc would have added.

import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { crc32 } from "node:zlib";

export const RELAY_CRC_PREFIX = "RELAY-CRC ";
export const RELAY_REFUSED_PREFIX = "RELAY-REFUSED ";

export function relayCrc(text) {
  const buf = Buffer.from(text, "utf8");
  return `${crc32(buf).toString(16).padStart(8, "0")}:${buf.length}`;
}

export function relayLines(obj) {
  const json = JSON.stringify(obj);
  return `${RELAY_CRC_PREFIX}${relayCrc(json)}\n${json}\n`;
}

const WIRE_ESC = "\u29f5";
const WIRE_CODES = { "\\": "b", [WIRE_ESC]: "e", "'": "q" };
const WIRE_CHARS = { b: "\\", e: WIRE_ESC, q: "'" };
export function wireEncode(text) {
  return String(text).replace(/[\\\u29f5']/g, (c) => `${WIRE_ESC}${WIRE_CODES[c]}`);
}
export function wireDecode(text) {
  return String(text).replace(/\u29f5([beq])/g, (_, k) => WIRE_CHARS[k]);
}

// A heredoc adds one newline after its body; that one is not part of the copy.
export function checkCopy(fileText, expected) {
  const body = fileText.endsWith("\n") ? fileText.slice(0, -1) : fileText;
  const got = relayCrc(body);
  return got === expected ? { ok: true } : { ok: false, got };
}

function assemble(file, expected) {
  let wire;
  try {
    wire = readFileSync(`${file}.wire`, "utf8");
  } catch (e) {
    process.stdout.write(`${RELAY_REFUSED_PREFIX}${file}: cannot read the copy (${e.message})\n`);
    process.exit(3);
  }
  const body = wireDecode(wire);
  const got = relayCrc(body);
  if (got !== expected) {
    process.stdout.write(`${RELAY_REFUSED_PREFIX}${file}: expected ${expected}, the copy is ${got}\n`);
    process.exit(3);
  }
  writeFileSync(file, `${body}\n`);
}

function main() {
  const [cmd, file, expected] = process.argv.slice(2);
  if ((cmd !== "check" && cmd !== "assemble") || !file || !expected) {
    process.stderr.write("usage: relay.mjs check|assemble <file> <crc:bytes>\n");
    process.exit(2);
  }
  if (cmd === "assemble") return assemble(file, expected);
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch (e) {
    process.stdout.write(`${RELAY_REFUSED_PREFIX}${file}: cannot read the copy (${e.message})\n`);
    process.exit(3);
  }
  const r = checkCopy(text, expected);
  if (!r.ok) {
    process.stdout.write(`${RELAY_REFUSED_PREFIX}${file}: expected ${expected}, the copy is ${r.got}\n`);
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
