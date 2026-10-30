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
jq_expr=""; method=GET; args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --jq) jq_expr="$2"; shift 2 ;;
    -X) method="$2"; shift 2 ;;
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
    case "$method ${args[1]}" in
      "DELETE "*/issues/*/labels/*)
        if [ -f "$FX/label-missing" ]; then echo "gh: Label does not exist (HTTP 404)" >&2; exit 1; fi
        echo "${args[1]}" >> "$FX/deleted-labels" ;;
      "GET "*/issues/*/comments*)
        if [ -f "$FX/comments-error" ]; then echo "gh: Server Error (HTTP 502)" >&2; exit 1; fi
        emit "$FX/comments.json" ;;
      "GET "*/pulls/*/files*) emit "$FX/files.json" ;;
      "GET "*/contents/*)
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
printf 'OAUTH=%s\nAPI_KEY=%s\n' "${CLAUDE_CODE_OAUTH_TOKEN-<unset>}" "${ANTHROPIC_API_KEY-<unset>}" >> "$FX/claude-env"
printf 'GIT_AUTHOR=%s <%s>\nGIT_COMMITTER=%s <%s>\n' "$GIT_AUTHOR_NAME" "$GIT_AUTHOR_EMAIL" \
  "$GIT_COMMITTER_NAME" "$GIT_COMMITTER_EMAIL" >> "$FX/claude-env"
# The Authorization headers git would send to github.com: an empty value resets the list.
git config --get-all http.https://github.com/.extraheader | awk '/^$/ {n = 0; next} {h[n++] = $0}
  END {for (i = 0; i < n; i++) print "HEADER=" h[i]}' >> "$FX/claude-env"
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
gate() { : > "$GITHUB_OUTPUT"; HAS_CLAUDE_AUTH="${HAS_CLAUDE:-true}" INPUT_LABEL=pr-evidence INPUT_PATHS="${1:-}" ATTACH_TOKEN="${2:-}" bash "$RUN" gate 2>/dev/null; sed -n 's/^run=//p' "$GITHUB_OUTPUT"; }

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
echo '[{"user":{"login":"other-app[bot]","type":"Bot"},"body":"> <!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
assert_eq "another bot quoting the marker does not stop the run" "true" "$(gate)"
new_job opened false
assert_eq "without Claude credentials (e.g. Dependabot) it does not run" "false" "$(HAS_CLAUDE=false gate)"
new_job opened false
echo '[{"user":{"login":"evidence-bot","type":"User"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
echo '{"login":"evidence-bot"}' > "$FX/user.json"
assert_eq "posted evidence from the attach-token user stops the run" "false" "$(gate '' attach-secret)"
new_job labeled false pr-evidence
echo '[{"user":{"login":"github-actions[bot]","type":"Bot"},"body":"<!-- pr-evidence:posted -->"}]' > "$FX/comments.json"
assert_eq "the label runs even after posted evidence" "true" "$(gate)"

# once and drafts. Given explicitly, the defaults keep the rules above.
POSTED_BY_BOT='[{"user":{"login":"github-actions[bot]","type":"Bot"},"body":"<!-- pr-evidence:posted -->\nshot","html_url":"https://github.com/acme/web-app/pull/12#issuecomment-99"}]'
new_job opened true
assert_eq "drafts: false skips a draft" "false" "$(INPUT_DRAFTS=false gate)"
new_job opened false; echo "$POSTED_BY_BOT" > "$FX/comments.json"
assert_eq "once: true skips after posted evidence" "false" "$(INPUT_ONCE=true gate)"
new_job opened false; echo "$POSTED_BY_BOT" > "$FX/comments.json"
assert_eq "once: false runs again after posted evidence" "true" "$(INPUT_ONCE=false gate)"
new_job opened true
assert_eq "drafts: true runs a draft on an automatic event" "true" "$(INPUT_DRAFTS=true gate)"
new_job opened true
assert_eq "drafts: true still applies paths" "false" "$(INPUT_DRAFTS=true gate 'apps/**')"
new_job labeled true pr-evidence; echo "$POSTED_BY_BOT" > "$FX/comments.json"
assert_eq "the label ignores once, drafts and paths" "true" "$(INPUT_ONCE=true INPUT_DRAFTS=false gate 'apps/**')"

# The gate sub-action computes HAS_CLAUDE_AUTH from its inputs. gate_has_auth NAME=VALUE...
# evaluates that expression as GitHub would; inputs not given take their manifest defaults.
GATE_YML="$DIR/actions/pr-evidence/gate/action.yml"
gate_has_auth() {
  python3 - "$GATE_YML" "$@" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
given = dict(a.split("=", 1) for a in sys.argv[2:])
expr = re.search(r"HAS_CLAUDE_AUTH: \$\{\{(.*?)\}\}", text).group(1)
def value(name):
    if name in given: return given[name]
    m = re.search(r"^  " + re.escape(name) + r":\n(?:    .*\n)*?    default: '?([^'\n]*)'?$", text, re.M)
    return m.group(1) if m else ""
terms = []
for t in expr.split("||"):
    m = re.fullmatch(r"\s*inputs\.([\w-]+)\s*(==|!=)\s*'([^']*)'\s*", t)
    if not m: sys.exit("unsupported term: " + t)
    terms.append((value(m[1]) == m[3]) == (m[2] == "=="))
print("true" if any(terms) else "false")
PY
}
assert_eq "gate: claude-token alone is a credential" "true" "$(gate_has_auth claude-token=x)"
assert_eq "gate: anthropic-api-key alone is a credential" "true" "$(gate_has_auth anthropic-api-key=x)"
assert_eq "gate: the deprecated has-claude-credential still counts" "true" "$(gate_has_auth has-claude-credential=true)"
assert_eq "gate: has-claude-credential false and no credential" "false" "$(gate_has_auth has-claude-credential=false)"
assert_eq "gate: no credential at all" "false" "$(gate_has_auth)"
# manifest_input FILE NAME: the input's raw default, `required` when it has none, or <missing>.
manifest_input() {
  python3 -c 'import re,sys
t = open(sys.argv[1]).read()
m = re.search(r"^  " + re.escape(sys.argv[2]) + r":\n((?:    .*\n)*)", t, re.M)
d = m and re.search(r"^    default: (.*)$", m.group(1), re.M)
print(d.group(1) if d else ("required" if m else "<missing>"))' "$1" "$2"
}
assert_eq "gate: has-claude-credential is optional" "''" "$(manifest_input "$GATE_YML" has-claude-credential)"
for m in action.yml gate/action.yml; do
  assert_eq "$m: once defaults to true" "'true'" "$(manifest_input "$DIR/actions/pr-evidence/$m" once)"
  assert_eq "$m: drafts defaults to false" "'false'" "$(manifest_input "$DIR/actions/pr-evidence/$m" drafts)"
  assert_eq "$m: claude-token defaults to empty" "''" "$(manifest_input "$DIR/actions/pr-evidence/$m" claude-token)"
  assert_eq "$m: anthropic-api-key defaults to empty" "''" "$(manifest_input "$DIR/actions/pr-evidence/$m" anthropic-api-key)"
done

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

new_job opened false; printf '%s' '{"testOrigin":"http://[::1]:3000","boot":"x"}' > "$FX/base-config.json"
prepare
assert_eq "an IPv6 testOrigin keeps its brackets" "http://[::1]:$(cat "$TMP/runner/pr-evidence/port")" "$(cfg_val "$TMP/ws/.pr-evidence/config.json" "testOrigin")"

new_job opened false; echo "gh: Server Error (HTTP 502)" > "$FX/contents-error"
echo '{"tracked":true}' > "$TMP/ws/.pr-evidence/config.json"
prepare; (cd "$TMP/ws" && bash "$RUN" cleanup >/dev/null 2>&1)
assert_eq "cleanup after a failed prepare keeps the checkout's config" '{"tracked":true}' "$(cat "$TMP/ws/.pr-evidence/config.json")"

# --- capture ------------------------------------------------------------------------------------
capture() { (cd "$TMP/ws" && GITHUB_TOKEN_INPUT=job-secret ATTACH_TOKEN="${1:-}" \
  CLAUDE_CODE_OAUTH_TOKEN="${OAUTH-oauth-secret}" ANTHROPIC_API_KEY="${API_KEY-}" \
  INPUT_INSTRUCTIONS="${2:-}" INPUT_MAX_TURNS=50 INPUT_CLAUDE_ARGS="--model m" bash "$RUN" capture >/dev/null 2>&1); }
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare
git -C "$TMP/ws" init -q 2>/dev/null
git -C "$TMP/ws" config http.https://github.com/.extraheader "AUTHORIZATION: basic CHECKOUT"
capture "" docs/evidence.md
assert_ok "the prompt names the PR" grep -q "PR_NUMBER=12" "$FX/claude-prompt.md"
assert_ok "the prompt points at the staged skill" grep -qF "$TMP/runner/pr-evidence/claude/skills/pr-evidence/scripts/run.sh host" "$FX/claude-prompt.md"
assert_ok "the prompt includes the repository instructions" grep -qF '`docs/evidence.md`' "$FX/claude-prompt.md"
assert_ok "the prompt's environment and publishing win over the repository document" grep -qF "always apply, even where that document names other paths" "$FX/claude-prompt.md"
assert_fail "no placeholder is left" grep -q '{{' "$FX/claude-prompt.md"
assert_ok "git-branch mode uses the job token" grep -qx "GH_TOKEN=job-secret" "$FX/claude-env"
assert_ok "Claude gets the port" grep -qx "PORT=$(cat "$TMP/runner/pr-evidence/port")" "$FX/claude-env"
assert_ok "Claude gets the session" grep -qx "SESS=pr-evidence-7-1" "$FX/claude-env"
assert_ok "Claude reads the staged config dir" grep -qx "CLAUDE_CONFIG_DIR=$TMP/runner/pr-evidence/claude" "$FX/claude-env"
assert_ok "extra claude args are passed" grep -q -- "--max-turns 50 .*--model m" "$FX/claude-env"
assert_ok "hosting commits have an author" grep -qx "GIT_AUTHOR=github-actions\[bot\] <41898282+github-actions\[bot\]@users.noreply.github.com>" "$FX/claude-env"
assert_ok "hosting commits have a committer" grep -q "^GIT_COMMITTER=github-actions\[bot\] <" "$FX/claude-env"
assert_eq "git sends exactly one Authorization header" "1" "$(grep -c '^HEADER=' "$FX/claude-env")"
assert_ok "the header carries github-token" grep -qx "HEADER=AUTHORIZATION: basic $(printf 'x-access-token:job-secret' | base64)" "$FX/claude-env"
assert_ok "the OAuth token reaches Claude" grep -qx "OAUTH=oauth-secret" "$FX/claude-env"
assert_ok "no API key is set when none was given" grep -qx "API_KEY=<unset>" "$FX/claude-env"
OAUTH="" API_KEY=api-secret capture
assert_ok "an API key alone reaches Claude" grep -qx "API_KEY=api-secret" "$FX/claude-env"
assert_ok "with an API key alone no OAuth token is set" grep -qx "OAUTH=<unset>" "$FX/claude-env"
API_KEY=api-secret capture
assert_ok "with both, the API key is used" grep -qx "API_KEY=api-secret" "$FX/claude-env"
assert_ok "with both, the OAuth token is dropped" grep -qx "OAUTH=<unset>" "$FX/claude-env"
capture
assert_ok "hosting passes the config path" grep -qF "run.sh host --config .pr-evidence/config.json pr-12" "$FX/claude-prompt.md"

new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare attach-secret
capture attach-secret
assert_ok "attachment mode uses the attach token" grep -qx "GH_TOKEN=attach-secret" "$FX/claude-env"
assert_ok "attachment mode posts files with the comment" grep -qF "run.sh comment 12 <body-file> <files...>" "$FX/claude-prompt.md"
assert_fail "attachment mode has no hosting step" grep -qF "run.sh host --config" "$FX/claude-prompt.md"

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
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare
(cd "$TMP/ws" && INPUT_LABEL="pr evidence/ui" bash "$RUN" cleanup >/dev/null 2>&1)
assert_eq "cleanup removes the URL-encoded label" "repos/acme/web-app/issues/12/labels/pr%20evidence%2Fui" "$(cat "$FX/deleted-labels" 2>/dev/null)"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare; touch "$FX/label-missing"
(cd "$TMP/ws" && INPUT_LABEL=pr-evidence bash "$RUN" cleanup >/dev/null 2>&1); RC=$?
assert_eq "cleanup tolerates a label that is not there" "0" "$RC"
assert_fail "cleanup still removes the job directory after a 404" test -d "$TMP/runner/pr-evidence"

# --- summary ------------------------------------------------------------------------------------
summary() { GITHUB_STEP_SUMMARY="$TMP/runner/summary.md" bash "$RUN" summary 2>/dev/null; }
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare; capture
echo "$POSTED_BY_BOT" > "$FX/comments.json"
summary; RC=$?
assert_eq "summary exits 0" "0" "$RC"
assert_eq "summary: posted" "### PR evidence — PR #12 · ✅ posted" "$(head -n 1 "$TMP/runner/summary.md")"
assert_ok "summary links the posted comment" grep -qF "(https://github.com/acme/web-app/pull/12#issuecomment-99)" "$TMP/runner/summary.md"
assert_ok "summary reads only comments since the run started" \
  grep -qF "comments?per_page=100&since=$(cat "$TMP/runner/pr-evidence/started-at")" "$FX/gh.log"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare; capture
echo '[{"user":{"login":"dev","type":"User"},"body":"<!-- pr-evidence:posted -->","html_url":"https://x/1"}]' > "$FX/comments.json"
summary
assert_eq "summary: a person's comment is not the evidence" "### PR evidence — PR #12 · ⏳ not posted: the evidence run ended without posting a result" "$(head -n 1 "$TMP/runner/summary.md")"
new_job opened false; echo "gh: Server Error (HTTP 502)" > "$FX/contents-error"; prepare
summary
assert_eq "summary: setup failed before the capture" "### PR evidence — PR #12 · ⏳ not posted: the action's setup failed before the capture started" "$(head -n 1 "$TMP/runner/summary.md")"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare; capture
echo "$POSTED_BY_BOT" > "$FX/comments.json"; touch "$FX/comments-error"
summary; RC=$?; rm -f "$FX/comments-error"
assert_eq "summary exits 0 when it cannot list the comments" "0" "$RC"
assert_eq "summary: unknown when it cannot list the comments" "### PR evidence — PR #12 · ⚠️ unknown: could not list the pull request's comments" "$(head -n 1 "$TMP/runner/summary.md")"
new_job opened false; printf '%s' "$base_cfg" > "$FX/base-config.json"; prepare; capture; : > "$FX/gh.log"
(unset GITHUB_STEP_SUMMARY; bash "$RUN" summary >/dev/null 2>&1); RC=$?
assert_eq "summary without GITHUB_STEP_SUMMARY exits 0" "0" "$RC"
assert_eq "summary without GITHUB_STEP_SUMMARY calls nothing" "" "$(cat "$FX/gh.log")"
assert_fail "summary without GITHUB_STEP_SUMMARY writes nothing" test -f "$TMP/runner/summary.md"

finish_tests
