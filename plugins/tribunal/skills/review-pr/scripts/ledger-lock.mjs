// ledger-lock.mjs — one writer at a time on metrics.jsonl.
//
// Appends and whole-file rewrites both touch the ledger, and tribunal runs may
// finish at the same moment. A rewrite that read the file before another run's
// append writes it back without that row. The lock is a directory next to the
// ledger (mkdir is atomic on every platform), and lib/persist-run.sh takes the
// same one around its append.

import { mkdirSync, rmdirSync, statSync } from "node:fs";

// TRIBUNAL_LEDGER_LOCK_WAIT_MS shortens the wait for the tests that hold the
// lock on purpose; anything but a positive integer leaves the 15 s.
export const DEFAULT_WAIT_MS = 15000;
const waitOverride = process.env.TRIBUNAL_LEDGER_LOCK_WAIT_MS ?? "";
export const WAIT_MS = /^[1-9][0-9]*$/.test(waitOverride) ? Number(waitOverride) : DEFAULT_WAIT_MS;
const STALE_MS = 60000;

export function withLedgerLock(path, fn) {
  const lock = `${path}.lock`;
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    try {
      mkdirSync(lock);
      break;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
      try {
        if (Date.now() - statSync(lock).mtimeMs > STALE_MS) {
          rmdirSync(lock);
          continue;
        }
      } catch {
        continue;
      }
      if (Date.now() > deadline) throw new Error(`could not lock ${path} within ${WAIT_MS} ms`);
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50);
    }
  }
  try {
    return fn();
  } finally {
    try {
      rmdirSync(lock);
    } catch {}
  }
}
