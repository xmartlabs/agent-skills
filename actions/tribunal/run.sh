#!/usr/bin/env bash
# run.sh gate|prepare|review|post|fallback|cleanup — the steps of the tribunal action.
#
# Each step is a subcommand so it can be tested on its own. Everything a run writes lives in one
# job directory under RUNNER_TEMP: HOME (Claude's config and the homosapiens skill), TMPDIR (the
# pull request's checkout, report and transcripts), the binary's data directory and the binary.
# Self-hosted runners are persistent and share a host, so nothing goes to the runner's own HOME.
# pipefail: review and post pipe the binary through `tee`, and its exit status must survive it.
set -u -o pipefail

ACTION_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN="$(cd "$ACTION_DIR/../../plugins/tribunal" && pwd)"
HOMOSAPIENS="$(cd "$ACTION_DIR/../../plugins/homosapiens/skills/homosapiens" && pwd)"
JOB="${RUNNER_TEMP:?RUNNER_TEMP is not set}/tribunal"
# The binary writes the run's methodology, conventions and votes under /tmp whatever TMPDIR says.
FIXED_TMP="${TRIBUNAL_FIXED_TMP:-/tmp}"
PENDING_MARKER='<!-- tribunal:pending -->'
GH_VERSION="${TRIBUNAL_GH_VERSION:-2.103.3}"
CLAUDE_CODE_VERSION="${TRIBUNAL_CLAUDE_CODE_VERSION:-2.1.312}"

log() { echo "tribunal: $*" >&2; }
out() { echo "$1=$2" >> "${GITHUB_OUTPUT:-/dev/null}"; }

# event KEY: a dotted key of the triggering event's payload, empty when absent.
event() {
  python3 - "${GITHUB_EVENT_PATH:?}" "$1" <<'PY'
import json, sys
cur = json.load(open(sys.argv[1]))
for k in sys.argv[2].split('.'):
    cur = cur.get(k) if isinstance(cur, dict) else None
print('' if cur is None else (str(cur).lower() if isinstance(cur, bool) else cur))
PY
}

pr_number() { event pull_request.number; }

cmd_gate() {
  decide() { log "gate: $1"; out run "$2"; }
  [ -n "$(pr_number)" ] || { decide "not a pull_request event" false; return 0; }
  [ "$(event pull_request.head.repo.full_name)" = "$GITHUB_REPOSITORY" ] \
    || { decide "fork pull request (no secrets)" false; return 0; }
  # Dependabot and similar runs get no secrets: without Claude credentials there is nothing to run.
  [ "${HAS_CLAUDE_AUTH:-}" = true ] \
    || { decide "no claude-token or anthropic-api-key (secrets are not available to this run)" false; return 0; }
  if [ "$(event action)" = labeled ] && [ "$(event label.name)" != "$INPUT_LABEL" ]; then
    decide "label '$(event label.name)' is not '$INPUT_LABEL'" false; return 0
  fi
  decide "$(event action)" true
}

# release_field KEY [TARGET]: a field of the binary pin in the plugin's rust/release.json
# (TRIBUNAL_RUST_MANIFEST overrides the path, as in the plugin's own launcher).
release_field() {
  python3 - "${TRIBUNAL_RUST_MANIFEST:-$PLUGIN/rust/release.json}" "$@" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
if len(sys.argv) == 3:
    print(m[sys.argv[2]])
else:
    print(m["assets"][sys.argv[3]][sys.argv[2]])
PY
}

target() { # TRIBUNAL_TARGET overrides the detection (tests run on other platforms)
  [ -n "${TRIBUNAL_TARGET:-}" ] && { echo "$TRIBUNAL_TARGET"; return 0; }
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo x86_64-unknown-linux-musl ;;
    *) log "no tribunal binary for $(uname -s) $(uname -m); use a Linux x64 runner"; return 1 ;;
  esac
}

