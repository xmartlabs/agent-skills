#!/usr/bin/env bash
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/helper.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
export RUNNER_TEMP="$TMP" GITHUB_STEP_SUMMARY="$TMP/summary.md" GITHUB_EVENT_PATH="$TMP/event.json"
mkdir -p "$TMP/tribunal"
echo '{"pull_request":{"number":12}}' > "$GITHUB_EVENT_PATH"
echo 'Posted REQUEST CHANGES: https://github.com/acme/web-app/pull/12' > "$TMP/tribunal/post.log"
bash "$HERE/../run.sh" summary
assert_ok "a multiword verdict is shown as posted" grep -qF '✅ posted' "$GITHUB_STEP_SUMMARY"
assert_ok "a multiword verdict retains its review link" grep -qF 'https://github.com/acme/web-app/pull/12' "$GITHUB_STEP_SUMMARY"
echo incremental > "$TMP/tribunal/effective-mode"
: > "$GITHUB_STEP_SUMMARY"
bash "$HERE/../run.sh" summary
assert_ok "summary shows the effective review mode" grep -qF 'Review mode: `incremental`' "$GITHUB_STEP_SUMMARY"
echo '{"metrics":{"reviewMode":"full","incrementalFallback":"baseline ancestry diverged"}}' > "$TMP/tribunal/run.json"
: > "$GITHUB_STEP_SUMMARY"
bash "$HERE/../run.sh" summary
assert_ok "summary prefers the actual engine review mode" grep -qF 'Review mode: `full`' "$GITHUB_STEP_SUMMARY"
assert_ok "summary explains engine incremental fallback" grep -qF 'Incremental fallback: baseline ancestry diverged' "$GITHUB_STEP_SUMMARY"
finish_tests
