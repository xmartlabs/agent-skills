#!/usr/bin/env bash
# classify-head-failure.sh <stderr-or-output-file> — why an external head exited non-zero.
#
# A head that failed used to be recorded as a bare `error`, and its stderr went
# to a mktemp nobody read or removed. On 2026-10-18 every Codex call from 17:48
# on failed with "ERROR: Your workspace is out of credits", and the runs said
# only "codex did not respond (error)" while the panels quietly lost their
# second model family.
#
# Prints two lines:
#   FAILCLASS: quota | auth | error
#   REASON: <the head's first error line, at most 200 characters>
#
# Only lines the CLI marks as errors are read. Codex echoes the whole prompt and
# the files it opened into stderr, and on a PR about invoices or billing a match
# over the full text would call a healthy head out of credits. Always exits 0.
set -uo pipefail

FILE="${1:-}"
TEXT=""
[ -n "$FILE" ] && [ -f "$FILE" ] && TEXT=$(cat "$FILE")

ERRORS=$(printf '%s\n' "$TEXT" | grep -E '^[[:space:]]*(\[API Error|ERROR|Error|error)[]:[:space:]]' || true)
FIRST=$(printf '%s\n' "$ERRORS" | sed -n '1p' | sed -E 's/^[[:space:]]+//' | cut -c1-200)

if printf '%s\n' "$ERRORS" | grep -qiE 'out of credits|insufficient[_ ]quota|exceeded your current quota|billing|credit balance|quota exceeded'; then
  CLASS=quota
elif printf '%s\n' "$ERRORS" | grep -qiE '(^|[^0-9])40[13]([^0-9]|$)|unauthori[sz]ed|not logged in|log ?in|invalid.?(api.?)?key|authentication'; then
  CLASS=auth
else
  CLASS=error
fi

echo "FAILCLASS: $CLASS"
echo "REASON: ${FIRST:-no error line in the output}"
