#!/bin/bash
# persist-run.sh — reconcile, self-audit and append one peer-review metrics row.
#
# WHY THIS FILE EXISTS. Every line below used to live inside the
# `reconcile-audit-persist` prompt in workflows/peer-review.js, which meant a
# haiku had to RETYPE all of it into a Bash tool call on every single run.
# Measured 2026-07-28 on PR #1935: the agent emitted 13,372 characters in one
# call, of which the metrics JSON — the only part that changes between runs —
# was 3,794. The other 9,578 were this fixed script, re-generated token by token
# for 67 seconds of a 516-second review.
#
# It also shrinks a real failure surface rather than only a cost. The truncation
# guard below exists because a model relaying this block CAN cut it short: the
# 2026-07-23 run landed with 17 of 30 findings and said nothing. With the logic
# on disk, the only thing still travelling through a model is the JSON payload,
# so there is less that can be truncated — and the guard still covers what is.
#
# CONTRACT. The caller writes $VD/metrics.json (and $VD/recorded-votes.json when
# there are external votes) and then invokes this script. It prints ONE line of
# JSON on stdout with keys: reconciliation, selfAudit, persisted,
# wallClockSeconds, wallClockNote, wallClockSource.
#
#   persist-run.sh <vote-dir> <has-votes 0|1> <expected-findings> <pr-number> \
#                  <head-sha|""> <batch-panels true|false> <start-epoch-hint|"">
#
# This file is part of the hashed harness surface (scripts/harness-version.mjs).
# Editing it changes the version id, which is the point: before the move it was
# prompt text inside a hashed file, and it must not become an unhashed script.
#
# Values reach python through the ENVIRONMENT, and every python body is a QUOTED
# heredoc, so nothing inside is expanded by the shell or needs escaping. That is
# the same rule the incremental probe follows, and for the same reason: the first
# version of that probe interpolated a JSON-quoted sha into `python3 -c "…"`,
# which closed the bash string and cut the script in half.

set -u

# Python's default text encoding is locale-dependent, and on Windows that locale
# is cp1252, not UTF-8. Every heredoc below reads or writes files that routinely
# carry non-ASCII — a review report quoting code, a finding title with a dash —
# and one such byte aborts the whole step:
#
#   UnicodeDecodeError: 'charmap' codec can't decode byte 0x9d in position 5824
#
# Measured 2026-08-10 on PR #1516: the run completed, returned a well-formed
# report, and appended no metrics row at all. UTF-8 Mode makes open() and the
# standard streams UTF-8 regardless of locale, which is what every path here
# already assumes on macOS and Linux.
export PYTHONUTF8=1

VD="$1"
HAS_VOTES="$2"
EXPECTED_FINDINGS="$3"
PR_NUMBER="$4"
HEAD_SHA="$5"
BATCH_PANELS="$6"
START_EPOCH_HINT="$7"
# Optional: the caller's repo (owner/name). Keys the started/completed markers
# and the wall-clock match, so BE#N and FE#N cannot lend each other an epoch.
PR_REPO="${PR_REPO:-}"
# The one data-dir rule every reader already follows (eval/paths.mjs).
DATA_DIR="${TRIBUNAL_DATA_DIR:-$HOME/.claude/tribunal-data}"

mkdir -p "$DATA_DIR"

# The caller's payload files were retyped by a model, so check them against the
# CRC32 and byte length the workflow computed before they left it
# (PAYLOAD-CHECKSUM in workflows/tribunal.js). A copy can drop a comma and stop
# parsing, or change a path and still parse; both stop here, with nothing
# written and no completed marker, so the row is missing rather than wrong. An
# unset PAYLOAD_CHECKSUMS is an older caller and skips the check.
# ===== BEGIN PY-CHECKSUM =====
CHECKSUM_LINE=$(VD="$VD" HAS_VOTES="$HAS_VOTES" PAYLOAD_CHECKSUMS="${PAYLOAD_CHECKSUMS:-}" python3 <<'PYCHECKSUM'
import json, os, zlib
vd = os.environ['VD']
spec = os.environ.get('PAYLOAD_CHECKSUMS', '').strip()
bad = []
for item in spec.split():
    name, _, want = item.partition('=')
    want_crc, _, want_len = want.partition(':')
    try:
        data = open(os.path.join(vd, name), 'rb').read()
    except OSError:
        bad.append('%s: missing' % name)
        continue
    if data.endswith(b'\n'):
        data = data[:-1]
    got_crc = '%08x' % (zlib.crc32(data) & 0xffffffff)
    if got_crc != want_crc or str(len(data)) != want_len:
        bad.append('%s: crc32 %s, %d bytes; expected %s, %s bytes' % (name, got_crc, len(data), want_crc, want_len))
if bad:
    detail = 'payload checksum mismatch, the relayed copy differs from what the workflow sent: ' + '; '.join(bad)
    recon = None
    if os.environ.get('HAS_VOTES') == '1':
        recon = {'checked': None, 'ok': None, 'counts': {}, 'mismatches': [], 'error': detail}
    print(json.dumps({'reconciliation': recon, 'selfAudit': {'passed': None, 'checked': 0, 'violations': [{'id': 'payload-checksum', 'detail': detail}]}, 'persisted': 'REFUSED: ' + detail + '. Nothing written.', 'wallClockSeconds': None, 'wallClockNote': None, 'wallClockSource': None}))
PYCHECKSUM
)
# ===== END PY-CHECKSUM =====
if [ -n "$CHECKSUM_LINE" ]; then
  printf '%s\n' "$CHECKSUM_LINE"
  exit 0
