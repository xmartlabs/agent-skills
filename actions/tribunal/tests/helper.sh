#!/usr/bin/env bash
# Minimal dependency-free test harness. Source it, call assert_*, end with finish_tests.
_T_PASS=0; _T_FAIL=0
assert_ok()   { local m="$1"; shift; if "$@"; then _T_PASS=$((_T_PASS+1)); else _T_FAIL=$((_T_FAIL+1)); echo "  FAIL: $m"; fi; }
assert_fail() { local m="$1"; shift; if "$@"; then _T_FAIL=$((_T_FAIL+1)); echo "  FAIL: $m (expected failure)"; else _T_PASS=$((_T_PASS+1)); fi; }
assert_eq()   { local m="$1" exp="$2" act="$3"; if [ "$exp" = "$act" ]; then _T_PASS=$((_T_PASS+1)); else _T_FAIL=$((_T_FAIL+1)); echo "  FAIL: $m (expected '$exp' got '$act')"; fi; }
finish_tests(){ echo "  $_T_PASS passed, $_T_FAIL failed"; [ "$_T_FAIL" -eq 0 ]; }
