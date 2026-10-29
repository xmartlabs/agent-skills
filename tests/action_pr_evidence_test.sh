#!/usr/bin/env bash
# Tests for actions/pr-evidence/run.sh: gate, prepare, capture, fallback and cleanup, with fake
# gh and claude binaries. Tool installation and warm-up need the network and are not covered.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/actions/pr-evidence/run.sh"
echo "action_pr_evidence_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/fx" "$TMP/ws"

# Fake gh: answers from fixture files in $FX and logs every call to $FX/gh.log.
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$FX/gh.log"
jq_expr=""; args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --jq) jq_expr="$2"; shift 2 ;;
    --paginate) shift ;;
    -H) shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
emit() { if [ -n "$jq_expr" ]; then jq -r "$jq_expr" < "$1"; else cat "$1"; fi; }
case "${args[0]} ${args[1]:-}" in
  "api user")
    [ -f "$FX/user.json" ] && [ "$GH_TOKEN" = attach-secret ] || { echo "HTTP 403" >&2; exit 1; }
    emit "$FX/user.json" ;;
  "pr comment")
    body=""; for ((i = 0; i < ${#args[@]}; i++)); do [ "${args[$i]}" = --body-file ] && body="${args[$((i + 1))]}"; done
    cp "$body" "$FX/posted-body.md" ;;
  api*)
    case "${args[1]}" in
      */issues/*/comments*) emit "$FX/comments.json" ;;
      */pulls/*/files*) emit "$FX/files.json" ;;
      */contents/*)
        if [ -f "$FX/base-config.json" ]; then cat "$FX/base-config.json"
        elif [ -f "$FX/contents-error" ]; then cat "$FX/contents-error" >&2; exit 1
        else echo "gh: Not Found (HTTP 404)" >&2; exit 1; fi ;;
      *) echo "unexpected api call: ${args[*]}" >&2; exit 1 ;;
    esac ;;
  *) echo "unexpected gh call: ${args[*]}" >&2; exit 1 ;;
esac
SH
# Fake claude: records its prompt and the environment it was given.
cat > "$TMP/bin/claude" <<'SH'
#!/usr/bin/env bash
cat > "$FX/claude-prompt.md"
printf 'GH_TOKEN=%s\nPORT=%s\nSESS=%s\nCLAUDE_CONFIG_DIR=%s\nARGS=%s\n' \
  "$GH_TOKEN" "$PORT" "$SESS" "$CLAUDE_CONFIG_DIR" "$*" > "$FX/claude-env"
SH
chmod +x "$TMP/bin/gh" "$TMP/bin/claude"

export PATH="$TMP/bin:$PATH" FX="$TMP/fx" GITHUB_REPOSITORY=acme/web-app GITHUB_RUN_ID=7 \
  GITHUB_RUN_ATTEMPT=1 GITHUB_SERVER_URL=https://github.com GH_TOKEN=job-secret

# new_job ACTION DRAFT [LABEL] [HEAD_REPO]: a fresh runner temp dir, event payload and outputs.
new_job() {
  rm -rf "$TMP/runner" "${FX:?}"/*; mkdir -p "$TMP/runner"
  export RUNNER_TEMP="$TMP/runner" GITHUB_OUTPUT="$TMP/runner/out" GITHUB_PATH="$TMP/runner/path"
  export GITHUB_EVENT_PATH="$TMP/runner/event.json"
  python3 - "$GITHUB_EVENT_PATH" "$1" "$2" "${3:-}" "${4:-acme/web-app}" <<'PY'
import json, sys
path, action, draft, label, head = sys.argv[1:]
e = {"action": action, "pull_request": {"number": 12, "draft": draft == "true",
     "base": {"sha": "b" * 40}, "head": {"repo": {"full_name": head}}}}
if label: e["label"] = {"name": label}
json.dump(e, open(path, "w"))
PY
  echo '[]' > "$FX/comments.json"
  echo '[{"filename":"src/app/page.tsx"},{"filename":"README.md"}]' > "$FX/files.json"
}
gate() { : > "$GITHUB_OUTPUT"; INPUT_LABEL=pr-evidence INPUT_PATHS="${1:-}" ATTACH_TOKEN="${2:-}" bash "$RUN" gate 2>/dev/null; sed -n 's/^run=//p' "$GITHUB_OUTPUT"; }

# --- gate ---------------------------------------------------------------------------------------
new_job opened false
assert_eq "opened PR runs" "true" "$(gate)"
new_job opened true
assert_eq "draft PR does not run" "false" "$(gate)"
new_job labeled true pr-evidence
assert_eq "the label forces a run on a draft" "true" "$(gate)"
new_job labeled false bug
assert_eq "another label does not run" "false" "$(gate)"
new_job opened false "" someone/fork
assert_eq "fork PR does not run" "false" "$(gate)"
new_job opened false
assert_eq "paths match runs" "true" "$(gate 'src/**')"
new_job opened false
assert_eq "paths without a match do not run" "false" "$(gate 'apps/**')"
new_job opened false
assert_eq "an excluded path does not count" "false" "$(gate $'src/**\n!src/app/**')"
new_job opened false
echo '[{"user":{"login":"github-actions[bot]","type":"Bot"},"body":"<!-- pr-evidence:posted -->\nshot"}]' > "$FX/comments.json"
assert_eq "posted evidence from the bot stops the run" "false" "$(gate)"
new_job opened false
echo '[{"user":{"login":"dev","type":"User"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
assert_eq "a person's comment with the marker does not stop the run" "true" "$(gate)"
new_job opened false
echo '[{"user":{"login":"evidence-bot","type":"User"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
echo '{"login":"evidence-bot"}' > "$FX/user.json"
assert_eq "posted evidence from the attach-token user stops the run" "false" "$(gate '' attach-secret)"
new_job labeled false pr-evidence
echo '[{"user":{"login":"github-actions[bot]","type":"Bot"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
assert_eq "the label runs even after posted evidence" "true" "$(gate)"

# --- prepare ------------------------------------------------------------------------------------
prepare() { (cd "$TMP/ws" && INPUT_CONFIG=.pr-evidence/config.json ATTACH_TOKEN="${1:-}" bash "$RUN" prepare 2>/dev/null); }
cfg_val() { # FILE DOTTED.KEY
  python3 -c 'import functools,json,sys;print(functools.reduce(lambda d,k:d.get(k,{}),sys.argv[2].split("."),json.load(open(sys.argv[1]))) or "")' "$1" "$2"
}
base_cfg='{"testOrigin":"http://127.0.0.1:3200","boot":"scripts/boot.sh $PORT","host":{"target":"github-attachment","branch":"assets"}}'

new_job opened false; rm -rf "$TMP/ws/.pr-evidence"
printf '%s' "$base_cfg" > "$FX/base-config.json"
mkdir -p "$TMP/ws/.pr-evidence"; echo '{"testOrigin":"http://127.0.0.1:1","boot":"evil.sh"}' > "$TMP/ws/.pr-evidence/config.json"
prepare; RC=$?
assert_eq "prepare exits 0" "0" "$RC"
assert_eq "the base-branch config wins over the checkout's" "scripts/boot.sh \$PORT" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "boot")"
PORT_USED="$(cat "$TMP/runner/pr-evidence/port")"
assert_eq "testOrigin points at the picked port" "http://127.0.0.1:$PORT_USED" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "testOrigin")"
assert_eq "without attach-token the host is git-branch" "git-branch" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "host.target")"
assert_eq "other host keys are kept" "assets" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "host.branch")"
assert_ok "the skill is staged in Claude's config dir" test -f "$TMP/runner/pr-evidence/claude/skills/pr-evidence/SKILL.md"
assert_eq "changed files are listed" "2" "$(wc -l < "$TMP/runner/pr-evidence/changed-files.txt" | tr -d ' ')"

new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"
prepare attach-secret
assert_eq "with attach-token the host is github-attachment" "github-attachment" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "host.target")"

new_job opened false; echo '{"testOrigin":"http://localhost:3000","boot":"head.sh"}' > "$TMP/ws/.pr-evidence/config.json"
prepare
assert_eq "without a base config the PR's copy is used" "head.sh" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "boot")"

new_job opened false; echo "gh: Bad credentials (HTTP 401)" > "$FX/contents-error"
prepare; RC=$?
assert_ok "any other error reading the base config fails" test "$RC" -ne 0

new_job opened false; printf '%s' '{"testOrigin":"https://app.example.com","boot":"x"}' > "$FX/base-config.json"
prepare; RC=$?
assert_ok "a non-local testOrigin is refused" test "$RC" -ne 0

# --- capture ------------------------------------------------------------------------------------
capture() { (cd "$TMP/ws" && GITHUB_TOKEN_INPUT=job-secret ATTACH_TOKEN="${1:-}" CLAUDE_CODE_OAUTH_TOKEN=c \
  INPUT_INSTRUCTIONS="${2:-}" INPUT_MAX_TURNS=50 INPUT_CLAUDE_ARGS="--model m" bash "$RUN" capture >/dev/null 2>&1); }
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare
capture "" docs/evidence.md
assert_ok "the prompt names the PR" grep -q "PR_NUMBER=12" "$FX/claude-prompt.md"
assert_ok "the prompt points at the staged skill" grep -qF "$TMP/runner/pr-evidence/claude/skills/pr-evidence/scripts/run.sh host pr-12" "$FX/claude-prompt.md"
assert_ok "the prompt includes the repository instructions" grep -qF '`docs/evidence.md`' "$FX/claude-prompt.md"
assert_fail "no placeholder is left" grep -q '{{' "$FX/claude-prompt.md"
assert_ok "git-branch mode uses the job token" grep -qx "GH_TOKEN=job-secret" "$FX/claude-env"
assert_ok "Claude gets the port" grep -qx "PORT=$(cat "$TMP/runner/pr-evidence/port")" "$FX/claude-env"
assert_ok "Claude gets the session" grep -qx "SESS=pr-evidence-7-1" "$FX/claude-env"
assert_ok "Claude reads the staged config dir" grep -qx "CLAUDE_CONFIG_DIR=$TMP/runner/pr-evidence/claude" "$FX/claude-env"
assert_ok "extra claude args are passed" grep -q -- "--max-turns 50 .*--model m" "$FX/claude-env"

new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare attach-secret
capture attach-secret
assert_ok "attachment mode uses the attach token" grep -qx "GH_TOKEN=attach-secret" "$FX/claude-env"
assert_ok "attachment mode posts files with the comment" grep -qF "run.sh comment 12 <body-file> <files...>" "$FX/claude-prompt.md"
assert_fail "attachment mode has no hosting step" grep -qF "run.sh host pr-12" "$FX/claude-prompt.md"

# --- fallback -----------------------------------------------------------------------------------
fallback() { INPUT_LABEL=pr-evidence bash "$RUN" fallback >/dev/null 2>&1; }
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare
fallback
assert_ok "nothing reported: a pending comment is posted" grep -qF "<!-- pr-evidence:pending -->" "$FX/posted-body.md"
assert_ok "the pending comment links the run" grep -qF "https://github.com/acme/web-app/actions/runs/7" "$FX/posted-body.md"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare
echo '[{"user":{"login":"github-actions[bot]","type":"Bot"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
fallback
assert_fail "already reported: no pending comment" test -f "$FX/posted-body.md"

# --- cleanup ------------------------------------------------------------------------------------
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"
echo '{"original":true}' > "$TMP/ws/.pr-evidence/config.json"; prepare
(cd "$TMP/ws" && bash "$RUN" cleanup >/dev/null 2>&1); RC=$?
assert_eq "cleanup exits 0" "0" "$RC"
assert_eq "cleanup restores the checkout's config" '{"original":true}' "$(cat "$TMP/ws/.pr-evidence/config.json")"
assert_fail "cleanup removes the job directory" test -d "$TMP/runner/pr-evidence"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; rm -rf "$TMP/ws/.pr-evidence"; prepare
(cd "$TMP/ws" && bash "$RUN" cleanup >/dev/null 2>&1)
assert_fail "cleanup removes a config the checkout did not have" test -f "$TMP/ws/.pr-evidence/config.json"

finish_tests
