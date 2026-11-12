#!/usr/bin/env bash
# Runs the design-space guard policy tests (node:test, no dependencies).
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
echo "design_space_guard_test:"
OUT="$(node --test --test-reporter=tap "$DIR/tests/design_space/guard.test.mjs" 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || echo "$OUT" | grep -E "^(not ok|# (fail|pass))|Error|error" | head -40
PASS="$(echo "$OUT" | sed -n 's/^# pass \([0-9]*\)$/\1/p')"
FAIL="$(echo "$OUT" | sed -n 's/^# fail \([0-9]*\)$/\1/p')"
assert_eq "guard policy tests pass (node --test exit status)" 0 "$rc"
assert_eq "no guard policy case failed" 0 "${FAIL:-missing}"
assert_ok "guard policy cases ran (${PASS:-0} passed)" test "${PASS:-0}" -gt 50
finish_tests
