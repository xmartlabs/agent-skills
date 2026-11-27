#!/usr/bin/env bash
# Tests for actions/tribunal/run.sh: gate, prepare, review, post, fallback and cleanup, with fake
# gh, curl, claude and tribunal binaries. Nothing touches the network.
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/helper.sh"
RUN="$HERE/../run.sh"
echo "tribunal action run_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/ghbin" "$TMP/fx" "$TMP/fixed-tmp" "$TMP/rel"

# A release archive holding a fake tribunal binary that records how it was run.
mkdir -p "$TMP/rel/tribunal-v9-x86_64-unknown-linux-musl"
cat > "$TMP/rel/tribunal-v9-x86_64-unknown-linux-musl/tribunal" <<'SH'
#!/usr/bin/env bash
sub="$1"; shift
{ echo "ARGS=$sub $*"; echo "PWD=$PWD"; echo "HOME=$HOME"; echo "TMPDIR=$TMPDIR"
  echo "DATA=$TRIBUNAL_DATA_DIR"; echo "ROOT=$TRIBUNAL_PLUGIN_ROOT"
  echo "TRIBUNAL_KEY=${TRIBUNAL_ANTHROPIC_API_KEY-<unset>}"; echo "OAUTH=${CLAUDE_CODE_OAUTH_TOKEN-<unset>}"
  echo "API_KEY=${ANTHROPIC_API_KEY-<unset>}"; echo "CODEX_HOME=$CODEX_HOME"; echo "CONVENTIONS=${TRIBUNAL_CONVENTIONS_TOKEN-<unset>}";
  echo "OPENAI_KEY=${OPENAI_API_KEY-<unset>}"; echo "CODEX_KEY=${CODEX_API_KEY-<unset>}";
  echo "ACTION_REF=${TRIBUNAL_ACTION_REF-<unset>}"; echo "REPO_DIR=${TRIBUNAL_REPO_DIR-<unset>}"; } > "$FX/tribunal-$sub.env"
if [ "$sub" = review ]; then
  echo "REPORT-BODY-ON-STDOUT"
  out=""; while [ $# -gt 0 ]; do [ "$1" = --out ] && out="$2"; shift; done
  if [ -f "$FX/run.json" ]; then cp "$FX/run.json" "$out"; else printf '{"report":"x","metrics":{"headSha":"%s"}}' "$HEAD_SHA" > "$out"; fi
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

# Fake curl: serves the tribunal archive, or gh's release and checksums, and logs the URL.
cat > "$TMP/bin/curl" <<'SH'
#!/usr/bin/env bash
dest=""; url=""
while [ $# -gt 0 ]; do case "$1" in -o) dest="$2"; shift 2 ;; -*) shift ;; *) url="$1"; shift ;; esac; done
echo "$url" >> "$FX/curl.log"
case "$url" in
  */cli/cli/*checksums.txt) cp "$GH_SUMS" "$dest" ;;
  */cli/cli/*) cp "$GH_ARCHIVE" "$dest" ;;
  *) cp "$REL_ARCHIVE" "$dest" ;;
esac
SH
# Fake npm: installs a stub claude where `npm install --prefix DIR` would.
cat > "$TMP/bin/npm" <<'SH'
#!/usr/bin/env bash
packages="$*"
prefix=""; while [ $# -gt 0 ]; do [ "$1" = --prefix ] && prefix="$2"; shift; done
mkdir -p "$prefix/node_modules/.bin" && printf '#!/usr/bin/env bash\n' > "$prefix/node_modules/.bin/claude"
chmod +x "$prefix/node_modules/.bin/claude"
cat > "$prefix/node_modules/.bin/codex" <<'CODEX'
#!/usr/bin/env bash
[ "$1 $2" = "login --with-api-key" ] || exit 1
mkdir -p "$CODEX_HOME"
cat > "$CODEX_HOME/auth.json"
CODEX
chmod +x "$prefix/node_modules/.bin/codex"
echo "npm $prefix $packages INPUT_OPENAI_API_KEY=${INPUT_OPENAI_API_KEY-<unset>}" >> "$FX/npm.log"
SH
# Fake gh: profile lookups answer from $FX/profile-<base|default|head>; the pull request's files,
# its reviews and the token's user from $FX/files.json, reviews.json and user.json through --jq;
# the pull request's commits from $FX/commits.json and a commit's combined status (latest per
# context, paged by per_page and page as GitHub pages it) from $FX/statuses-<sha>.json; a collaborator's permission from $FX/permission (missing:
# the lookup fails).
# Everything is logged.
cat > "$TMP/ghbin/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$FX/gh.log"
jq_expr=""; prev=""
for a in "$@"; do [ "$prev" = --jq ] && jq_expr="$a"; prev="$a"; done
emit() { # FIXTURE: the fixture through --jq, an empty list when it is missing
  if [ -f "$1" ]; then jq -r "${jq_expr:-.}" < "$1"; else echo '[]' | jq -r "${jq_expr:-.}"; fi
}
case "$*" in
  "--version") echo "gh version 2.103.3" ;;
  "auth setup-git") ;;
  "api repos/acme/web-app/pulls/12 --jq .head.sha") echo "${LIVE_HEAD:-$HEAD_SHA}" ;;
  "api user"*) [ -f "$FX/user.json" ] || { echo "gh: Resource not accessible by integration (HTTP 403)" >&2; exit 1; }
    emit "$FX/user.json" ;;
  "api --paginate repos/acme/web-app/pulls/12/commits --jq .[].sha")
    [ -f "$FX/commits-error" ] && { echo "gh: Server Error (HTTP 502)" >&2; exit 1; }
    emit "$FX/commits.json" ;;
  "api repos/acme/web-app/commits/"*"/status"|"api repos/acme/web-app/commits/"*"/status?"*)
    # Paged as GitHub pages it: per_page (default 30, at most 100) and page from the query string.
    sha="${2#repos/acme/web-app/commits/}"; query="${sha#*/status}"; sha="${sha%%/status*}"
    per_page="$(printf '%s' "$query" | sed -n 's/.*[?&]per_page=\([0-9]*\).*/\1/p')"; per_page="${per_page:-30}"
    [ "$per_page" -le 100 ] || per_page=100
    page="$(printf '%s' "$query" | sed -n 's/.*[?&]page=\([0-9]*\).*/\1/p')"; page="${page:-1}"
    { cat "$FX/statuses-$sha.json" 2>/dev/null || echo '[]'; } \
      | jq --argjson n "$per_page" --argjson p "$page" '{state: "pending", total_count: length, statuses: .[($p - 1) * $n:$p * $n]}' ;;
  "api repos/acme/web-app/collaborators/"*"/permission --jq .permission")
    [ -f "$FX/permission" ] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    cat "$FX/permission" ;;
  *repos/acme/web-app/pulls/12/files*) emit "$FX/files.json" ;;
  *repos/acme/web-app/pulls/12/reviews*)
    [ -f "$FX/reviews-error" ] && { echo "gh: Server Error (HTTP 502)" >&2; exit 1; }
    emit "$FX/reviews.json" ;;
  "pr comment"*)
    while [ $# -gt 0 ]; do [ "$1" = --body-file ] && cp "$2" "$FX/posted-body.md"; shift; done ;;
  "api -X POST repos/acme/web-app/issues/12/labels "*|"api -X DELETE repos/acme/web-app/issues/12/labels/needs-human"*)
    [ -f "$FX/labels-forbidden" ] && { echo "gh: Resource not accessible by integration (HTTP 403)" >&2; exit 1; }
    case "$*" in "api -X DELETE"*) [ -f "$FX/label-present" ] || { echo "gh: Label does not exist (HTTP 404)" >&2; exit 1; } ;; esac ;;
  "api -X DELETE"*) [ -f "$FX/label-present" ] || { echo "HTTP 404" >&2; exit 1; } ;;
  "api -X POST repos/acme/web-app/statuses/"*" --input "*)
    [ -f "$FX/statuses-forbidden" ] && { echo "gh: Resource not accessible by integration (HTTP 403)" >&2; exit 1; }
    printf '%s\n' "${4##*/}" > "$FX/status-sha"
    cp "$6" "$FX/status-posted.json"
    cat "$6" >> "$FX/statuses-posted.jsonl"; echo >> "$FX/statuses-posted.jsonl" ;;
  *contents/.claude/review-profile.json*)
    case "$*" in
      *"?ref=main "*|*"?ref=main") f=base ;;
      *"?ref=$HEAD_SHA"*) f=head ;;
      *) f=default ;;
    esac
    [ -f "$FX/profile-$f" ] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    case "$*" in *"Accept: application/vnd.github.raw"*)
      if [ "$f" = head ] && [ -f "$FX/rules-head" ]; then echo '{"from":"head","projectRules":".claude/review-rules.md"}'
      else echo '{"from":"'"$f"'"}'; fi ;;
      *) echo .claude/review-profile.json ;; esac ;;
  *contents/.claude/review-rules.md*) [ -f "$FX/rules-head" ] && echo "<!-- rules -->" || { echo "HTTP 404" >&2; exit 1; } ;;
  *) echo "unexpected gh call: $*" >&2; exit 1 ;;
