#!/usr/bin/env bash
# classify-verdict.sh — read an external review model's free-form answer and
# decide whether it cast a verdict. Shared by the peer-review Codex and Gemini
# refuters (and by eval/replay-verdicts.mjs) so the parsing lives in ONE place.
#
# Usage: classify-verdict.sh <file>
#   stdout, on FAILCLASS "none" exactly five lines; on a head failure two
#   (FAILCLASS and REASON); otherwise just the first:
#     FAILCLASS: none | unparseable | refused | confined | harness | quota | auth | error
#     VERDICT: REFUTED | NOT_REFUTED
#     CONFIDENCE: <0..1>          (0.5 when the model declared none — the same
#                                  default tallyVotes applies, never a new one)
#     CONFIDENCE_DECLARED: yes|no  (no means the 0.5 above is OURS, not the
#                                  model's — see the comment at the case below)
#     REASON: <one line, verbatim from the model>
#   always exits 0 — classification is the output, not the exit code.
#
# Everything the caller needs is extracted HERE, in shell, so the dispatched
# wrapper agent has nothing left to interpret — it copies four fields. That is
# deliberate: on PR #1911 two wrappers, asked to read the prose and pick a
# killReason, instead re-ran the CLI with prompts they invented and submitted
# their own synthesis as a 0.98-confidence vote. Less room to judge, less room
# to improvise. `"$(tribunal-root)"/lib/reconcile-votes.mjs` re-runs this script over the
# saved artifacts at the end of a review and names any vote that disagrees.
#
# Why this exists: the original classifier was `grep -q "^VERDICT:"`. Models
# answer `**VERDICT: REFUTED**`, `### Verdict: **REFUTED**`, `# VERDICT: REFUTED
# (FALSE POSITIVE)`, bare `REFUTED.` or `**CONFIRMED**`. On the PR #1911 run that
# grep discarded 12 of 34 external votes as abstentions — 8 of them REFUTED, i.e.
# the panel lost mostly dissent. See specs/2026-07-22-pr1911-postmortem.md.
#
# Matching rules:
#   - strip markdown decoration (*, `, _, leading #{1,6}, leading -/+/> markers)
#   - accept a verdict DECLARATION only: a line that IS the token, or a line
#     prefixed with "verdict:"/"veredicto:". Never a bare occurrence inside
#     prose — the word "refuted" appears constantly in the reasoning body.
#   - CONFIRMED means the finding stands, i.e. NOT_REFUTED.
#   - test not-refuted/confirmed BEFORE refuted: "NOT REFUTED" contains "REFUTED".
#
# Which line wins, when several match:
#   1. the LAST explicitly "verdict:"-labelled line — the prompt says "End with
#      EXACTLY these two lines", so the labelled declaration nearest the end is
#      the model's conclusion, not a step label from its reasoning;
#   2. otherwise the FIRST bare token line — models that skip the label
#      (`REFUTED.`, `**CONFIRMED**`) put it up front as a headline.
# Taking the first match unconditionally would let a verbose model's bolded
# section heading ("**Refuted:**" as a reasoning step) outrank its real verdict.
set -uo pipefail

FILE="${1:?usage: classify-verdict.sh <file>}"

if [ ! -s "$FILE" ]; then
  echo "FAILCLASS: refused"
  exit 0
fi

# A stub the invocation block wrote INSTEAD of calling the head. It is not a
# statement about the model — the model was never asked. Kept as its own class
# because on 2026-07-23 it was read as `unparseable`, which feeds sticky
# routing, so one bug in the harness's own bash latched both external families
# unhealthy and cost a whole run its decorrelation while reporting clean.
if head -1 "$FILE" | grep -qE "PROMPT TRUNCATED IN TRANSIT|^HARNESS FAULT:"; then
  echo "FAILCLASS: harness"
  exit 0
fi

# A head that exited non-zero. The invocation block writes this line first,
# from lib/classify-head-failure.sh, so the reason survives in the artifact:
# before it existed a failed head left no .out at all and the run could only
# say "error". quota and auth are about the account, never the finding.
FIRSTLINE=$(head -1 "$FILE")
if printf '%s\n' "$FIRSTLINE" | grep -qE '^HEAD FAILURE \((quota|auth|error)\): '; then
  echo "FAILCLASS: $(printf '%s\n' "$FIRSTLINE" | sed -E 's/^HEAD FAILURE \(([a-z]+)\).*/\1/')"
  echo "REASON: $(printf '%s\n' "$FIRSTLINE" | sed -E 's/^HEAD FAILURE \([a-z]+\): //' | cut -c1-200)"
  exit 0
fi

NORM=$(sed -e 's/[*`_]//g' \
           -e 's/^[[:space:]]*#\{1,6\}[[:space:]]*//' \
           -e 's/^[[:space:]]*[-+>][[:space:]]*//' "$FILE")