fi

if [ "$HAS_VOTES" = "1" ]; then
  node "$(tribunal-root)"/lib/reconcile-votes.mjs "$VD" --compact \
    < "$VD/recorded-votes.json" > "$VD/recon.json" 2>"$VD/recon.err" || true
else
  printf '{"checked": -1}' > "$VD/recon.json"
fi

VD="$VD" HAS_VOTES="$HAS_VOTES" python3 <<'PYRECON'
import json, os
vd = os.environ['VD']
has_votes = os.environ['HAS_VOTES'] == '1'
try:
    recon = json.load(open(vd + '/recon.json'))
except Exception:
    recon = {}
metrics = json.load(open(vd + '/metrics.json'))
if not has_votes:
    metrics['voteReconciliation'] = None
    metrics['codexModelUsed'] = None
elif recon.get('checked', -1) >= 0:
    metrics['voteReconciliation'] = {
        'checked': recon.get('checked'),
        'ok': recon.get('ok'),
        # The classifier-spawn fix. How many votes could not be audited because the verdict
        # classifier itself failed to run. Carried into the row deliberately:
        # the checked >= 0 test above accepts any well-formed result, so before
        # this a reconciliation produced by a dead classifier was recorded as a
        # real one — 0 clean slots reading as 0 clean slots, with nothing to say
        # the auditor never ran. This field is what lets a past row tell
        # "unaudited" from "audited and dirty" once the temp dir is gone.
        #
        # No backticks in this heredoc — see the PYRECON guard in
        # eval/replay-embedded-bash.mjs; one makes the workflow unparseable.
        'classifierFailed': recon.get('classifierFailed') or 0,
        'counts': recon.get('counts') or {},
        'mismatches': recon.get('mismatches') or [],
    }
    # Which Codex model actually voted, read back from the .meta sidecars via
    # reconcile-votes.mjs's codexModelsUsed. A single string is the normal case;
    # a list means ~/.codex/config.toml changed mid-run — a real model-boundary
    # crossing metrics.jsonl previously had no way to record (RELEASES.md,
    # 2026-07-31). Empty (no codex slot ran this review) -> None.
    codex_models = recon.get('codexModelsUsed') or []
    metrics['codexModelUsed'] = codex_models[0] if len(codex_models) == 1 else (codex_models or None)
else:
    metrics['voteReconciliation'] = {
        'checked': None, 'ok': None, 'counts': {}, 'mismatches': [],
        'error': recon.get('scriptError') or 'reconciliation did not return a usable result',
    }
    metrics['codexModelUsed'] = None
json.dump(metrics, open(vd + '/metrics.json', 'w'))
PYRECON

# The report the fidelity critic judged, materialised beside the row that grades
# it. Until 2026-09-10 the synthesized report lived only in one agent turn's
# context and an mktemp file the posting step discarded, so a
# `fidelity-critic-flagged` violation named a drift nobody could go back and
# read. The caller writes it as one line of JSON (`{"report": "..."}`) for the
# same reason metrics.json is JSON: raw markdown is full of backticks and could
# in principle carry the heredoc delimiter, and JSON escaping makes both
# impossible. Missing is not fatal — an older caller, or a run that produced no
# report, still persists its row.
if [ -f "$VD/report.json" ]; then
  VD="$VD" python3 <<'PYREPORT'
import json, os
vd = os.environ['VD']
try:
    body = json.load(open(vd + '/report.json')).get('report')
    if isinstance(body, str) and body:
        open(vd + '/report.md', 'w').write(body)
except Exception:
    pass
PYREPORT
fi

node "$(tribunal-root)"/lib/audit-run.mjs "$VD" < "$VD/metrics.json" > "$VD/audit.json" 2>"$VD/audit.err" || true