esac
SH
chmod +x "$TMP/bin/curl" "$TMP/bin/npm" "$TMP/ghbin/gh"
# gh's own release archive holds the same fake gh, for runners that have none.
mkdir -p "$TMP/rel/gh_2.103.3_linux_amd64/bin" && cp "$TMP/ghbin/gh" "$TMP/rel/gh_2.103.3_linux_amd64/bin/gh"
tar -czf "$TMP/rel/gh_2.103.3_linux_amd64.tar.gz" -C "$TMP/rel" gh_2.103.3_linux_amd64
GH_SUM="$( (sha256sum "$TMP/rel/gh_2.103.3_linux_amd64.tar.gz" 2>/dev/null || shasum -a 256 "$TMP/rel/gh_2.103.3_linux_amd64.tar.gz") | cut -d' ' -f1)"
echo "$GH_SUM  gh_2.103.3_linux_amd64.tar.gz" > "$TMP/rel/gh-sums.txt"

export PATH="$TMP/bin:$TMP/ghbin:$PATH" FX="$TMP/fx" REL_ARCHIVE="$TMP/rel/$ASSET" \
  GH_ARCHIVE="$TMP/rel/gh_2.103.3_linux_amd64.tar.gz" GH_SUMS="$TMP/rel/gh-sums.txt" TRIBUNAL_TARGET=x86_64-unknown-linux-musl \
  TRIBUNAL_RUST_MANIFEST="$TMP/release.json" TRIBUNAL_FIXED_TMP="$TMP/fixed-tmp" GITHUB_REPOSITORY=acme/web-app \
  GITHUB_RUN_ID=7 GITHUB_SERVER_URL=https://github.com GH_TOKEN=job-secret HEAD_SHA=aaaa1111
PLUGIN_ROOT="$(cd "$HERE/../../../plugins/tribunal" && pwd)"

