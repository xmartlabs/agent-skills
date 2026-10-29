#!/usr/bin/env bash
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/skills/pr-evidence/scripts/run.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
echo "run_browser_test:"
printf '{"testOrigin":"http://localhost:3001","boot":"true"}' > "$TMP/none.json"
printf '{"testOrigin":"http://localhost:3001","boot":"true","browser":"chromium"}' > "$TMP/chromium.json"
printf '{"testOrigin":"http://localhost:3001","boot":"true","browser":"netscape"}' > "$TMP/bad.json"
printf '{"testOrigin":"http://localhost:3001","boot":"true","browser":null}' > "$TMP/null.json"
assert_eq   "browser defaults to chrome"      "chrome"   "$(bash "$RUN" browser "$TMP/none.json")"
assert_eq   "configured browser is printed"   "chromium" "$(bash "$RUN" browser "$TMP/chromium.json")"
assert_eq   "null browser falls back"         "chrome"   "$(bash "$RUN" browser "$TMP/null.json")"
assert_fail "unknown browser rejected"        bash "$RUN" browser "$TMP/bad.json"
assert_fail "missing config rejected"         bash "$RUN" browser "$TMP/does-not-exist.json"
# the shipped example config keeps the previous behavior (chrome)
assert_eq   "example config uses chrome"      "chrome" \
  "$(bash "$RUN" browser "$DIR/skills/pr-evidence/config/pr-evidence.config.example.json")"
finish_tests