VD="$VD" python3 <<'PYAUDIT'
import json, os
vd = os.environ['VD']
metrics = json.load(open(vd + '/metrics.json'))
try:
    audit = json.load(open(vd + '/audit.json'))
    passed = audit.get('passed')
except Exception:
    passed = None
    audit = {}
if isinstance(passed, bool):
    metrics['selfAudit'] = {'passed': passed, 'checked': audit.get('checked') or 0, 'violations': audit.get('violations') or [], 'notes': audit.get('notes') or []}
else:
    metrics['selfAudit'] = {'passed': None, 'checked': 0, 'violations': [{'id': 'audit-unreadable', 'detail': 'the self-audit returned nothing usable'}], 'notes': []}
json.dump(metrics, open(vd + '/metrics.json', 'w'))
PYAUDIT

# WHICH harness produced this row. Best-effort by design: if the hash cannot be
# produced the row still gets written, carrying null — an unidentifiable row is
# worth more than no row, and null already means "not captured" everywhere else
# in this schema.
HARNESS_VERSION=$(node "$(tribunal-root)"/skills/review-pr/scripts/harness-version.mjs --short 2>/dev/null || true)

VD="$VD" HARNESS_VERSION="$HARNESS_VERSION" EXPECTED_FINDINGS="$EXPECTED_FINDINGS" \
  PR_NUMBER="$PR_NUMBER" HEAD_SHA="$HEAD_SHA" START_EPOCH_HINT="$START_EPOCH_HINT" \
  TRIBUNAL_DATA_DIR="$DATA_DIR" \
  python3 <<'PYPERSIST' > "$VD/persist-result.txt" 2>&1
import sys, json, datetime, os, time
vd = os.environ['VD']
DATA_DIR = os.environ['TRIBUNAL_DATA_DIR']
path = os.path.join(DATA_DIR, 'metrics.jsonl')
rec = json.load(open(vd + '/metrics.json'))
rec['harnessVersion'] = os.environ.get('HARNESS_VERSION', '').strip() or None
# Refuse to write an abridged record: a partial row is worse than no row,
# because it reads as a complete measurement.
expected = int(os.environ['EXPECTED_FINDINGS'])
got = len(rec.get('findings') or [])
if got != expected:
    sys.exit('REFUSED: record carries %d findings, expected %d — the JSON was truncated in transit, nothing written' % (got, expected))
rec['ts'] = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
# Wall-clock proxy. The closing timestamp can only be taken here, at the moment
# the row is about to be written. The START epoch is read from the inflight-runs
# trail the Context phase appended, NOT from that dispatch's prose return: the
# return is the part that failed three times on 2026-07-27 while the appended
# line was present every time. The hint is kept for the case where the file
# cannot answer at all.
_hint = os.environ.get('START_EPOCH_HINT', '').strip()
START_EPOCH_HINT = int(_hint) if _hint.isdigit() else None
PR = int(os.environ['PR_NUMBER'])
_sha = os.environ.get('HEAD_SHA', '').strip()
HEAD_SHA = _sha or None
INFLIGHT = os.path.join(DATA_DIR, 'inflight-runs.jsonl')
REPO = rec.get('repo') or None
END_EPOCH = int(time.time())
ACTIVE_RUN_BOUND = 4 * 3600
# ===== BEGIN PY-WALLCLOCK =====
def _epoch_of(entry):
    # Prefer the recorded epoch; fall back to the ISO ts every started line has
    # carried since before the epoch field existed. That fallback is what makes
    # already-written lines recoverable instead of only future ones.
    val = entry.get('epoch')
    if isinstance(val, int) and not isinstance(val, bool):
        return val
    ts = entry.get('ts')
    if not isinstance(ts, str):
        return None
    try:
        parsed = datetime.datetime.strptime(ts, '%Y-%m-%dT%H:%M:%SZ')
    except Exception:
        return None
    return int(parsed.replace(tzinfo=datetime.timezone.utc).timestamp())

def _resolve_start(path, pr, head_sha, hint, end_epoch, repo=None):
    # The last 'started' line belonging to THIS run. An exact headSha match wins
    # so a re-review at another commit cannot lend its epoch to this row, and
    # markers later than end_epoch are ignored because a subsequent run's start
    # is not this run's start.
    exact = []
    loose = []
    try:
        with open(path) as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except Exception:
                    continue
                if entry.get('event') != 'started' or entry.get('pr') != pr:
                    continue
                # A marker from another repo with the same PR number is not
                # this run. Markers written before 2026-10-19 carry no repo and
                # still match, as they always did.
                if repo and entry.get('repo') and entry.get('repo') != repo:
                    continue
                found = _epoch_of(entry)
                if found is None or found > end_epoch:
                    continue
                if head_sha is not None and entry.get('headSha') == head_sha:
                    exact.append(found)
                else:
                    loose.append(found)
    except Exception:
        pass
    if exact:
        return exact[-1], 'inflight-exact'
    if loose:
        return loose[-1], 'inflight-pr'
    if isinstance(hint, int):
        return hint, 'return-value'
    return None, None