# The profile is read at the base branch, then the default branch, so a pull request cannot change
# the rules it is reviewed under. A repository with neither gets the pull request's own copy (the
# PR that adds it), and a repository with none at all gets the plugin's template for INPUT_REVIEWER.
pick_profile() {
  local base_ref head_sha
  base_ref="$(event pull_request.base.ref)"; head_sha="$(event pull_request.head.sha)"
  has_profile() { # REF_QUERY
    gh api "repos/$GITHUB_REPOSITORY/contents/.claude/review-profile.json$1" --jq .path \
      > /dev/null 2> "$JOB/profile.err" && return 0
    grep -q 'HTTP 404' "$JOB/profile.err" && return 1
    cat "$JOB/profile.err" >&2; exit 1
  }
  : > "$JOB/profile-args"
  if has_profile "?ref=$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$base_ref")" \
      || has_profile ""; then
    log "using .claude/review-profile.json from the base or default branch"
  elif has_profile "?ref=$head_sha"; then
    log "the base branch has no profile yet; using the pull request's copy"
    for f in review-profile.json review-rules.md; do
      gh api -H 'Accept: application/vnd.github.raw' \
        "repos/$GITHUB_REPOSITORY/contents/.claude/$f?ref=$head_sha" > "$JOB/$f" 2>/dev/null || rm -f "$JOB/$f"
    done
    printf '%s\n' --profile "$JOB/review-profile.json" >> "$JOB/profile-args"
    [ -f "$JOB/review-rules.md" ] && printf '%s\n' --rules "$JOB/review-rules.md" >> "$JOB/profile-args"
  else
    case "$INPUT_REVIEWER" in frontend | backend) ;; *) log "reviewer must be frontend or backend, not '$INPUT_REVIEWER'"; exit 1 ;; esac
    log "the repository has no .claude/review-profile.json; using the default $INPUT_REVIEWER profile"
    python3 - "$PLUGIN/templates/review-profile.$INPUT_REVIEWER.json" "$JOB/review-profile.json" \
      "$GITHUB_REPOSITORY" "$base_ref" <<'PY'
import json, sys
src, dst, repo, base = sys.argv[1:]
p = json.load(open(src))
p.update(project=repo.split("/")[-1], repo=repo, baseBranch=base, crossRepoAnchors=[])
p.pop("projectRules", None)
json.dump(p, open(dst, "w"), indent=2)
PY
    printf '%s\n' --profile "$JOB/review-profile.json" >> "$JOB/profile-args"
  fi
}

gh_at_least() { # MAJOR MINOR
  command -v gh >/dev/null || return 1
  gh --version | python3 -c 'import re,sys; m=re.search(r"gh version (\d+)\.(\d+)",sys.stdin.read()); sys.exit(0 if m and (int(m[1]),int(m[2]))>=(int(sys.argv[1]),int(sys.argv[2])) else 1)' "$1" "$2"
}

install_tools() {
  local tools="$JOB/tools"
  mkdir -p "$tools/bin"; echo "$tools/bin" >> "${GITHUB_PATH:-/dev/null}"
  export PATH="$tools/bin:$PATH"
  if ! gh_at_least 2 60; then
    local rel="https://github.com/cli/cli/releases/download/v${GH_VERSION}" tgz="gh_${GH_VERSION}_linux_amd64.tar.gz"
    curl -fsSL -o "$tools/$tgz" "$rel/$tgz"
    curl -fsSL -o "$tools/gh-checksums.txt" "$rel/gh_${GH_VERSION}_checksums.txt"
    (cd "$tools" && grep " $tgz\$" gh-checksums.txt | sha256sum -c -)
    tar -xzf "$tools/$tgz" -C "$tools"
    ln -sf "$tools/gh_${GH_VERSION}_linux_amd64/bin/gh" "$tools/bin/gh"
  fi
  if ! command -v claude >/dev/null; then
    npm install --silent --no-audit --no-fund --prefix "$tools/claude" "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}"
    ln -sf "$tools/claude/node_modules/.bin/claude" "$tools/bin/claude"
  fi
}

cmd_prepare() {
  set -e
  rm -rf "$JOB"
  mkdir -p "$JOB/home/.claude/skills" "$JOB/tmp" "$JOB/data" "$JOB/work" "$JOB/bin"
  date -u +%Y-%m-%dT%H:%M:%SZ > "$JOB/started-at"

  # The binary pinned by this ref's release.json, checked against its sha256.
  local tgt tag repo asset sha
  tgt="$(target)"; tag="$(release_field tag)"; repo="$(release_field repo)"
  asset="$(release_field name "$tgt")"; sha="$(release_field sha256 "$tgt")"
  curl -fsSL -o "$JOB/bin/$asset" "https://github.com/$repo/releases/download/$tag/$asset" \
    || { log "could not download $asset from $repo release $tag"; return 1; }
  echo "$sha  $JOB/bin/$asset" | sha256sum -c - >&2 || { log "$asset does not match release.json"; return 1; }
  tar -xzf "$JOB/bin/$asset" -C "$JOB/bin"
  local bin; bin="$(find "$JOB/bin" -type f -name tribunal | head -n 1)"
  [ -n "$bin" ] || { log "no tribunal binary in $asset"; return 1; }
  chmod +x "$bin"; echo "$bin" > "$JOB/bin-path"

  # The post step rewrites the report with the homosapiens skill; without it nothing is posted.
  cp -R "$HOMOSAPIENS" "$JOB/home/.claude/skills/homosapiens"
  pick_profile
  install_tools
  log "tribunal $tag ($asset)"
}