# An EPISTEMIC abstention the model declares about ITSELF: it was confined to a
# slice of the code that cannot settle the claim — a cross-file mechanism whose
# links live in files the prompt scoped out. This is NOT a refutation, and
# testing it BEFORE the token match is the whole point: such an answer carries
# no refuted/confirmed token, so it previously landed in `unparseable`, which
# both handed the finding a full-weight kill (via the refuter's "default to
# REFUTED" instruction) and tripped sticky routing on a perfectly healthy head.
# Measured on the 2026-07-24 PR #1968 run: Codex refuted a hand-verified REAL
# finding while stating outright that it would only bite "if you import
# extra-file assumptions ... which you told me not to do". `confined` is
# content-specific like `refused`, so it is deliberately kept out of
# markUnhealthy's list — it says nothing about the head's health.
if printf '%s\n' "$NORM" | grep -qiE \
  "^[[:space:]]*(verdict|veredicto)?[[:space:]]*[:：—–-]?[[:space:]]*(cannot[[:space:]._-]?assess|abstain)([^a-zA-Z]|\$)"; then
  echo "FAILCLASS: confined"
  exit 0
fi

TOKEN='(not[[:space:]._-]?refuted|refuted|confirmed)'

# 1. last labelled declaration
LINE=$(printf '%s\n' "$NORM" | grep -iE \
  "^[[:space:]]*(verdict|veredicto)[[:space:]]*[:：—–-]?[[:space:]]*${TOKEN}([^a-zA-Z]|\$)" | tail -1)

# 2. fall back to the first bare token line
if [ -z "$LINE" ]; then
  LINE=$(printf '%s\n' "$NORM" | grep -m1 -iE \
    "^[[:space:]]*${TOKEN}([^a-zA-Z]|\$)")
fi

if [ -z "$LINE" ]; then
  # Output exists but declares nothing: a formatting/refusal-shaped answer.
  # "unparseable" is systemic by construction (it will recur on every call),
  # which is why the caller lets it trip the sticky-routing circuit breaker
  # while a genuine empty "refused" stays exempt.
  echo "FAILCLASS: unparseable"
  exit 0
fi

echo "FAILCLASS: none"
if printf '%s' "$LINE" | grep -qiE 'not[[:space:]._-]?refuted|confirmed'; then
  echo "VERDICT: NOT_REFUTED"
else
  echo "VERDICT: REFUTED"
fi

# Confidence: the model's own declared value. Same markdown tolerance as the
# verdict, and the LAST one wins for the same reason (the prompt asks for it on
# the final line). Anything outside 0..1 is treated as undeclared rather than
# clamped — a model writing "CONFIDENCE: 95" meant 0.95 or nothing, and guessing
# which would be inventing a weight.
CONF=$(printf '%s\n' "$NORM" \
  | grep -iE '^[[:space:]]*(confidence|confianza)[[:space:]]*[:：—–-]?[[:space:]]*[0-9]' \
  | tail -1 \
  | sed -E 's/.*[:：—–-][[:space:]]*//; s/[^0-9.].*$//')
case "$CONF" in
  0|1|0.[0-9]*|1.0*) CONF_DECLARED=yes ;;
  *) CONF="0.5"; CONF_DECLARED=no ;;
esac
echo "CONFIDENCE: $CONF"
# Whether that 0.5 is the model's answer or ours. They used to be the same four
# characters on the wire, so a head that declared nothing and a head that
# deliberately said "half" were indistinguishable everywhere downstream — in the
# tally's weighting, in the persisted vote rows, and in the report. Writing a
# default into a field and then reading it back as data is fabrication, however
# reasonable the default; this line is what makes it answerable instead.
echo "CONFIDENCE_DECLARED: $CONF_DECLARED"

# Reason: the verdict line itself, minus the label and any leading dash. It is
# what the model said, verbatim — the wrapper does not get to rewrite it.
REASON=$(printf '%s' "$LINE" \
  | sed -E 's/^[[:space:]]*(verdict|veredicto)[[:space:]]*[:：—–-]?[[:space:]]*//I' \
  | sed -E 's/^(not[[:space:]._-]?refuted|refuted|confirmed)[[:space:]]*[.:—–-]*[[:space:]]*//I' \
  | sed -E 's/^\(.*\)[[:space:]]*$//')
# A bare headline verdict ("REFUTED." on its own line) carries no reason on that
# line — the argument starts on the next non-empty one. Take that instead of
# reporting "no reason", which is what the common Codex format would produce.
if [ -z "$REASON" ]; then
  MATCH_LINE=$(printf '%s\n' "$NORM" | grep -nxF -m1 -- "$LINE" | cut -d: -f1)
  # Skip the model's own metadata lines (CONFIDENCE, refutationType, …) and
  # bare section headings — the first line of actual argument is the reason.
  [ -n "$MATCH_LINE" ] && REASON=$(printf '%s\n' "$NORM" |
    awk -v n="$MATCH_LINE" '
      NR <= n || !NF { next }
      tolower($0) ~ /^[[:space:]]*(confidence|confianza|refutationtype|decisionquestioned|killreason|coresstands|corestands|source|slotstatus)[[:space:]]*:/ { next }
      /^[[:space:]]*[A-Za-z][A-Za-z ]{0,30}:[[:space:]]*$/ { next }
      { print; exit }')
fi
# A model that echoed the field label ("REASON: …") would otherwise double it.
REASON=$(printf '%s' "$REASON" |
  sed -E 's/^[[:space:]]*(reason|razón|razon)[[:space:]]*[:：][[:space:]]*//I' |
  cut -c1-400)
[ -z "$REASON" ] && REASON="(no reason line — see the raw output)"
echo "REASON: $REASON"