start_epoch, epoch_source = _resolve_start(INFLIGHT, PR, HEAD_SHA, START_EPOCH_HINT, END_EPOCH, REPO)
note = None
if start_epoch is None:
    seconds = None
    note = 'no start epoch: neither the inflight trail nor the dispatch return carried one'
else:
    seconds = END_EPOCH - start_epoch
    if seconds < 0:
        # A clock that ran backwards is not a duration. Refuse rather than record.
        note = 'refused: start epoch %d is after end epoch %d' % (start_epoch, END_EPOCH)
        seconds = None
    elif seconds > ACTIVE_RUN_BOUND:
        # Kept, never nulled. The 46903s row of 2026-07-25 reproduced exactly from
        # its own marker's ts: the clock was right and that run really did span
        # 13h. This field measures ELAPSED wall time, never active compute, so a
        # paused run is honest data an aggregate should exclude, not delete.
        note = 'elapsed, not active: %ds exceeds the %ds active-run bound' % (seconds, ACTIVE_RUN_BOUND)
rec['wallClockSeconds'] = seconds
rec['wallClockNote'] = note
rec['wallClockSource'] = epoch_source
# ===== END PY-WALLCLOCK =====
with open(vd + '/wallclock.json', 'w') as fh2:
    json.dump({'wallClockSeconds': rec['wallClockSeconds'], 'wallClockNote': rec['wallClockNote'], 'wallClockSource': rec['wallClockSource']}, fh2)
# The same lock scripts/ledger-lock.mjs takes: a rewrite of the ledger by another
# run must not read the file before this append and write it back after.
import atexit
_lock = path + '.lock'
_deadline = time.time() + 15
while True:
    try:
        os.mkdir(_lock)
        break
    except FileExistsError:
        try:
            if time.time() - os.stat(_lock).st_mtime > 60:
                os.rmdir(_lock)
                continue
        except OSError:
            continue
        if time.time() > _deadline:
            sys.exit('could not lock %s within 15 s, nothing written' % path)
        time.sleep(0.05)
def _unlock():
    try:
        os.rmdir(_lock)
    except OSError:
        pass
atexit.register(_unlock)
# Mark a repeat of the same commit so aggregates can exclude it: four rows for
# PR 1957 at one headSha were counted as four independent samples.
prior = 0
try:
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line: continue
            try: row = json.loads(line)
            except Exception: continue
            if row.get('pr') == rec.get('pr') and row.get('headSha') == rec.get('headSha') and (
                not row.get('repo') or not rec.get('repo') or row.get('repo') == rec.get('repo')
            ):
                prior += 1
except FileNotFoundError:
    pass
rec['rerunOf'] = prior or None
with open(path, 'a') as fh:
    fh.write(json.dumps(rec) + '\n')
_unlock()
print('ok')
PYPERSIST

# Completed marker for the same inflight-runs.jsonl trail the Context phase
# opened (context-bookkeeping). Written only once persist-result.txt reports ok
# — a run that dies before this point keeps its "started" entry unmatched, which
# is the whole point: that's the signal a run was lost.
if grep -q '^ok' "$VD/persist-result.txt" 2>/dev/null; then
  TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  if [ -n "$HEAD_SHA" ]; then HS="\"$HEAD_SHA\""; else HS=null; fi
  if [ -n "$PR_REPO" ]; then RP="\"$PR_REPO\""; else RP=null; fi
  cat <<DONEJSON >> "$DATA_DIR/inflight-runs.jsonl"
{"event":"completed","pr":$PR_NUMBER,"repo":$RP,"headSha":$HS,"batchPanels":$BATCH_PANELS,"ts":"$TS"}
DONEJSON
fi

VD="$VD" python3 <<'PYREPORT'
import json, os
vd = os.environ['VD']
metrics = json.load(open(vd + '/metrics.json'))
persisted = open(vd + '/persist-result.txt').read().strip()
try:
    wall = json.load(open(vd + '/wallclock.json'))
except Exception:
    wall = {}
print(json.dumps({'reconciliation': metrics.get('voteReconciliation'), 'selfAudit': metrics.get('selfAudit'), 'persisted': persisted, 'wallClockSeconds': wall.get('wallClockSeconds'), 'wallClockNote': wall.get('wallClockNote'), 'wallClockSource': wall.get('wallClockSource')}))
PYREPORT
