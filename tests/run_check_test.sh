#!/usr/bin/env bash
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/skills/pr-evidence/scripts/run.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
echo "run_check_test:"
printf '{"testOrigin":"http://localhost:3001","boot":"true","auth":{"strategy":"none"},"host":{"target":"git-branch"},"tracker":{"pr":"github"}}' > "$TMP/ok.json"
printf '{"testOrigin":"https://app.example.com","boot":"true","auth":{"strategy":"none"}}' > "$TMP/prod.json"
assert_eq  "valid config prints origin" "http://localhost:3001" "$(bash "$RUN" check "$TMP/ok.json")"
assert_fail "prod origin rejected"       bash "$RUN" check "$TMP/prod.json"
assert_fail "missing config rejected"    bash "$RUN" check "$TMP/does-not-exist.json"
# config present but testOrigin key absent -> reject
printf '{"boot":"true","auth":{"strategy":"none"}}' > "$TMP/nokey.json"
assert_fail "missing testOrigin rejected" bash "$RUN" check "$TMP/nokey.json"
# malformed JSON -> reject (cfg_get swallows the parse error, cmd_check fails closed)
printf 'not json at all' > "$TMP/bad.json"
assert_fail "malformed json rejected"    bash "$RUN" check "$TMP/bad.json"
# testOrigin present but non-scalar (object) -> is_test_origin rejects the json-dumped value
printf '{"testOrigin":{"nested":true}}' > "$TMP/obj.json"
assert_fail "non-scalar testOrigin rejected" bash "$RUN" check "$TMP/obj.json"
# testOrigin explicitly null -> treated as unset -> reject
printf '{"testOrigin":null}' > "$TMP/null.json"
assert_fail "null testOrigin rejected"   bash "$RUN" check "$TMP/null.json"
# the shipped example config must be valid JSON and pass check when copied verbatim
assert_eq  "example config passes check" "http://localhost:3001" \
  "$(bash "$RUN" check "$DIR/skills/pr-evidence/config/pr-evidence.config.example.json")"
finish_tests
