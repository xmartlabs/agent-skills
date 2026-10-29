#!/usr/bin/env bash
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/skills/pr-evidence/scripts/run.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
printf '{"testOrigin":"http://localhost:3001","host":{"target":"custom","upload":"echo https://cdn.example/img.png"}}' > "$TMP/c.json"
echo "host_comment_test:"
# custom host runs the upload command and returns its URL
assert_eq "custom host returns url" "https://cdn.example/img.png" "$(bash "$RUN" host --config "$TMP/c.json" PC-1 "$TMP/c.json")"
# custom host --dry-run prints the upload command instead of executing it
printf '{"testOrigin":"http://localhost:3001","host":{"target":"custom","upload":"touch %s/should-not-exist"}}' "$TMP" > "$TMP/dry.json"
out="$(bash "$RUN" host --config "$TMP/dry.json" --dry-run PC-1)"
assert_eq "custom host dry-run prints upload cmd" "touch $TMP/should-not-exist" "$out"
assert_ok "custom host dry-run does not execute" bash -c '[ ! -e "$1" ]' _ "$TMP/should-not-exist"
# comment dry-run prints a gh command
out="$(bash "$RUN" comment --dry-run 42 "$TMP/c.json")"
assert_ok "comment dry-run mentions gh pr comment" bash -c "printf '%s' \"\$1\" | grep -q 'gh pr comment 42'" _ "$out"
# default (git-branch) host with a config that omits `host` entirely -> uses built-in defaults
printf '{"testOrigin":"http://localhost:3001"}' > "$TMP/nohost.json"
out="$(bash "$RUN" host --config "$TMP/nohost.json" --dry-run PC-9)"
assert_eq "default host uses branch+remote defaults" "host-git-branch.sh pr-evidence origin PC-9 " "$out"
# flag order independence: --dry-run before --config must still resolve the config
out="$(bash "$RUN" host --dry-run --config "$TMP/c.json" PC-9)"
assert_eq "flag order independent (custom upload)" "echo https://cdn.example/img.png" "$out"
# custom host target with no host.upload -> hard error, no silent success
printf '{"testOrigin":"http://localhost:3001","host":{"target":"custom"}}' > "$TMP/noupload.json"
assert_fail "custom host without upload errors" bash "$RUN" host --config "$TMP/noupload.json" PC-9
# A fake gh records argument boundaries without publishing anything.
mkdir -p "$TMP/bin"
cat > "$TMP/bin/gh" <<'GH'
#!/usr/bin/env bash
if [ "${1:-}" = --version ]; then echo "gh version ${TEST_GH_VERSION:-2.100.0}"; exit 0; fi
if [ "${3:-}" = --help ]; then echo "${TEST_GH_HELP:---attach}"; exit 0; fi
printf '%s\n' "$@" > "$TEST_GH_ARGS"
GH
chmod +x "$TMP/bin/gh"
export PATH="$TMP/bin:$PATH" TEST_GH_ARGS="$TMP/args"
touch "$TMP/screen shot.png" "$TMP/clip.mp4"
assert_ok "multiple attachments" bash "$RUN" comment 42 "$TMP/c.json" "$TMP/screen shot.png" "$TMP/clip.mp4"
expected="$(printf '%s\n' pr comment 42 --body-file "$TMP/c.json" --attach "$TMP/screen shot.png" --attach "$TMP/clip.mp4")"
assert_eq "preserves argument boundaries" "$expected" "$(cat "$TMP/args")"
assert_ok "plain comment remains supported" bash "$RUN" comment 77 "$TMP/c.json"
assert_eq "plain comment arguments" "$(printf '%s\n' pr comment 77 --body-file "$TMP/c.json")" "$(cat "$TMP/args")"
rm "$TMP/args"
assert_ok "attachment dry run" bash "$RUN" comment --dry-run 42 "$TMP/c.json" "$TMP/clip.mp4"
assert_fail "missing capability rejected" env TEST_GH_HELP=unsupported bash "$RUN" comment 42 "$TMP/c.json" "$TMP/clip.mp4"
assert_fail "missing file rejected" bash "$RUN" comment 42 "$TMP/c.json" "$TMP/missing.png"
assert_fail "old gh rejected" env TEST_GH_VERSION=2.97.0 bash "$RUN" comment 42 "$TMP/c.json" "$TMP/clip.mp4"
assert_ok "failures never publish" test ! -e "$TMP/args"
printf '{"testOrigin":"http://localhost:3001","host":{"target":"github-attachment"}}' > "$TMP/native.json"
assert_fail "native host cannot return fake URLs" bash "$RUN" host --config "$TMP/native.json" PC-1 "$TMP/clip.mp4"
finish_tests
