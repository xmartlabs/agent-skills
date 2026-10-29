#!/usr/bin/env bash
# Tests for actions/tribunal/run.sh: gate, prepare, review, post, fallback and cleanup, with fake
# gh, curl, claude and tribunal binaries. Nothing touches the network.
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/helper.sh"
RUN="$HERE/../run.sh"
echo "tribunal action run_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/fx" "$TMP/fixed-tmp" "$TMP/rel"

# A release archive holding a fake tribunal binary that records how it was run.
mkdir -p "$TMP/rel/tribunal-v9-x86_64-unknown-linux-musl"
cat > "$TMP/rel/tribunal-v9-x86_64-unknown-linux-musl/tribunal" <<'SH'
#!/usr/bin/env bash
sub="$1"; shift
{ echo "ARGS=$sub $*"; echo "PWD=$PWD"; echo "HOME=$HOME"; echo "TMPDIR=$TMPDIR"
  echo "DATA=$TRIBUNAL_DATA_DIR"; echo "ROOT=$TRIBUNAL_PLUGIN_ROOT"
  echo "TRIBUNAL_KEY=${TRIBUNAL_ANTHROPIC_API_KEY-<unset>}"; echo "OAUTH=${CLAUDE_CODE_OAUTH_TOKEN-<unset>}"
  echo "API_KEY=${ANTHROPIC_API_KEY-<unset>}"; } > "$FX/tribunal-$sub.env"
if [ "$sub" = review ]; then
  out=""; while [ $# -gt 0 ]; do [ "$1" = --out ] && out="$2"; shift; done
  if [ -f "$FX/run.json" ]; then cp "$FX/run.json" "$out"; else echo '{"report":"x"}' > "$out"; fi
fi
if [ "$sub" = post ]; then cat "$FX/post-output" 2>/dev/null; exit "$(cat "$FX/post-rc" 2>/dev/null || echo 0)"; fi
SH
chmod +x "$TMP/rel/tribunal-v9-x86_64-unknown-linux-musl/tribunal"
ASSET=tribunal-v9-x86_64-unknown-linux-musl.tar.gz
tar -czf "$TMP/rel/$ASSET" -C "$TMP/rel" tribunal-v9-x86_64-unknown-linux-musl
SHA="$( (sha256sum "$TMP/rel/$ASSET" 2>/dev/null || shasum -a 256 "$TMP/rel/$ASSET") | cut -d' ' -f1)"
manifest() { # SHA
  printf '{"tag":"tribunal-rust-v9","repo":"acme/tools","assets":{"x86_64-unknown-linux-musl":{"name":"%s","sha256":"%s"}}}' \
    "$ASSET" "$1" > "$TMP/release.json"
}

# Fake curl: serves the archive and logs the URL.
cat > "$TMP/bin/curl" <<'SH'
#!/usr/bin/env bash
dest=""; url=""
while [ $# -gt 0 ]; do case "$1" in -o) dest="$2"; shift 2 ;; -*) shift ;; *) url="$1"; shift ;; esac; done
echo "$url" >> "$FX/curl.log"
cp "$REL_ARCHIVE" "$dest"
SH
# Fake gh: profile lookups answer from $FX/profile-<base|default|head>, everything is logged.
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$FX/gh.log"
case "$*" in
  "--version") echo "gh version 2.103.3" ;;
  "auth setup-git") ;;
  "pr comment"*)
    while [ $# -gt 0 ]; do [ "$1" = --body-file ] && cp "$2" "$FX/posted-body.md"; shift; done ;;
  "api -X DELETE"*) [ -f "$FX/label-present" ] || { echo "HTTP 404" >&2; exit 1; } ;;
  *contents/.claude/review-profile.json*)
    case "$*" in
      *"?ref=main "*|*"?ref=main") f=base ;;
      *"?ref=$HEAD_SHA"*) f=head ;;
      *) f=default ;;
    esac
    [ -f "$FX/profile-$f" ] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    case "$*" in *"Accept: application/vnd.github.raw"*) echo '{"from":"'"$f"'"}' ;; *) echo .claude/review-profile.json ;; esac ;;
  *contents/.claude/review-rules.md*) [ -f "$FX/rules-head" ] && echo "<!-- rules -->" || { echo "HTTP 404" >&2; exit 1; } ;;
  *) echo "unexpected gh call: $*" >&2; exit 1 ;;
esac
SH
printf '#!/usr/bin/env bash\n' > "$TMP/bin/claude"
chmod +x "$TMP/bin/curl" "$TMP/bin/gh" "$TMP/bin/claude"

export PATH="$TMP/bin:$PATH" FX="$TMP/fx" REL_ARCHIVE="$TMP/rel/$ASSET" TRIBUNAL_TARGET=x86_64-unknown-linux-musl \
  TRIBUNAL_RUST_MANIFEST="$TMP/release.json" TRIBUNAL_FIXED_TMP="$TMP/fixed-tmp" GITHUB_REPOSITORY=acme/web-app \
  GITHUB_RUN_ID=7 GITHUB_SERVER_URL=https://github.com GH_TOKEN=job-secret HEAD_SHA=aaaa1111