# Run the binary with the job's isolated environment and one Claude credential.
tribunal() {
  # An API key wins over the OAuth token; the binary only uses a key given as
  # TRIBUNAL_ANTHROPIC_API_KEY, and drops any other.
  if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    export TRIBUNAL_ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"
    [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] && log "both claude-token and anthropic-api-key are set; using the API key"
    unset CLAUDE_CODE_OAUTH_TOKEN
  fi
  unset ANTHROPIC_API_KEY
  export HOME="$JOB/home" TMPDIR="$JOB/tmp" TRIBUNAL_DATA_DIR="$JOB/data" TRIBUNAL_PLUGIN_ROOT="$PLUGIN"
  export PATH="$JOB/tools/bin:$PATH"
  cd "$JOB/work" || return 1
  "$(cat "$JOB/bin-path")" "$@"
}

cmd_review() {
  local args=()
  while IFS= read -r a; do args+=("$a"); done < "$JOB/profile-args"
  # The binary clones the pull request with GH_TOKEN; this lets its git fetches use it too.
  HOME="$JOB/home" gh auth setup-git >&2
  tribunal review --pr "$(pr_number)" --repo "$GITHUB_REPOSITORY" --head-sha "$(event pull_request.head.sha)" \
    --no-codex --no-post --out "$JOB/run.json" ${args[@]+"${args[@]}"} 2>&1 | tee "$JOB/review.log"
}

# GITHUB_TOKEN cannot read `gh api user`, so once a pull request has a review the duplicate-review
# gate cannot tell whose it is and holds the post. Running the action is a request for a review, so
# the full review is posted over any standing one, always as a comment.
cmd_post() {
  tribunal post --from "$JOB/run.json" --verdict comment --over-standing-review 2>&1 | tee "$JOB/post.log"
}

cmd_fallback() {
  local reason=""
  reason="$(python3 - "$JOB/run.json" <<'PY' 2>/dev/null
import json, sys
r = json.load(open(sys.argv[1]))
if r.get("skipped"):
    print(f" Tribunal skipped this pull request: {r.get('reason')}.")
PY
)" || reason=""
  if [ -z "$reason" ] && [ -f "$JOB/post.log" ]; then
    reason="$(grep -m1 '^NOT POSTED:' "$JOB/post.log" | sed 's/^/ /')"
  fi
  local run_url="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
  printf '%s\n%s\n%s\n' "$PENDING_MARKER" \
    "**Tribunal pending** — the review run ended without posting a review ([run log]($run_url)).$reason" \
    "Add the \`$INPUT_LABEL\` label again to retry." > "$JOB/pending.md"
  gh pr comment "$(pr_number)" --repo "$GITHUB_REPOSITORY" --body-file "$JOB/pending.md"
}

# The label is a one-shot request: remove it so adding it again starts a new review.
cmd_cleanup() {
  gh api -X DELETE "repos/$GITHUB_REPOSITORY/issues/$(pr_number)/labels/$INPUT_LABEL" >/dev/null 2>&1 \
    || log "label '$INPUT_LABEL' was not on the pull request"
  local slug="${GITHUB_REPOSITORY//\//-}" pr; pr="$(pr_number)"
  rm -rf "$JOB" "$FIXED_TMP/tribunal-methodology-$slug-$pr.md" "$FIXED_TMP/tribunal-conventions-$slug-$pr.md" \
    "$FIXED_TMP"/peer-review-votes-"$pr"-*
  return 0
}

case "${1:-}" in
  gate) cmd_gate ;;
  prepare) cmd_prepare ;;
  review) cmd_review ;;
  post) cmd_post ;;
  fallback) cmd_fallback ;;
  cleanup) cmd_cleanup ;;
  *) echo "usage: run.sh gate|prepare|review|post|fallback|cleanup" >&2; exit 2 ;;
esac
