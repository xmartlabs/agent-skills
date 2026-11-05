#!/usr/bin/env bash
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/helper.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
export RUNNER_TEMP="$TMP"
RUN="$HERE/../run.sh"
assert_ok "default configuration validates" bash "$RUN" validate
for setting in INPUT_TRIBUNAL_VERSION=114.0.0 INPUT_VERDICT=approve INPUT_REVIEW_MODE=delta INPUT_CODEX=maybe INPUT_BOT_COMMENTS=1 INPUT_CONCURRENCY=0 INPUT_MAX_PANELS=NaN INPUT_LENS_SAMPLES=5 INPUT_PRIOR_VERIFY=-1 INPUT_EFFORT=invalid; do
  assert_fail "invalid $setting is rejected" env "$setting" bash "$RUN" validate
 done
assert_ok "structured overrides validate" env INPUT_TRIBUNAL_VERSION=115.2.0 INPUT_VERDICT=suggested INPUT_REVIEW_MODE=incremental INPUT_CONCURRENCY=2 INPUT_LENS_SAMPLES=4 INPUT_EFFORT=high bash "$RUN" validate
assert_fail "Codex requires an explicit credential" env INPUT_CODEX=true bash "$RUN" validate
assert_ok "Codex accepts an explicit credential" env INPUT_CODEX=true INPUT_OPENAI_API_KEY=fixture-secret bash "$RUN" validate
finish_tests
