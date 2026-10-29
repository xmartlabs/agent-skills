#!/usr/bin/env bash
# The offline test suite of tribunal that runs without the data directory
# (TRIBUNAL_HARNESS_DATA).
#
#   plugins/tribunal/scripts/check-portable.sh
#
# Some replays read real past runs, labelled PRs or frozen client profiles from
# that directory, which is not part of this repository. They are listed in
# needs-data.txt next to this script and skipped here; they run only where the directory
# exists. Everything else must pass.
#
# Both lists are kept honest in both directions: a replay that fails and is not
# listed is a failure, and a listed replay that passes is reported so it can leave
# the list. Replays run in a throwaway HOME, so no ledger on this machine is read
# or written. Exits 1 on any failure.
set -uo pipefail
PLUGIN=$(cd -P "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
NEEDS="$PLUGIN/scripts/needs-data.txt"

SB=$(mktemp -d)
trap 'rm -rf "$SB"' EXIT
mkdir -p "$SB/home/.claude" "$SB/tmp"
export HOME="$SB/home" USERPROFILE="$SB/home" TMPDIR="$SB/tmp"
export TRIBUNAL_DATA_DIR="$HOME/.claude/tribunal-data"
unset TRIBUNAL_HARNESS_DATA

fail=0; ran=0; skipped=0
export CLAUDE_PLUGIN_ROOT="$PLUGIN"
export PATH="$PLUGIN/bin:$PATH"
cd "$PLUGIN/skills/review-pr" || exit 2
for f in eval/replay-*.mjs; do
  name=$(basename "$f")
  if node "$f" >"$SB/out.txt" 2>&1; then
    ran=$((ran + 1))
    grep -qx "tribunal $name" "$NEEDS" && echo "note: $name passes without data, remove it from scripts/needs-data.txt"
  elif grep -qx "tribunal $name" "$NEEDS"; then
    skipped=$((skipped + 1))
  else
    fail=$((fail + 1)); echo "FAIL $name"; tail -5 "$SB/out.txt" | sed 's/^/     /'
  fi
done
# An entry for a replay that no longer exists hides nothing, but it hides the
# fact that the list went stale.
while read -r _ n; do
  [ -f "eval/$n" ] || { fail=$((fail + 1)); echo "FAIL needs-data.txt lists $n, which does not exist"; }
done < "$NEEDS"
node eval/check-mirror.mjs >"$SB/mirror.txt" 2>&1 \
  && echo "ok   check-mirror: $(grep -o 'mirror invariants: [0-9/]* agree' "$SB/mirror.txt" | tail -1)" \
  || { fail=$((fail + 1)); echo "FAIL check-mirror"; tail -5 "$SB/mirror.txt" | sed 's/^/     /'; }

echo "portable replays passed: $ran, skipped for needing data: $skipped, failed: $fail"
exit $((fail > 0))