# new_job ACTION [LABEL] [HEAD_REPO]; DRAFT=true makes the pull request a draft, BEFORE sets the
# event's previous head (a synchronize event's `before`).
new_job() {
  rm -rf "$TMP/runner" "${FX:?}"/*; mkdir -p "$TMP/runner"; unset GITHUB_EVENT_NAME
  export RUNNER_TEMP="$TMP/runner" GITHUB_OUTPUT="$TMP/runner/out" GITHUB_PATH="$TMP/runner/path"
  export GITHUB_EVENT_PATH="$TMP/runner/event.json"
  python3 - "$GITHUB_EVENT_PATH" "$1" "${2:-}" "${3:-acme/web-app}" "${DRAFT:-false}" "${PRIVATE-true}" "${BEFORE:-}" <<'PY'
import json, sys
path, action, label, head, draft, private, before = sys.argv[1:]
e = {"action": action, "pull_request": {"number": 12, "base": {"ref": "main"}, "draft": draft == "true",
     "head": {"sha": "aaaa1111", "repo": {"full_name": head}}}}
if private: e["repository"] = {"private": private == "true"}
if label: e["label"] = {"name": label}
if before: e["before"] = before
json.dump(e, open(path, "w"))
PY
  manifest "$SHA"
}
# gate: the action's defaults unless PATHS, ONCE or DRAFTS say otherwise.
gate() {
  : > "$GITHUB_OUTPUT"
  HAS_CLAUDE_AUTH="${HAS:-true}" INPUT_LABEL=tribunal INPUT_PATHS="${PATHS:-}" INPUT_ONCE="${ONCE:-true}" \
    INPUT_DRAFTS="${DRAFTS:-false}" INPUT_HUMAN_REVIEW_CHECK="${HRC:-off}" INPUT_HUMAN_REVIEW_ON_SKIP="${ONSKIP:-carry}" \
    bash "$RUN" gate > "$TMP/gate.txt" 2>&1
  sed -n 's/^run=//p' "$GITHUB_OUTPUT"
}
files() { python3 -c 'import json,sys;print(json.dumps([{"filename": f} for f in sys.argv[1:]]))' "$@" > "$FX/files.json"; }
review_by() { # LOGIN BODY [STATE] [COMMIT]: append a review to the pull request's reviews
  python3 - "$FX/reviews.json" "$1" "$2" "${3:-COMMENTED}" "${4:-}" <<'PY'
import json, os, sys
path, login, body, state, commit = sys.argv[1:]
r = json.load(open(path)) if os.path.exists(path) else []
r.append({"id": len(r) + 1, "user": {"login": login}, "body": body, "state": state})
if commit: r[-1]["commit_id"] = commit
json.dump(r, open(path, "w"))
PY
}
FOOTER='<sub>tribunal v111 · 9 agents · 1.2M tokens · 14m</sub>'
prepare() { INPUT_REVIEWER="${1:-frontend}" bash "$RUN" prepare 2>/dev/null; }
job() { echo "$TMP/runner/tribunal"; }
args() { tr '\n' ' ' < "$(job)/profile-args"; }

# --- gate ---------------------------------------------------------------------------------------
new_job labeled tribunal;  assert_eq "the tribunal label starts a review" "true" "$(gate)"
new_job labeled bug;       assert_eq "another label does not" "false" "$(gate)"
new_job opened;            assert_eq "other events review (the caller chose them)" "true" "$(gate)"
new_job labeled tribunal someone/fork; assert_eq "fork PRs do not run" "false" "$(gate)"
new_job labeled tribunal;  assert_eq "no Claude credential: no run" "false" "$(HAS=false gate)"
new_job labeled tribunal;  gate >/dev/null
assert_fail "a label run reads nothing from the API (as before the contract)" test -s "$FX/gh.log"

# Drafts.
DRAFT=true new_job opened; assert_eq "a draft does not run" "false" "$(gate)"
DRAFT=true new_job opened; assert_eq "a draft runs with drafts: true" "true" "$(DRAFTS=true gate)"
DRAFT=true new_job labeled tribunal; assert_eq "the label forces a draft" "true" "$(gate)"

# Paths.
new_job opened; files src/app/main.ts README.md
assert_eq "a changed file matching paths runs (* crosses /)" "true" "$(PATHS='src/*' gate)"
assert_ok "the changed files come from the pull request" grep -q "repos/acme/web-app/pulls/12/files" "$FX/gh.log"
new_job opened; files README.md docs/guide.md
assert_eq "no changed file matching paths: no run" "false" "$(PATHS='src/*' gate)"
new_job opened; files src/app.test.ts
assert_eq "a file that only matches an exclusion does not count" "false" "$(PATHS=$'src/*\n!src/*.test.ts' gate)"
new_job opened; files src/app.test.ts src/app.ts
assert_eq "another file that is not excluded still runs" "true" "$(PATHS=$'src/*\n!src/*.test.ts' gate)"
new_job opened; files README.md
gate >/dev/null
assert_fail "empty paths means any file (the files are not read)" grep -q "pulls/12/files" "$FX/gh.log"

# Once.
new_job opened; review_by "github-actions[bot]" "## Peer review"$'\n\n'"$FOOTER"
assert_eq "a pull request the action already reviewed does not run again" "false" "$(gate)"
assert_ok "the reviews come from the pull request" grep -q "repos/acme/web-app/pulls/12/reviews" "$FX/gh.log"
new_job opened; review_by "github-actions[bot]" "## Peer review"$'\n\n'"$FOOTER"
assert_eq "once: false reviews again" "true" "$(ONCE=false gate)"
new_job opened; review_by dev "## Peer review"$'\n\n'"$FOOTER"
assert_eq "a review by another user does not count" "true" "$(gate)"
new_job opened; review_by "github-actions[bot]" "Looks good to me"
assert_eq "an own review without tribunal's footer does not count" "true" "$(gate)"
new_job opened; echo '{"login":"review-bot"}' > "$FX/user.json"; review_by review-bot "$FOOTER"
assert_eq "a review by the github-token's user counts" "false" "$(gate)"
new_job opened; touch "$FX/reviews-error"
: > "$GITHUB_OUTPUT"
HAS_CLAUDE_AUTH=true INPUT_LABEL=tribunal INPUT_ONCE=true bash "$RUN" gate >/dev/null 2>&1; RC=$?
assert_ok "the gate fails when it cannot list the reviews" test "$RC" -ne 0
assert_eq "and does not decide to run" "" "$(sed -n 's/^run=//p' "$GITHUB_OUTPUT")"

# The label ignores paths and once.
new_job labeled tribunal; files README.md; review_by "github-actions[bot]" "$FOOTER"
assert_eq "the label forces a review over paths and an earlier one" "true" "$(PATHS='src/*' gate)"

# --- prepare ------------------------------------------------------------------------------------
new_job labeled tribunal; touch "$FX/profile-base"
prepare; RC=$?
assert_eq "prepare exits 0" "0" "$RC"
assert_eq "the binary comes from release.json's repo, tag and asset" \
  "https://github.com/acme/tools/releases/download/tribunal-rust-v9/$ASSET" "$(head -1 "$FX/curl.log")"
assert_ok "the binary is unpacked" test -x "$(cat "$(job)/bin-path")"
assert_ok "homosapiens is staged in the job's HOME" test -f "$(job)/home/.claude/skills/homosapiens/SKILL.md"
assert_eq "base-branch profile is snapshotted" "--profile $(job)/review-profile.json " "$(args)"

new_job labeled tribunal; manifest "0000000000000000000000000000000000000000000000000000000000000000"
prepare; RC=$?
assert_ok "a checksum mismatch fails prepare" test "$RC" -ne 0
assert_fail "no binary is kept after a mismatch" test -f "$(job)/bin-path"

new_job labeled tribunal; touch "$FX/profile-default"
prepare
assert_eq "default-branch profile is snapshotted" "--profile $(job)/review-profile.json " "$(args)"

new_job labeled tribunal; touch "$FX/profile-head" "$FX/rules-head"
prepare
assert_eq "without a base profile the PR's profile and rules are used" \
  "--profile $(job)/review-profile.json --rules $(job)/review-rules.md " "$(args)"
assert_eq "the PR's profile comes from its head commit" '{"from":"head","projectRules":".claude/review-rules.md"}' "$(cat "$(job)/review-profile.json")"

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

# A PATH with the basic tools only, so no gh from the machine running the tests leaks in.
mkdir -p "$TMP/nogh"
for c in git bash sh env python3 tar gzip xz sha256sum shasum perl cut sed grep cat mkdir find cp ln chmod date head tail tr rm dirname basename uname sort wc awk touch ls xargs mktemp readlink tee; do
  p="$(command -v "$c" 2>/dev/null)" && ln -sf "$p" "$TMP/nogh/$c"
done
new_job labeled tribunal; touch "$FX/profile-base"
PATH="$TMP/bin:$TMP/nogh" prepare; RC=$?
assert_eq "prepare works on a runner without gh" "0" "$RC"
assert_ok "it installs gh from its release" grep -q "cli/cli/releases/download/v2.103.3/gh_2.103.3_linux_amd64.tar.gz" "$FX/curl.log"
assert_ok "the installed gh is the one that reads the profile" grep -q "contents/.claude/review-profile.json" "$FX/gh.log"
assert_ok "the pinned Claude CLI is always installed" grep -q "npm $(job)/tools/claude" "$FX/npm.log"

# --- review and post ----------------------------------------------------------------------------
review() { CLAUDE_CODE_OAUTH_TOKEN="${OAUTH-oauth-secret}" ANTHROPIC_API_KEY="${API_KEY-}" bash "$RUN" review >/dev/null 2>&1; }
new_job labeled tribunal; touch "$FX/profile-head"; prepare
review; RC=$?
assert_eq "review exits 0" "0" "$RC"
assert_eq "review flags" "ARGS=review --pr 12 --repo acme/web-app --no-post --out $(job)/run.json --profile $(job)/review-profile.json --no-codex --head-sha aaaa1111 --full" \
  "$(grep '^ARGS=' "$FX/tribunal-review.env")"
assert_ok "the run record is written" test -f "$(job)/run.json"
assert_ok "it runs from the job's empty work dir" grep -qx "PWD=$(job)/work" "$FX/tribunal-review.env"
assert_ok "HOME is the job's" grep -qx "HOME=$(job)/home" "$FX/tribunal-review.env"
assert_ok "TMPDIR is the job's" grep -qx "TMPDIR=$(job)/tmp" "$FX/tribunal-review.env"
assert_ok "the data dir is the job's" grep -qx "DATA=$(job)/data" "$FX/tribunal-review.env"
assert_ok "the plugin root is the action's own ref" grep -qx "ROOT=$PLUGIN_ROOT" "$FX/tribunal-review.env"
assert_ok "the OAuth token reaches the binary" grep -qx "OAUTH=oauth-secret" "$FX/tribunal-review.env"
assert_ok "no conventions token means none reaches the binary" grep -qx "CONVENTIONS=<unset>" "$FX/tribunal-review.env"
TRIBUNAL_CONVENTIONS_TOKEN='' review
assert_ok "an empty conventions token is no token" grep -qx "CONVENTIONS=<unset>" "$FX/tribunal-review.env"
assert_eq "an empty conventions token masks nothing" "0" \
  "$(TRIBUNAL_CONVENTIONS_TOKEN='' CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" review 2>/dev/null | grep -c 'add-mask')"
MASK_OUT="$(TRIBUNAL_CONVENTIONS_TOKEN=docs-secret CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" review 2>/dev/null)"
assert_ok "a conventions token is masked" grep -qx '::add-mask::docs-secret' <<< "$MASK_OUT"
assert_ok "a conventions token reaches the binary" grep -qx "CONVENTIONS=docs-secret" "$FX/tribunal-review.env"
assert_eq "a run's stdout carries it only in the mask command" "1" "$(grep -c 'docs-secret' <<< "$MASK_OUT")"
mkdir -p "$TMP/workspace/app"
review
assert_ok "without typecheck-dir no checkout is named" grep -qx "REPO_DIR=<unset>" "$FX/tribunal-review.env"
(cd "$TMP/workspace" && INPUT_TYPECHECK_DIR=app review)
assert_ok "typecheck-dir reaches the binary as an absolute path" grep -qx "REPO_DIR=$(cd "$TMP/workspace/app" && pwd)" "$FX/tribunal-review.env"
TC_OUT="$(cd "$TMP/workspace" && INPUT_TYPECHECK_DIR=missing CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" review 2>&1)"; RC=$?
assert_eq "a missing typecheck-dir does not fail the review" "0" "$RC"
assert_ok "it says the typecheck is skipped" grep -q "typecheck-dir 'missing' is not a directory" <<< "$TC_OUT"
assert_ok "and names no checkout" grep -qx "REPO_DIR=<unset>" "$FX/tribunal-review.env"
review
assert_ok "no API key without one" grep -qx "TRIBUNAL_KEY=<unset>" "$FX/tribunal-review.env"
assert_ok "git fetches use the job token" grep -qx "auth setup-git" "$FX/gh.log"

OAUTH="" API_KEY=api-secret review
assert_ok "an API key goes in as TRIBUNAL_ANTHROPIC_API_KEY" grep -qx "TRIBUNAL_KEY=api-secret" "$FX/tribunal-review.env"
assert_ok "ANTHROPIC_API_KEY itself is not passed" grep -qx "API_KEY=<unset>" "$FX/tribunal-review.env"
API_KEY=api-secret review
assert_ok "with both, the API key is passed (a binary that reads it uses it)" grep -qx "TRIBUNAL_KEY=api-secret" "$FX/tribunal-review.env"
assert_ok "with both, the OAuth token is kept (an older binary falls back to it)" grep -qx "OAUTH=oauth-secret" "$FX/tribunal-review.env"

post() { CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" post >/dev/null 2>&1; }
post; RC=$?
assert_eq "post exits 0 when the binary posts" "0" "$RC"
assert_eq "post flags" "ARGS=post --from $(job)/run.json --over-standing-review --verdict comment" \
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
assert_ok "a skipped run says why" grep -qF "**Tribunal skipped** this pull request: deployment branch." "$FX/posted-body.md"
assert_fail "a deliberate skip does not ask to retry" grep -qF "label again" "$FX/posted-body.md"
new_job labeled tribunal; mkdir -p "$(job)"; printf 'lens:1 ok\ntribunal: cannot clone acme/web-app\n' > "$(job)/review.log"
fallback
assert_ok "a crashed review quotes its last tribunal line" grep -qF "tribunal: cannot clone acme/web-app" "$FX/posted-body.md"

new_job labeled tribunal
fallback
assert_ok "a run that failed before the review says the setup failed" \
  grep -qF "the action's setup failed before the review started" "$FX/posted-body.md"

# --- node check ---------------------------------------------------------------------------------
node_check() { : > "$GITHUB_OUTPUT"; bash "$RUN" node-check >/dev/null 2>&1; sed -n 's/^missing=//p' "$GITHUB_OUTPUT"; }
new_job labeled tribunal
assert_eq "npm on the runner: no Node setup" "false" "$(node_check)"
assert_eq "no npm on the runner: Node is set up" "true" "$(PATH="$TMP/nogh" node_check)"

# --- findings check -----------------------------------------------------------------------------
findings_check() { # MODE: run the step; RC is its exit code and COUNTS its "blocking/should-fix" outputs
  : > "$GITHUB_OUTPUT"
  INPUT_FINDINGS_CHECK="$1" bash "$RUN" findings-check >/dev/null 2>&1; RC=$?
  COUNTS="$(sed -n 's/^blocking=//p' "$GITHUB_OUTPUT")/$(sed -n 's/^should-fix=//p' "$GITHUB_OUTPUT")"
}
status_field() { python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(eval(sys.argv[2],{},{"d":d}))' "$FX/status-posted.json" "$1"; }
open_run() { # one blocking and one should-fix finding on the reviewed head
  mkdir -p "$(job)"
  cat > "$(job)/run.json" <<'JSON'
{"metrics":{"headSha":"aaaa1111","reviewMode":"full","findings":[
 {"file":"src/a.ts","line":3,"title":"Skips the last item","survived":true,"blocking":true,"severity":"important"},
 {"file":"src/b.ts","line":9,"title":"Retries forever","survived":true,"blocking":false,"severity":"important","rankedBelowBlocking":true,"rankWorth":4},
 {"file":"src/c.ts","line":1,"title":"Naming","survived":true,"blocking":false,"severity":"nit"}]}}
JSON
}
new_job opened; open_run
findings_check inform
assert_eq "inform: the step outputs the open counts" "1/1" "$COUNTS"
assert_eq "the step never fails the job" "0" "$RC"
assert_eq "inform: open findings leave the status green" "success" "$(status_field "d['state']")"
assert_eq "with the counts and that they are not enforced" "1 blocking, 1 should fix open (not enforced)" "$(status_field "d['description']")"
assert_eq "the status is on the reviewed head" "aaaa1111" "$(cat "$FX/status-sha")"
assert_eq "the status is named for the findings" "Tribunal findings" "$(status_field "d['context']")"
assert_eq "it links the run" "https://github.com/acme/web-app/actions/runs/7" "$(status_field "d['target_url']")"
assert_ok "the summary gets the status's line" grep -qF "Findings status: success: 1 blocking, 1 should fix open (not enforced)" "$(job)/findings-line"
assert_ok "and the table of open findings" grep -qF '| Blocking | `src/a.ts:3` | Skips the last item | this review |' "$(job)/findings-table.md"
new_job opened; open_run
findings_check enforce
assert_eq "enforce: open findings fail the status" "failure" "$(status_field "d['state']")"
new_job opened; open_run; touch "$FX/statuses-forbidden"
findings_check inform
assert_eq "without statuses: write the step still exits 0" "0" "$RC"
assert_ok "and the summary names the missing permission" grep -qF 'Give the job `statuses: write`' "$(job)/findings-line"
new_job opened; mkdir -p "$(job)"; echo '{"report":"x","metrics":{"headSha":"aaaa1111"}}' > "$(job)/run.json"
findings_check inform
assert_eq "a run without findings to count outputs no numbers" "/" "$COUNTS"
assert_eq "and leaves an error status, never a green one" "error" "$(status_field "d['state']")"
new_job opened; mkdir -p "$(job)"; echo '{"metrics":{"headSha":"bbbb2222","findings":[]}}' > "$(job)/run.json"
findings_check inform
assert_fail "a run for another head leaves no status" test -f "$FX/status-posted.json"
assert_ok "and says why" grep -qF "reviewed HEAD differs" "$(job)/findings-line"
assert_eq "even then the step exits 0" "0" "$RC"

# --- needs-human --------------------------------------------------------------------------------
needs_human_step() { : > "$GITHUB_OUTPUT"; bash "$RUN" needs-human >/dev/null 2>&1; RC=$?; sed -n 's/^needs-human=//p' "$GITHUB_OUTPUT"; }
new_job opened; mkdir -p "$(job)"
echo '{"needsHuman":{"raised":true,"reasons":[{"signal":"panel-cut","detail":"The panel was cut"},{"signal":"lens-empty","detail":"A lens returned nothing"}]}}' > "$(job)/run.json"
assert_eq "needs_human_output_true" "true" "$(needs_human_step)"
assert_eq "the step exits 0" "0" "$RC"
assert_eq "the reasons are listed one per line" "- The panel was cut (panel-cut)
- A lens returned nothing (lens-empty)" "$(cat "$(job)/needs-human.md")"
echo '{"needsHuman":{"raised":false,"reasons":[]}}' > "$(job)/run.json"
assert_eq "not raised outputs false" "false" "$(needs_human_step)"
echo '{"verdict":"APPROVE","metrics":{}}' > "$(job)/run.json"
assert_eq "needs_human_output_empty_on_old_engine" "" "$(needs_human_step)"
assert_ok "an old engine still writes the output key, empty" grep -qx 'needs-human=' "$GITHUB_OUTPUT"
assert_fail "and no reasons file" test -f "$(job)/needs-human.md"
echo '{"skipped":true,"reason":"draft"}' > "$(job)/run.json"
assert_eq "a skipped run outputs nothing" "" "$(needs_human_step)"
rm -f "$(job)/run.json"
assert_eq "a missing run.json outputs nothing" "" "$(needs_human_step)"
assert_eq "and the step still exits 0" "0" "$RC"

# --- summary ------------------------------------------------------------------------------------
summary() { : > "$TMP/summary.md"; GITHUB_STEP_SUMMARY="$TMP/summary.md" bash "$RUN" summary >/dev/null 2>&1; }
new_job labeled tribunal; mkdir -p "$(job)/tmp"
# The binary prints the verdict in capitals and the PR URL (no review anchor), as the live runs show.
echo "Posted COMMENT: https://github.com/acme/web-app/pull/12" > "$(job)/post.log"
printf '## Peer review: PR #12\n\n### Blocking\n\nNone.\n' > "$(job)/tmp/tribunal-report-12.md"
summary; RC=$?
assert_eq "summary exits 0" "0" "$RC"
assert_ok "a posted review is marked as posted" grep -qF "### Tribunal — PR #12 · ✅ posted" "$TMP/summary.md"
assert_ok "it links the posted review" grep -qF "https://github.com/acme/web-app/pull/12" "$TMP/summary.md"
assert_ok "it includes the report" grep -qF "## Peer review: PR #12" "$TMP/summary.md"

new_job labeled tribunal; mkdir -p "$(job)"
echo '{"skipped":true,"reason":"deployment branch"}' > "$(job)/run.json"
summary
assert_ok "a skipped run is marked as skipped, with its reason" grep -qF "⏭️ skipped: deployment branch" "$TMP/summary.md"

new_job labeled tribunal; mkdir -p "$(job)"
echo "NOT POSTED: duplicate-review gate is held" > "$(job)/post.log"
summary
assert_ok "a held post is marked as not posted, with the reason" grep -qF "⏳ not posted: NOT POSTED: duplicate-review gate is held" "$TMP/summary.md"
assert_ok "without a report it says so" grep -qF "No report was written." "$TMP/summary.md"

new_job labeled tribunal; mkdir -p "$(job)/tmp"
python3 -c 'print("x" * 1100000)' > "$(job)/tmp/tribunal-report-12.md"
summary
assert_ok "a huge report is cut to fit GitHub's summary limit" test "$(wc -c < "$TMP/summary.md")" -lt 1000000
assert_ok "and says it was cut" grep -qF "Report cut to fit" "$TMP/summary.md"

new_job labeled tribunal
summary
assert_ok "a run that failed before the review says so in the summary" \
  grep -qF "⏳ not posted: the action's setup failed before the review started" "$TMP/summary.md"

new_job labeled tribunal
: > "$TMP/summary.md"; env -u GITHUB_STEP_SUMMARY bash "$RUN" summary >/dev/null 2>&1; RC=$?
assert_eq "summary without GITHUB_STEP_SUMMARY is a no-op" "0" "$RC"

# --- cleanup ------------------------------------------------------------------------------------
new_job labeled tribunal; touch "$FX/profile-base" "$FX/label-present"; prepare
touch "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-12.md" "$TMP/fixed-tmp/tribunal-conventions-acme-web-app-12.md" \
  "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-13.md"
mkdir -p "$TMP/fixed-tmp/peer-review-votes-12-aaaa1111"
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1; RC=$?
assert_eq "cleanup exits 0" "0" "$RC"
assert_ok "the label is removed" grep -q "api -X DELETE repos/acme/web-app/issues/12/labels/tribunal" "$FX/gh.log"
assert_fail "the job directory is removed" test -d "$(job)"
assert_fail "this PR's methodology file is removed" test -f "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-12.md"
assert_fail "this PR's conventions file is removed" test -f "$TMP/fixed-tmp/tribunal-conventions-acme-web-app-12.md"
assert_fail "this PR's vote dir is removed" test -d "$TMP/fixed-tmp/peer-review-votes-12-aaaa1111"
assert_ok "another PR's files stay" test -f "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-13.md"
new_job opened
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1; RC=$?
assert_eq "cleanup without the label still exits 0" "0" "$RC"

new_job labeled tribunal
touch "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-12.md" "$TMP/fixed-tmp/tribunal-conventions-acme-web-app-12.md"
GITHUB_REPOSITORY=acme/web_app.v2 INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1
touch "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-v2-12.md"
GITHUB_REPOSITORY=acme/web_app.v2 INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1
assert_fail "the slug follows the binary's (non-alphanumeric runs become one -)" test -f "$TMP/fixed-tmp/tribunal-methodology-acme-web-app-v2-12.md"

new_job labeled tribunal
mkdir -p "$TMP/fixed-tmp/peer-review-votes-12-aaaa1111" "$TMP/fixed-tmp/peer-review-votes-12-bbbb2222"
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1
assert_fail "this run's vote dir is removed" test -d "$TMP/fixed-tmp/peer-review-votes-12-aaaa1111"
assert_ok "a vote dir of another head (another repo's PR 12) stays" test -d "$TMP/fixed-tmp/peer-review-votes-12-bbbb2222"

new_job labeled "needs review"; touch "$FX/label-present"
INPUT_LABEL="needs review" bash "$RUN" cleanup >/dev/null 2>&1
assert_ok "a label with a space is URL-encoded" grep -q "labels/needs%20review" "$FX/gh.log"

# --- structured configuration, races and incremental fallback -----------------------------------
for kind in python e2e; do
  new_job labeled tribunal
  prepare "$kind"
  assert_eq "$kind generic profile" "\"$kind\"" "$(cfg reviewer)"
done

new_job labeled tribunal
INPUT_MODEL=claude-model INPUT_EFFORT=high INPUT_CONCURRENCY=3 INPUT_MAX_PANELS=12 INPUT_MAX_AGENTS=30 INPUT_MAX_OUTPUT_TOKENS=2000 INPUT_PRIOR_VERIFY=2 INPUT_LENS_SAMPLES=2 INPUT_BOT_COMMENTS=true INPUT_VERDICT=suggested prepare
review
for flag in '--model claude-model' '--effort high' '--concurrency 3' '--max-panels 12' '--max-agents 30' '--max-output-tokens 2000' '--prior-verify 2' '--lens-samples 2' '--bot-comments'; do
  assert_ok "structured $flag reaches review" grep -qF -- "$flag" "$FX/tribunal-review.env"
done
post
assert_fail "suggested verdict does not override engine verdict" grep -qF -- '--verdict' "$FX/tribunal-post.env"
assert_ok "model reaches report composition" grep -qF -- '--model claude-model --effort high' "$FX/tribunal-post.env"

# no-approve: a change request when the engine asks for one, a comment otherwise, never an approval.
no_approve_post() { # ENGINE_VERDICT
  new_job labeled tribunal; INPUT_VERDICT=no-approve prepare
  printf '{"report":"x","verdict":"%s","metrics":{"headSha":"aaaa1111"}}' "$1" > "$FX/run.json"
  review >/dev/null 2>&1
  review_by 'github-actions[bot]' "Blocking. $FOOTER" CHANGES_REQUESTED
  review_by dev "Please fix" CHANGES_REQUESTED
  review_by 'github-actions[bot]' "Earlier. $FOOTER" COMMENTED
  post; RC=$?
}
no_approve_post APPROVE
assert_eq "no-approve: the post succeeds" "0" "$RC"
assert_ok "an engine APPROVE is posted as a comment" grep -qF -- '--verdict comment' "$FX/tribunal-post.env"
assert_ok "and the action's earlier change request is dismissed" \
  grep -qF "api -X PUT repos/acme/web-app/pulls/12/reviews/1/dismissals" "$FX/gh.log"
assert_fail "a person's change request is not" grep -qF "reviews/2/dismissals" "$FX/gh.log"
assert_fail "nor the action's earlier comment" grep -qF "reviews/3/dismissals" "$FX/gh.log"
no_approve_post "REQUEST CHANGES"
assert_ok "an engine change request is posted as one" grep -qF -- '--verdict request-changes' "$FX/tribunal-post.env"
assert_fail "and dismisses nothing" grep -qF "dismissals" "$FX/gh.log"
no_approve_post COMMENT
assert_ok "an engine COMMENT is posted as a comment" grep -qF -- '--verdict comment' "$FX/tribunal-post.env"
new_job labeled tribunal; prepare
printf '{"report":"x","verdict":"APPROVE","metrics":{"headSha":"aaaa1111"}}' > "$FX/run.json"
review >/dev/null 2>&1
review_by 'github-actions[bot]' "Blocking. $FOOTER" CHANGES_REQUESTED
post
assert_fail "the default comment verdict dismisses nothing" grep -qF "dismissals" "$FX/gh.log"

new_job labeled tribunal; prepare
LIVE_HEAD=bbbb2222 review; RC=$?
assert_ok "moved head before review fails" test "$RC" -ne 0
assert_fail "moved head never dispatches the engine" test -f "$FX/tribunal-review.env"
assert_ok "moved head reason reaches fallback log" grep -qF 'PR head moved before review' "$(job)/review.log"

new_job labeled tribunal; prepare
echo '{"report":"x","metrics":{"headSha":"bbbb2222"}}' > "$FX/run.json"
review; RC=$?
assert_ok "race while resolving head fails the post-review check" test "$RC" -ne 0
post; RC=$?
assert_ok "mismatched run is not posted" test "$RC" -ne 0
assert_fail "mismatched run never invokes engine post" test -f "$FX/tribunal-post.env"

new_job labeled tribunal; INPUT_REVIEW_MODE=incremental prepare
bash "$RUN" restore >/dev/null 2>&1
assert_eq "no verified baseline forces full review" full "$(cat "$(job)/effective-mode")"
review
assert_ok "fallback really passes --full (no human baseline)" grep -qF -- '--full' "$FX/tribunal-review.env"
# Restore provenance is tested independently with real ZIPs in test_runtime.py.
echo incremental > "$(job)/effective-mode"
review
assert_fail "incremental does not disable the probe with --head-sha" grep -qF -- '--head-sha' "$FX/tribunal-review.env"
assert_fail "incremental does not force full" grep -qF -- '--full' "$FX/tribunal-review.env"

new_job labeled tribunal; prepare
bash "$RUN" baseline >/dev/null 2>&1
assert_ok "unposted runs cannot upload a baseline" grep -qF 'saved=false' "$GITHUB_OUTPUT"
assert_fail "unposted runs leave no baseline envelope" test -f "$(job)/baseline/baseline.json"

# Codex setup is explicit, isolated, version-pinned, and never uploaded.
new_job labeled tribunal
INPUT_CODEX=true INPUT_OPENAI_API_KEY=fixture-openai-key INPUT_CODEX_MODEL=codex-model prepare
assert_eq "Codex login uses only the supplied key" fixture-openai-key "$(cat "$(job)/home/.codex/auth.json")"
assert_ok "Codex CLI is pinned" grep -qF '@openai/codex@0.165.0' "$FX/npm.log"
assert_fail "dependency installs cannot read the OpenAI key" grep -qF fixture-openai-key "$FX/npm.log"
assert_eq "Codex model is isolated" 'model = "codex-model"' "$(cat "$(job)/home/.codex/review.config.toml")"
OPENAI_API_KEY=ambient-key CODEX_API_KEY=ambient-key CODEX_HOME=/wrong review
assert_ok "Codex flag reaches the engine" grep -qF -- '--codex' "$FX/tribunal-review.env"
assert_ok "Codex auth home is isolated" grep -qx "CODEX_HOME=$(job)/home/.codex" "$FX/tribunal-review.env"
assert_ok "ambient OpenAI key is removed" grep -qx 'OPENAI_KEY=<unset>' "$FX/tribunal-review.env"
assert_ok "ambient Codex key is removed" grep -qx 'CODEX_KEY=<unset>' "$FX/tribunal-review.env"
# Each stage reopens the immutable selection, rather than reverting to bundled files.
mkdir -p "$(job)/selected-plugin"
printf '%s' "$(job)/selected-plugin" > "$(job)/plugin-path"
review
assert_ok "review uses the selected plugin root" grep -qx "ROOT=$(job)/selected-plugin" "$FX/tribunal-review.env"
rm -f "$FX/tribunal-review.env"
# A private repository keeps the review's output in the job log; a public one, or an event that does not
# say, keeps it in the file only and marks the gate's output public.
CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" review > "$TMP/console.txt" 2>&1
assert_ok "a private repository's job log shows the review output" grep -qF REPORT-BODY-ON-STDOUT "$TMP/console.txt"
# The engine records the action's ref (actionRef), read from where GitHub checked the action out.
GITHUB_ACTION_PATH=/runner/_work/_actions/xmartlabs/agent-skills/v1.4.3/actions/tribunal review
assert_ok "action ref reaches the engine" grep -qx 'ACTION_REF=xmartlabs/agent-skills@v1.4.3' "$FX/tribunal-review.env"
GITHUB_ACTION_PATH=/runner/_work/_actions/acme/tools/feat/x/actions/tribunal review
assert_ok "a branch ref keeps its slash" grep -qx 'ACTION_REF=acme/tools@feat/x' "$FX/tribunal-review.env"
GITHUB_ACTION_PATH=/runner/_work/repo/repo/./actions/tribunal review
assert_ok "a local action records no ref" grep -qx 'ACTION_REF=' "$FX/tribunal-review.env"
INPUT_LABEL=tribunal bash "$RUN" cleanup >/dev/null 2>&1
assert_fail "cleanup removes Codex authentication" test -f "$(job)/home/.codex/auth.json"

PRIVATE=true new_job labeled tribunal; gate >/dev/null 2>&1
assert_ok "the gate marks a private repository" grep -qx 'public=false' "$GITHUB_OUTPUT"
for vis in false ""; do
  PRIVATE="$vis" new_job labeled tribunal; touch "$FX/profile-head"; gate >/dev/null 2>&1; prepare
  assert_ok "the gate marks a repository with private=${vis:-<missing>} public" grep -qx 'public=true' "$GITHUB_OUTPUT"
  CLAUDE_CODE_OAUTH_TOKEN=oauth-secret bash "$RUN" review > "$TMP/console.txt" 2>&1
  assert_fail "a public repository's job log hides the review output" grep -qF REPORT-BODY-ON-STDOUT "$TMP/console.txt"
  assert_ok "the review output is still kept in the file" grep -qF REPORT-BODY-ON-STDOUT "$(job)/review.log"
done

# --- human review status ----------------------------------------------------------------------------
human_review_check() { : > "$GITHUB_OUTPUT"; INPUT_HUMAN_REVIEW_CHECK=on bash "$RUN" human-review-check >"$TMP/hr.txt" 2>&1; RC=$?; }
new_job opened; mkdir -p "$(job)"
echo '{"metrics":{"headSha":"aaaa1111"},"needsHuman":{"raised":true,"reasons":[{"signal":"panel-cut","detail":"The panel was cut"}]}}' > "$(job)/run.json"
human_review_check
assert_eq "human_review_check_posts_pending: the context" "Tribunal human review" "$(status_field "d['context']")"
assert_eq "human_review_check_posts_pending: the state" "pending" "$(status_field "d['state']")"
assert_eq "human_review_check_posts_pending: the description" "A person should review: panel-cut" "$(status_field "d['description']")"
assert_eq "the status is on the reviewed head" "aaaa1111" "$(cat "$FX/status-sha")"
assert_eq "the step exits 0" "0" "$RC"
new_job opened; mkdir -p "$(job)"
echo '{"metrics":{"headSha":"aaaa1111"},"needsHuman":{"raised":true,"reasons":[{"signal":"panel-cut","detail":"x"}]}}' > "$(job)/run.json"
INPUT_FINDINGS_CHECK=inform bash "$RUN" findings-check >/dev/null 2>&1; human_review_check
assert_eq "both statuses on one commit are told apart" "Tribunal findings
Tribunal human review" "$(python3 -c 'import json,sys;[print(json.loads(l)["context"]) for l in open(sys.argv[1]) if l.strip()]' "$FX/statuses-posted.jsonl")"
new_job opened; mkdir -p "$(job)"
echo '{"metrics":{"headSha":"bbbb2222"},"needsHuman":{"raised":true,"reasons":[]}}' > "$(job)/run.json"
human_review_check
assert_fail "a moved head posts no human review status" test -f "$FX/status-posted.json"
assert_eq "even then the step exits 0" "0" "$RC"
# An approval given while the review ran still counts when the review raises the flag.
RAISED='{"metrics":{"headSha":"aaaa1111"},"needsHuman":{"raised":true,"reasons":[{"signal":"panel-cut","detail":"x"}]}}'
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"
review_by dev "LGTM" APPROVED aaaa1111; echo write > "$FX/permission"
human_review_check
assert_eq "earlier_approval_clears_a_raised_flag" "success: Cleared by @dev's approval" "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"
assert_ok "earlier_approval_clears_a_raised_flag: the permission is looked up" grep -qF "api repos/acme/web-app/collaborators/dev/permission --jq .permission" "$FX/gh.log"
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"
review_by dev "LGTM" APPROVED 9999eeee0000; echo write > "$FX/permission"
human_review_check
assert_eq "an earlier approval of another commit leaves it pending" "pending" "$(status_field "d['state']" 2>/dev/null)"
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"
review_by dev "LGTM" APPROVED aaaa1111; echo read > "$FX/permission"
human_review_check
assert_eq "an earlier approval by a reader leaves it pending" "pending" "$(status_field "d['state']" 2>/dev/null)"
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"
review_by dev "LGTM" APPROVED aaaa1111; review_by dev "Wait" CHANGES_REQUESTED aaaa1111; echo write > "$FX/permission"
human_review_check
assert_eq "an approval its author took back leaves it pending" "pending" "$(status_field "d['state']" 2>/dev/null)"
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"
review_by "github-actions[bot]" "LGTM" APPROVED aaaa1111; echo write > "$FX/permission"
human_review_check
assert_eq "the action's own approval leaves it pending" "pending" "$(status_field "d['state']" 2>/dev/null)"
new_job opened; mkdir -p "$(job)"; echo "$RAISED" > "$(job)/run.json"; touch "$FX/reviews-error"
human_review_check
assert_eq "reviews that cannot be listed leave it pending" "pending" "$(status_field "d['state']" 2>/dev/null)"
new_job opened; mkdir -p "$(job)"; echo '{"skipped":true,"reason":"x"}' > "$(job)/run.json"
human_review_check
assert_fail "an engine skip posts no reviewed-commit status" test -f "$FX/status-posted.json"
assert_ok "an engine skip says so, not that the head moved" grep -qF "the engine skipped the review" "$TMP/hr.txt"
assert_fail "an engine skip does not blame the head" grep -qF "reviewed HEAD differs" "$TMP/hr.txt"
new_job opened; mkdir -p "$(job)"; touch "$FX/statuses-forbidden"
echo '{"metrics":{"headSha":"aaaa1111"},"needsHuman":{"raised":false,"reasons":[]}}' > "$(job)/run.json"
human_review_check
assert_eq "human_review_check_forbidden_only_warns: the job rc is 0" "0" "$RC"
assert_ok "human_review_check_forbidden_only_warns: the warning" grep -qF '::warning::tribunal: the human review status was not created; give the job statuses: write' "$TMP/hr.txt"

# --- human review status on skipped commits ----------------------------------------------------------
commits() { python3 -c 'import json,sys;print(json.dumps([{"sha": s} for s in sys.argv[1:]]))' "$@" > "$FX/commits.json"; }
statuses_of() { # SHA STATE DESCRIPTION [CONTEXT]: append a status to a commit's statuses (newest first)
  python3 - "$FX/statuses-$1.json" "$2" "$3" "${4:-Tribunal human review}" <<'PY2'
import json, os, sys
path, state, description, context = sys.argv[1:]
s = json.load(open(path)) if os.path.exists(path) else []
json.dump([{"context": context, "state": state, "description": description}] + s, open(path, "w"))
PY2
}
posted_count() { [ -f "$FX/statuses-posted.jsonl" ] && grep -c . "$FX/statuses-posted.jsonl" || echo 0; }

new_job synchronize; review_by "github-actions[bot]" "## Peer review"$'\n\n'"$FOOTER"
commits 1111aaaa2222 2222bbbb3333 aaaa1111
statuses_of 1111aaaa2222 success "No reason for a human review"
statuses_of 1111aaaa2222 pending "A person should review: panel-cut"
statuses_of 2222bbbb3333 success "No blocking or should-fix findings open" "Tribunal findings"
assert_eq "skip_status_carry_from_gate: the gate says no (once)" "false" "$(HRC=on gate)"
assert_eq "skip_status_carry_from_gate: the status is on the new head" "aaaa1111" "$(cat "$FX/status-sha" 2>/dev/null)"
assert_eq "skip_status_carry_from_gate: the context" "Tribunal human review" "$(status_field "d['context']" 2>/dev/null)"
assert_eq "skip_status_carry_from_gate: the earlier pending state is carried" "pending" "$(status_field "d['state']" 2>/dev/null)"
assert_eq "skip_status_carry_from_gate: the description names the commit it came from" \
  "Carried from 1111aaa: A person should review: panel-cut" "$(status_field "d['description']" 2>/dev/null)"
assert_eq "skip_status_carry_from_gate: it links the run" "https://github.com/acme/web-app/actions/runs/7" "$(status_field "d['target_url']" 2>/dev/null)"

new_job synchronize; review_by "github-actions[bot]" "$FOOTER"; commits aaaa1111
HRC=on gate >/dev/null
assert_eq "carry without an earlier status posts success" "success: Not reviewed by Tribunal" \
  "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"

DRAFT=true new_job opened
assert_eq "skip_status_success: a draft is not reviewed" "false" "$(HRC=on ONSKIP=success gate)"
assert_eq "skip_status_success: success, not reviewed" "success: Not reviewed by Tribunal" \
  "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"
assert_fail "skip_status_success: no history is read" grep -qsF "pulls/12/commits" "$FX/gh.log"

new_job synchronize
assert_eq "skip_status_pending: no Claude credential" "false" "$(HAS=false HRC=on ONSKIP=pending gate)"
assert_eq "skip_status_pending: pending, a person should review" "pending: Not reviewed by Tribunal; a person should review" \
  "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"

new_job synchronize "" someone/fork
HRC=on ONSKIP=pending gate >/dev/null
assert_eq "skip_status_not_posted_for_fork" "0" "$(posted_count)"
assert_fail "a fork makes no API call" test -s "$FX/gh.log"

new_job opened; mkdir -p "$(job)"; echo '{"skipped":true,"reason":"deployment pull request"}' > "$(job)/run.json"
INPUT_HUMAN_REVIEW_CHECK=on INPUT_HUMAN_REVIEW_ON_SKIP=pending bash "$RUN" human-review-skip > "$TMP/hs.txt" 2>&1; RC=$?
assert_eq "skip_status_on_engine_skip: the state" "pending" "$(status_field "d['state']" 2>/dev/null)"
assert_eq "skip_status_on_engine_skip: on the event head" "aaaa1111" "$(cat "$FX/status-sha" 2>/dev/null)"
assert_eq "skip_status_on_engine_skip: the step exits 0" "0" "$RC"
new_job opened; mkdir -p "$(job)"; echo '{"metrics":{"headSha":"aaaa1111"},"needsHuman":{"raised":false,"reasons":[]}}' > "$(job)/run.json"
INPUT_HUMAN_REVIEW_CHECK=on INPUT_HUMAN_REVIEW_ON_SKIP=pending bash "$RUN" human-review-skip >/dev/null 2>&1
assert_eq "a reviewed run gets no skip status" "0" "$(posted_count)"
new_job opened; mkdir -p "$(job)"; echo '{"skipped":true,"reason":"x"}' > "$(job)/run.json"
INPUT_HUMAN_REVIEW_CHECK=off bash "$RUN" human-review-skip >/dev/null 2>&1
assert_fail "human-review-check off: no call on an engine skip" test -s "$FX/gh.log"

BEFORE=0000dead1111 new_job synchronize; review_by "github-actions[bot]" "$FOOTER"
commits 1111aaaa2222 aaaa1111
statuses_of 1111aaaa2222 success "No reason for a human review"
statuses_of 0000dead1111 pending "A person should review: panel-cut"
HRC=on gate >/dev/null
assert_eq "carry_from_before_after_force_push: the previous head's pending status is carried" \
  "pending: Carried from 0000dea: A person should review: panel-cut" "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"
assert_ok "carry_from_before_after_force_push: read through the combined status" grep -qF "api repos/acme/web-app/commits/0000dead1111/status" "$FX/gh.log"

new_job synchronize; review_by "github-actions[bot]" "Earlier. $FOOTER" COMMENTED 9999eeee0000
review_by dev "Looks fine" COMMENTED 1111aaaa2222
review_by "github-actions[bot]" "Latest. $FOOTER" COMMENTED 2222bbbb3333
commits 1111aaaa2222 aaaa1111
statuses_of 1111aaaa2222 success "No reason for a human review"
HRC=on gate >/dev/null
assert_eq "carry_from_latest_own_review: a reviewed commit without a status yet carries pending" \
  "pending: Reviewed at 2222bbb; human review status not posted yet" "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"
new_job synchronize; review_by "github-actions[bot]" "Latest. $FOOTER" COMMENTED 2222bbbb3333
statuses_of 2222bbbb3333 pending "A person should review: lens-empty"
HRC=on ONSKIP=carry gate >/dev/null
assert_eq "carry_from_latest_own_review: its status is carried" \
  "pending: Carried from 2222bbb: A person should review: lens-empty" "$(status_field "d['state']+': '+d['description']" 2>/dev/null)"

new_job edited; review_by "github-actions[bot]" "$FOOTER" COMMENTED aaaa1111
statuses_of aaaa1111 pending "A person should review: panel-cut"
HRC=on ONSKIP=success gate >/dev/null
assert_eq "head_status_kept_via_combined_status" "0" "$(posted_count)"
assert_ok "head_status_kept_via_combined_status: the combined endpoint is read" grep -qF "api repos/acme/web-app/commits/aaaa1111/status" "$FX/gh.log"

new_job edited; review_by "github-actions[bot]" "$FOOTER" COMMENTED aaaa1111
for i in $(seq 1 130); do statuses_of aaaa1111 success "check $i" "ci/check-$i"; done
python3 - "$FX/statuses-aaaa1111.json" <<'PY2'
import json, sys
p = sys.argv[1]; s = json.load(open(p))
s.insert(120, {"context": "Tribunal human review", "state": "pending", "description": "A person should review: panel-cut"})
json.dump(s, open(p, "w"))
PY2
HRC=on ONSKIP=success gate >/dev/null
assert_eq "combined_status_paged: a head status on the second page is kept" "0" "$(posted_count)"
assert_ok "combined_status_paged: the second page is read" grep -qF "api repos/acme/web-app/commits/aaaa1111/status?per_page=100&page=2" "$FX/gh.log"

new_job labeled bug
HRC=on ONSKIP=pending gate >/dev/null
assert_eq "another label (same head) posts no skip status" "0" "$(posted_count)"
DRAFT=true new_job opened
gate >/dev/null
assert_fail "human-review-check off: a skip makes no call" test -s "$FX/gh.log"
DRAFT=true new_job ready_for_review; statuses_of aaaa1111 pending "A person should review: panel-cut"
HRC=on ONSKIP=success gate >/dev/null
assert_eq "a head that already has a human review status keeps it" "0" "$(posted_count)"
DRAFT=true new_job opened; touch "$FX/statuses-forbidden"
HRC=on ONSKIP=success gate >/dev/null; RC=$?
assert_ok "skip status refused: the gate still decides" grep -qx 'run=false' "$GITHUB_OUTPUT"
assert_ok "skip status refused: the warning" grep -qF '::warning::tribunal: the human review status was not created; give the job statuses: write' "$TMP/gate.txt"
new_job synchronize; review_by "github-actions[bot]" "$FOOTER"; touch "$FX/commits-error"
HRC=on gate >/dev/null
assert_eq "commits that cannot be listed post nothing" "0" "$(posted_count)"
assert_ok "and warn" grep -qF '::warning::tribunal: the human review status was not created' "$TMP/gate.txt"
assert_eq "and the gate still says no" "false" "$(sed -n 's/^run=//p' "$GITHUB_OUTPUT")"

# --- needs-human label ----------------------------------------------------------------------------
needs_human_label() { # VALUE: run the step; RC is its exit code, the output is in $TMP/nh.txt
  INPUT_NEEDS_HUMAN_LABEL=needs-human NEEDS_HUMAN="$1" bash "$RUN" needs-human-label > "$TMP/nh.txt" 2>&1; RC=$?
}
new_job opened; needs_human_label true
assert_eq "needs_human_label_added_when_raised" "api -X POST repos/acme/web-app/issues/12/labels -f labels[]=needs-human" "$(cat "$FX/gh.log")"
assert_eq "the step exits 0" "0" "$RC"
new_job opened; touch "$FX/label-present"; needs_human_label false
assert_eq "needs_human_label_removed_when_quiet" "api -X DELETE repos/acme/web-app/issues/12/labels/needs-human" "$(cat "$FX/gh.log")"
new_job opened; needs_human_label false
assert_eq "a label that is not there (404) is success" "0" "$RC"
assert_fail "and gives no warning" grep -qF '::warning::' "$TMP/nh.txt"
new_job opened; needs_human_label ""
assert_fail "needs_human_label_untouched_on_old_engine" test -s "$FX/gh.log"
new_job opened; INPUT_NEEDS_HUMAN_LABEL= NEEDS_HUMAN=true bash "$RUN" needs-human-label >/dev/null 2>&1
assert_fail "an empty label input makes no call" test -s "$FX/gh.log"
new_job opened; touch "$FX/labels-forbidden"; needs_human_label true
assert_eq "needs_human_label_failure_only_warns: the job rc is 0" "0" "$RC"
assert_ok "needs_human_label_failure_only_warns: the warning is logged" grep -qF '::warning::tribunal: the needs-human label was not updated; give the job pull-requests: write' "$TMP/nh.txt"
new_job opened; touch "$FX/labels-forbidden" "$FX/label-present"; needs_human_label false
assert_ok "a refused removal warns too" grep -qF '::warning::tribunal: the needs-human label was not updated' "$TMP/nh.txt"

# --- approval override (pull_request_review) -------------------------------------------------------
# new_review_event STATE USER_TYPE COMMIT PERMISSION: a pull_request_review event on PR 12 (head
# aaaa1111) by `dev`; an empty PERMISSION makes the permission lookup fail.
new_review_event() {
  new_job submitted
  export GITHUB_EVENT_NAME=pull_request_review
  python3 - "$GITHUB_EVENT_PATH" "$1" "$2" "$3" <<'PY'
import json, sys
path, state, user_type, commit = sys.argv[1:]
e = {"action": "submitted", "repository": {"private": True},
     "review": {"state": state, "commit_id": commit, "user": {"login": "dev", "type": user_type}},
     "pull_request": {"number": 12, "base": {"ref": "main"}, "draft": False,
                      "head": {"sha": "aaaa1111", "repo": {"full_name": "acme/web-app"}}}}
json.dump(e, open(path, "w"))
PY
  [ -z "$4" ] || printf '%s\n' "$4" > "$FX/permission"
}
approve() { bash "$RUN" human-review-approve > "$TMP/ap.txt" 2>&1; RC=$?; }

for perm in write maintain admin; do
  new_review_event approved User aaaa1111 "$perm"
  statuses_of aaaa1111 pending "A person should review: panel-cut"
  approve
  assert_eq "approval_clears_pending ($perm): success" "success" "$(status_field "d['state']" 2>/dev/null)"
done
assert_eq "approval_clears_pending: the description" "Cleared by @dev's approval" "$(status_field "d['description']" 2>/dev/null)"
assert_eq "approval_clears_pending: the context" "Tribunal human review" "$(status_field "d['context']" 2>/dev/null)"
assert_eq "approval_clears_pending: on the head" "aaaa1111" "$(cat "$FX/status-sha" 2>/dev/null)"
assert_eq "approval_clears_pending: it links the run" "https://github.com/acme/web-app/actions/runs/7" "$(status_field "d['target_url']" 2>/dev/null)"
assert_eq "approval_clears_pending: the step exits 0" "0" "$RC"
assert_ok "approval_clears_pending: the permission is looked up" grep -qF "api repos/acme/web-app/collaborators/dev/permission --jq .permission" "$FX/gh.log"

new_review_event approved Bot aaaa1111 write; statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
assert_eq "approval_by_bot_is_ignored" "0" "$(posted_count)"
assert_ok "approval_by_bot_is_ignored: says why" grep -qi "bot" "$TMP/ap.txt"
assert_eq "approval_by_bot_is_ignored: the step exits 0" "0" "$RC"

new_review_event approved User 9999eeee0000 write; statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
assert_eq "approval_of_old_commit_is_ignored" "0" "$(posted_count)"
assert_ok "approval_of_old_commit_is_ignored: says why" grep -qF "9999eee" "$TMP/ap.txt"
assert_eq "approval_of_old_commit_is_ignored: the step exits 0" "0" "$RC"

for perm in read triage none; do
  new_review_event approved User aaaa1111 "$perm"; statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
  assert_eq "approval_by_reader_is_ignored ($perm)" "0" "$(posted_count)"
done
assert_ok "approval_by_reader_is_ignored: says why" grep -qF "none" "$TMP/ap.txt"
new_review_event approved User aaaa1111 ""; statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
assert_eq "a permission that cannot be read counts as no write access" "0" "$(posted_count)"
assert_eq "even then the step exits 0" "0" "$RC"
new_review_event approved User aaaa1111 write; statuses_of aaaa1111 pending "A person should review: panel-cut"
echo '{"login":"dev"}' > "$FX/user.json"; approve
assert_eq "an approval by the action's own identity is ignored" "0" "$(posted_count)"

new_review_event approved User aaaa1111 write; statuses_of aaaa1111 success "No reason for a human review"; approve
assert_eq "approval_without_pending_status_posts_nothing (success)" "0" "$(posted_count)"
new_review_event approved User aaaa1111 write; approve
assert_eq "approval_without_pending_status_posts_nothing (no status)" "0" "$(posted_count)"
assert_eq "approval_without_pending_status_posts_nothing: the step exits 0" "0" "$RC"
new_review_event approved User aaaa1111 write
statuses_of aaaa1111 pending "A person should review: panel-cut" "Tribunal findings"
statuses_of aaaa1111 success "No reason for a human review"; approve
assert_eq "another context's pending status does not count" "0" "$(posted_count)"

new_review_event approved User aaaa1111 write; statuses_of aaaa1111 pending "A person should review: panel-cut"
python3 -c 'import json,sys;p=sys.argv[1];e=json.load(open(p));e["action"]="edited";json.dump(e,open(p,"w"))' "$GITHUB_EVENT_PATH"
approve
assert_eq "approval_edited_event_is_ignored" "0" "$(posted_count)"
assert_ok "approval_edited_event_is_ignored: says why" grep -qF "the review event is edited, not submitted" "$TMP/ap.txt"
assert_eq "approval_edited_event_is_ignored: the step exits 0" "0" "$RC"

new_review_event approved User aaaa1111 write
for i in $(seq 1 40); do statuses_of aaaa1111 success "check $i" "ci/check-$i"; done
python3 - "$FX/statuses-aaaa1111.json" <<'PY2'
import json, sys
p = sys.argv[1]; s = json.load(open(p))
s.append({"context": "Tribunal human review", "state": "pending", "description": "A person should review: panel-cut"})
json.dump(s, open(p, "w"))
PY2
approve
assert_eq "approval_finds_the_status_past_the_first_30_contexts" "success" "$(status_field "d['state']" 2>/dev/null)"
assert_ok "approval_finds_the_status_past_the_first_30_contexts: asks for 100 a page" grep -qF "api repos/acme/web-app/commits/aaaa1111/status?per_page=100&page=1" "$FX/gh.log"

for state in commented changes_requested dismissed; do
  new_review_event "$state" User aaaa1111 write; statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
  assert_eq "comment_review_is_ignored ($state)" "0" "$(posted_count)"
done
assert_eq "comment_review_is_ignored: the step exits 0" "0" "$RC"

new_review_event approved User aaaa1111 write; touch "$FX/statuses-forbidden"
statuses_of aaaa1111 pending "A person should review: panel-cut"; approve
assert_eq "a refused approval status only warns: rc 0" "0" "$RC"
assert_ok "a refused approval status only warns: the warning" grep -qF '::warning::tribunal: the human review status was not created; give the job statuses: write' "$TMP/ap.txt"

new_review_event approved User aaaa1111 write; statuses_of aaaa1111 pending "A person should review: panel-cut"
assert_eq "review_event_never_runs_a_review" "false" "$(HRC=on ONSKIP=pending gate)"
assert_ok "review_event_never_runs_a_review: approval=true with human-review-check on" grep -qx 'approval=true' "$GITHUB_OUTPUT"
assert_eq "review_event_never_runs_a_review: no skip status" "0" "$(posted_count)"
assert_fail "review_event_never_runs_a_review: the gate makes no API call" test -s "$FX/gh.log"
new_review_event approved User aaaa1111 write
assert_eq "a review event without a Claude credential: still no review" "false" "$(HAS=false HRC=on ONSKIP=pending gate)"
assert_ok "and still approval=true (the override needs no credential)" grep -qx 'approval=true' "$GITHUB_OUTPUT"
assert_eq "and no skip status" "0" "$(posted_count)"
new_review_event approved User aaaa1111 write
assert_eq "review_event_with_check_off_follows_the_old_rules: not reviewed yet, it reviews" "true" "$(gate)"
assert_ok "review_event_with_check_off_follows_the_old_rules: decided by the event's action" grep -qF "gate: submitted" "$TMP/gate.txt"
assert_fail "review_event_with_check_off_follows_the_old_rules: no approval output" grep -q '^approval=' "$GITHUB_OUTPUT"
new_review_event approved User aaaa1111 write; review_by "github-actions[bot]" "$FOOTER"
assert_eq "review_event_with_check_off_follows_the_old_rules: once still skips" "false" "$(gate)"
assert_ok "review_event_with_check_off_follows_the_old_rules: says already reviewed" grep -qF "already reviewed (once: true)" "$TMP/gate.txt"
assert_eq "review_event_with_check_off_follows_the_old_rules: no status" "0" "$(posted_count)"
new_review_event approved User aaaa1111 write
assert_eq "review_event_with_check_off_follows_the_old_rules: no credential skips" "false" "$(HAS=false gate)"
new_review_event approved User aaaa1111 write
python3 -c 'import json,sys;p=sys.argv[1];e=json.load(open(p));e["pull_request"]["head"]["repo"]["full_name"]="someone/fork";json.dump(e,open(p,"w"))' "$GITHUB_EVENT_PATH"
assert_eq "a fork's review event does not run" "false" "$(HRC=on gate)"
assert_fail "a fork's review event gets no approval output" grep -q '^approval=' "$GITHUB_OUTPUT"
unset GITHUB_EVENT_NAME

finish_tests
