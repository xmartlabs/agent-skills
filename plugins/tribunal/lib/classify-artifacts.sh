#!/usr/bin/env bash
# classify-artifacts.sh <artifact-dir> [suffix-filter]
#
# Runs classify-verdict.sh over every saved external-head CLI output in a
# directory and prints ONE line of JSON: an object keyed by artifact basename
# (without the .out), each value {failclass, verdict, confidence, reason}.
#
# Why this exists: the external heads' verdicts used to reach the harness as
# something an AGENT reported. On the 2026-07-23 PR #1911 run two slots reported
# votes their own artifacts contradict — one of them NOT_REFUTED at confidence
# 1.0, the maximum weight in the tally, over an artifact that reads
# "gemini-exec.sh: line 22: 3: missing prompt". The classifier had said
# `unparseable` for both. Nothing was wrong with the classifier, the CLI wrapper
# or the pinned worktree; the relay was simply not the truth.
#
# So the harness stopped asking. It derives each external vote from the artifact
# the CLI wrote, and this script is the derivation: one deterministic pass, no
# model in the loop, output small enough to survive being relayed verbatim.
#
# An agent can still overwrite an artifact before this runs — that hole is real
# and stays open (see reconcile-votes.mjs, which counts invocations). What is
# closed is the cheaper deviation: casting a vote the CLI never returned.
set -uo pipefail

DIR="${1:?usage: classify-artifacts.sh <artifact-dir> [suffix-filter]}"
FILTER="${2:-}"
CLASSIFIER="$(tribunal-root)/lib/classify-verdict.sh"

[ -d "$DIR" ] || { echo '{}'; exit 0; }
[ -x "$CLASSIFIER" ] || [ -f "$CLASSIFIER" ] || { echo '{}'; exit 0; }

# The count is emitted alongside the entries so the caller can tell a complete
# relay from an abridged one. Every payload that crosses an agent boundary in
# this harness now carries its own expected size, because twice in one run an
# agent shortened one and nothing noticed.
total=0
for f in "$DIR"/*.out; do
  [ -e "$f" ] || continue
  base=$(basename "$f" .out)
  case "$base" in *"$FILTER"*) total=$((total + 1)) ;; esac
done

first=1
printf '{"total":%d,"entries":{' "$total"
for f in "$DIR"/*.out; do
  [ -e "$f" ] || continue
  base=$(basename "$f" .out)
  case "$base" in
    *"$FILTER"*) ;;
    *) continue ;;
  esac

  out=$(bash "$CLASSIFIER" "$f" 2>/dev/null)
  failclass=$(printf '%s\n' "$out" | sed -n 's/^FAILCLASS: *//p' | head -1)
  verdict=$(printf '%s\n' "$out" | sed -n 's/^VERDICT: *//p' | head -1)
  confidence=$(printf '%s\n' "$out" | sed -n 's/^CONFIDENCE: *//p' | head -1)
  confdecl=$(printf '%s\n' "$out" | sed -n 's/^CONFIDENCE_DECLARED: *//p' | head -1)
  reason=$(printf '%s\n' "$out" | sed -n 's/^REASON: *//p' | head -1)
  [ -n "$failclass" ] || failclass="unparseable"

  # Reason text is free-form model prose and is the only field that can carry
  # quotes, newlines or backslashes into the JSON — let python quote it rather
  # than hand-rolling an escape that a stray backslash defeats.
  entry=$(PYTHONUTF8=1 FAILCLASS="$failclass" VERDICT="$verdict" CONF="$confidence" CONFDECL="$confdecl" REASON="$reason" BASE="$base" python3 -c '
import json, os
v = os.environ["VERDICT"] or None
try:
    c = float(os.environ["CONF"])
except ValueError:
    c = None
print(json.dumps(os.environ["BASE"]) + ":" + json.dumps({
    "failclass": os.environ["FAILCLASS"],
    "verdict": v,
    "confidence": c,
    # None when the classifier did not say: an older artifact, or a failclass
    # that never reached the confidence line. False means the 0.5 above is the
    # default this harness applies, not a number the head returned. No
    # apostrophes in here — this block is inside a single-quoted shell string.
    "confidenceDeclared": {"yes": True, "no": False}.get(os.environ["CONFDECL"]),
    "reason": (os.environ["REASON"] or "")[:100],
}))
')
  [ -n "$entry" ] || continue
  [ "$first" = 1 ] || printf ','
  first=0
  printf '%s' "$entry"
done
printf '}}\n'