PLUGIN_ROOT="$(cd "$HERE/../../../plugins/tribunal" && pwd)"

# new_job ACTION [LABEL] [HEAD_REPO]
new_job() {
  rm -rf "$TMP/runner" "${FX:?}"/*; mkdir -p "$TMP/runner"
  export RUNNER_TEMP="$TMP/runner" GITHUB_OUTPUT="$TMP/runner/out" GITHUB_PATH="$TMP/runner/path"
  export GITHUB_EVENT_PATH="$TMP/runner/event.json"
  python3 - "$GITHUB_EVENT_PATH" "$1" "${2:-}" "${3:-acme/web-app}" <<'PY'
import json, sys
path, action, label, head = sys.argv[1:]
e = {"action": action, "pull_request": {"number": 12, "base": {"ref": "main"},
     "head": {"sha": "aaaa1111", "repo": {"full_name": head}}}}
if label: e["label"] = {"name": label}
json.dump(e, open(path, "w"))
PY
  manifest "$SHA"
}
gate() { : > "$GITHUB_OUTPUT"; HAS_CLAUDE_AUTH="${HAS:-true}" INPUT_LABEL=tribunal bash "$RUN" gate 2>/dev/null; sed -n 's/^run=//p' "$GITHUB_OUTPUT"; }
prepare() { INPUT_REVIEWER="${1:-frontend}" bash "$RUN" prepare 2>/dev/null; }
job() { echo "$TMP/runner/tribunal"; }
args() { tr '\n' ' ' < "$(job)/profile-args"; }

# --- gate ---------------------------------------------------------------------------------------
new_job labeled tribunal;  assert_eq "the tribunal label starts a review" "true" "$(gate)"
new_job labeled bug;       assert_eq "another label does not" "false" "$(gate)"
new_job opened;            assert_eq "other events review (the caller chose them)" "true" "$(gate)"
new_job labeled tribunal someone/fork; assert_eq "fork PRs do not run" "false" "$(gate)"
new_job labeled tribunal;  assert_eq "no Claude credential: no run" "false" "$(HAS=false gate)"

# --- prepare ------------------------------------------------------------------------------------
new_job labeled tribunal; touch "$FX/profile-base"
prepare; RC=$?
assert_eq "prepare exits 0" "0" "$RC"
assert_eq "the binary comes from release.json's repo, tag and asset" \
  "https://github.com/acme/tools/releases/download/tribunal-rust-v9/$ASSET" "$(head -1 "$FX/curl.log")"
assert_ok "the binary is unpacked" test -x "$(cat "$(job)/bin-path")"
assert_ok "homosapiens is staged in the job's HOME" test -f "$(job)/home/.claude/skills/homosapiens/SKILL.md"
assert_eq "a base-branch profile needs no profile arguments" "" "$(args)"

new_job labeled tribunal; manifest "0000000000000000000000000000000000000000000000000000000000000000"
prepare; RC=$?
assert_ok "a checksum mismatch fails prepare" test "$RC" -ne 0
assert_fail "no binary is kept after a mismatch" test -f "$(job)/bin-path"

new_job labeled tribunal; touch "$FX/profile-default"
prepare
assert_eq "a default-branch profile needs no profile arguments" "" "$(args)"

new_job labeled tribunal; touch "$FX/profile-head" "$FX/rules-head"
prepare
assert_eq "without a base profile the PR's profile and rules are used" \
  "--profile $(job)/review-profile.json --rules $(job)/review-rules.md " "$(args)"
assert_eq "the PR's profile comes from its head commit" '{"from":"head"}' "$(cat "$(job)/review-profile.json")"

new_job labeled tribunal
prepare backend
assert_eq "without any profile the default one is passed" "--profile $(job)/review-profile.json " "$(args)"
cfg() { python3 -c 'import json,sys;print(json.dumps(json.load(open(sys.argv[1])).get(sys.argv[2])))' "$(job)/review-profile.json" "$1"; }
assert_eq "the default profile names the repository" '"acme/web-app"' "$(cfg repo)"
assert_eq "the default profile uses the PR's base branch" '"main"' "$(cfg baseBranch)"
assert_eq "the default profile is the requested reviewer kind" '"backend"' "$(cfg reviewer)"
assert_eq "the default profile has no rules file" 'null' "$(cfg projectRules)"
assert_eq "the default profile has no sibling repositories" '[]' "$(cfg crossRepoAnchors)"

new_job labeled tribunal
prepare mobile; RC=$?
assert_ok "an unknown reviewer kind fails" test "$RC" -ne 0

# --- review and post ----------------------------------------------------------------------------
review() { CLAUDE_CODE_OAUTH_TOKEN="${OAUTH-oauth-secret}" ANTHROPIC_API_KEY="${API_KEY-}" bash "$RUN" review >/dev/null 2>&1; }
new_job labeled tribunal; touch "$FX/profile-head"; prepare
review; RC=$?
assert_eq "review exits 0" "0" "$RC"
assert_eq "review flags" "ARGS=review --pr 12 --repo acme/web-app --head-sha aaaa1111 --no-codex --no-post --out $(job)/run.json --profile $(job)/review-profile.json" \
  "$(grep '^ARGS=' "$FX/tribunal-review.env")"
assert_ok "the run record is written" test -f "$(job)/run.json"
assert_ok "it runs from the job's empty work dir" grep -qx "PWD=$(job)/work" "$FX/tribunal-review.env"
assert_ok "HOME is the job's" grep -qx "HOME=$(job)/home" "$FX/tribunal-review.env"
assert_ok "TMPDIR is the job's" grep -qx "TMPDIR=$(job)/tmp" "$FX/tribunal-review.env"
assert_ok "the data dir is the job's" grep -qx "DATA=$(job)/data" "$FX/tribunal-review.env"
assert_ok "the plugin root is the action's own ref" grep -qx "ROOT=$PLUGIN_ROOT" "$FX/tribunal-review.env"
assert_ok "the OAuth token reaches the binary" grep -qx "OAUTH=oauth-secret" "$FX/tribunal-review.env"
assert_ok "no API key without one" grep -qx "TRIBUNAL_KEY=<unset>" "$FX/tribunal-review.env"
assert_ok "git fetches use the job token" grep -qx "auth setup-git" "$FX/gh.log"

OAUTH="" API_KEY=api-secret review
assert_ok "an API key goes in as TRIBUNAL_ANTHROPIC_API_KEY" grep -qx "TRIBUNAL_KEY=api-secret" "$FX/tribunal-review.env"
assert_ok "ANTHROPIC_API_KEY itself is not passed" grep -qx "API_KEY=<unset>" "$FX/tribunal-review.env"
API_KEY=api-secret review
assert_ok "with both, the API key is used" grep -qx "TRIBUNAL_KEY=api-secret" "$FX/tribunal-review.env"
assert_ok "with both, the OAuth token is dropped" grep -qx "OAUTH=<unset>" "$FX/tribunal-review.env"

post() { CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" post >/dev/null 2>&1; }
post; RC=$?
assert_eq "post exits 0 when the binary posts" "0" "$RC"
assert_eq "post flags" "ARGS=post --from $(job)/run.json --verdict comment --over-standing-review" \
  "$(grep '^ARGS=' "$FX/tribunal-post.env")"
echo 1 > "$FX/post-rc"; echo "NOT POSTED: duplicate-review gate is held" > "$FX/post-output"
post; RC=$?
assert_ok "post fails when the binary holds the post" test "$RC" -ne 0

# --- fallback -----------------------------------------------------------------------------------
fallback() { INPUT_LABEL=tribunal bash "$RUN" fallback >/dev/null 2>&1; }
fallback
assert_ok "a pending comment is posted" grep -qF "<!-- tribunal:pending -->" "$FX/posted-body.md"
assert_ok "it quotes why the post was held" grep -qF "NOT POSTED: duplicate-review gate is held" "$FX/posted-body.md"
assert_ok "it links the run" grep -qF "https://github.com/acme/web-app/actions/runs/7" "$FX/posted-body.md"
new_job labeled tribunal; touch "$FX/profile-base"; prepare
echo '{"skipped":true,"reason":"deployment branch"}' > "$FX/run.json"; review
fallback
assert_ok "a skipped run says why" grep -qF "Tribunal skipped this pull request: deployment branch." "$FX/posted-body.md"

# --- cleanup ------------------------------------------------------------------------------------
new_job labeled tribunal; touch "$FX/profile-base" "$FX/label-present"; prepare
touch "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-12.md" "$TMP/fixed-tmp/tribunal-conventions-acme-web-app-12.md" \
  "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-13.md"
mkdir -p "$TMP/fixed-tmp/peer-review-votes-12-abcd1234"
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1; RC=$?
assert_eq "cleanup exits 0" "0" "$RC"
assert_ok "the label is removed" grep -q "api -X DELETE repos/acme/web-app/issues/12/labels/tribunal" "$FX/gh.log"
assert_fail "the job directory is removed" test -d "$(job)"
assert_fail "this PR's methodology file is removed" test -f "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-12.md"
assert_fail "this PR's conventions file is removed" test -f "$TMP/fixed-tmp/tribunal-conventions-acme-web-app-12.md"
assert_fail "this PR's vote dir is removed" test -d "$TMP/fixed-tmp/peer-review-votes-12-abcd1234"
assert_ok "another PR's files stay" test -f "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-13.md"
new_job opened
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1; RC=$?
assert_eq "cleanup without the label still exits 0" "0" "$RC"

finish_tests
