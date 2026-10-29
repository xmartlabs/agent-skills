// data-ids.mjs — commit ids that exist only in the private test data, looked
// up by the sha256 of the id, so this public tree never spells out a commit of
// the repository the fixtures were recorded on.
//
//   dataId("a1b2c3d4", 8)  -> the 8-character hex id in TRIBUNAL_DATA whose
//                            sha256 starts with a1b2c3d4
//
// It reads file names and contents under fixtures/ and ledger/, and throws when
// no id or more than one matches, so a replay never runs against a guess.

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { TRIBUNAL_DATA } from "./paths.mjs";

let pool = null;

function collect() {
  const ids = new Set();
  const take = (text) => {
    for (const m of text.matchAll(/(?<![0-9a-f])[0-9a-f]{7,40}(?![0-9a-f])/g)) ids.add(m[0]);
  };
  const walk = (dir) => {
    if (!existsSync(dir)) return;
    for (const name of readdirSync(dir)) {
      const abs = join(dir, name);
      take(name);
      if (statSync(abs).isDirectory()) walk(abs);
      else take(readFileSync(abs, "utf8"));
    }
  };
  walk(join(TRIBUNAL_DATA, "fixtures"));
  walk(join(TRIBUNAL_DATA, "ledger"));
  return ids;
}

export function dataId(hash, length) {
  pool ??= collect();
  const hits = [...pool].filter((id) => id.length === length && createHash("sha256").update(id).digest("hex").startsWith(hash));
  if (hits.length !== 1) throw new Error(`data-ids: ${hits.length} ids of length ${length} match ${hash} under ${TRIBUNAL_DATA}`);
  return hits[0];
}
