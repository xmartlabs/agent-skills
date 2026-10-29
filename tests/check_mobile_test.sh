#!/usr/bin/env bash
# Exercises the mobile branch of `run.sh check` (the mobile guard) and confirms the web/testOrigin
# path is unchanged. No network.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/skills/pr-evidence/scripts/run.sh"
echo "check_mobile_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
is_nonzero() { [ "$1" -ne 0 ]; }
w() { printf '%s' "$2" > "$TMP/$1"; }

# 1. Mobile config, debug build required, no apiBase -> passes, prints "mobile:<platform>".
w ok.json '{"platform":"ios","guard":{"requireDebugBuild":true}}'
OUT="$(bash "$RUN" check "$TMP/ok.json" 2>/dev/null)"; RC=$?
assert_eq "mobile ok exits 0" "0" "$RC"
assert_eq "mobile ok prints platform" "mobile:ios" "$OUT"

# 2. Mobile config missing requireDebugBuild -> refused.
w nodebug.json '{"platform":"android","guard":{}}'
OUT2="$(bash "$RUN" check "$TMP/nodebug.json" 2>/dev/null)"; RC2=$?
assert_ok "missing-debug refused" is_nonzero "$RC2"
assert_eq "missing-debug prints no origin" "" "$OUT2"

# 3. Mobile config whose apiBase is a forbidden (prod) host -> refused.
w prodapi.json '{"platform":"ios","guard":{"requireDebugBuild":true,"apiBase":"https://api.prod.example.com","forbidApiHosts":["api.prod.example.com"]}}'
OUT3="$(bash "$RUN" check "$TMP/prodapi.json" 2>/dev/null)"; RC3=$?
assert_ok "prod apiBase refused" is_nonzero "$RC3"
assert_eq "prod apiBase prints nothing" "" "$OUT3"

# 4. Mobile config whose apiBase is NOT forbidden -> passes.
w devapi.json '{"platform":"ios","guard":{"requireDebugBuild":true,"apiBase":"https://api.dev.example.test","forbidApiHosts":["api.prod.example.com"]}}'
OUT4="$(bash "$RUN" check "$TMP/devapi.json" 2>/dev/null)"; RC4=$?
assert_eq "dev apiBase exits 0" "0" "$RC4"
assert_eq "dev apiBase prints platform" "mobile:ios" "$OUT4"

# 5. Regression: a web config (testOrigin, no platform) still takes the origin path.
w web.json '{"testOrigin":"http://localhost:3000"}'
OUT5="$(bash "$RUN" check "$TMP/web.json" 2>/dev/null)"; RC5=$?
assert_eq "web still works" "0" "$RC5"
assert_eq "web prints origin" "http://localhost:3000" "$OUT5"

# 6. Regression: a web config pointed at prod is still refused.
w webprod.json '{"testOrigin":"https://app.example.com"}'
OUT6="$(bash "$RUN" check "$TMP/webprod.json" 2>/dev/null)"; RC6=$?
assert_ok "web prod refused" is_nonzero "$RC6"

finish_tests
