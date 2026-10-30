#!/usr/bin/env bash
# run.sh gate|prepare|review|post|fallback|summary|cleanup — the steps of the tribunal action.
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
  # The pinned Claude CLI, whatever the runner has: the binary was tested against it.
  npm install --silent --no-audit --no-fund --prefix "$tools/claude" "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}"
  ln -sf "$tools/claude/node_modules/.bin/claude" "$tools/bin/claude"
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
  install_tools # before pick_profile, which needs gh
  pick_profile
  log "tribunal $tag ($asset)"
}

# Run the binary with the job's isolated environment and one Claude credential.
tribunal() {
  # The binary uses an API key only when it comes as TRIBUNAL_ANTHROPIC_API_KEY, and drops any
  # other. With both credentials, both are passed: a binary that reads the key uses it (the CLI
  # ranks a key above the login), and an older one still runs on the OAuth token.
  if [ -n "${ANTHROPIC_API_KEY:-}" ]; then export TRIBUNAL_ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"; fi
  [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || unset CLAUDE_CODE_OAUTH_TOKEN
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
  local run_url="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID" skipped="" reason=""
  [ -f "$JOB/run.json" ] && skipped="$(python3 -c 'import json,sys
r = json.load(open(sys.argv[1]))
if r.get("skipped"): print(r.get("reason") or "no reason given")' "$JOB/run.json" 2>/dev/null)"
  if [ -n "$skipped" ]; then
    printf '%s\n%s\n' "$PENDING_MARKER" \
      "**Tribunal skipped** this pull request: $skipped. ([run log]($run_url))" > "$JOB/pending.md"
  else
    # Why nothing was posted: the post's own line, else the last line the binary printed.
    [ -f "$JOB/post.log" ] && reason="$(grep -m1 '^NOT POSTED:' "$JOB/post.log")"
    [ -z "$reason" ] && [ -f "$JOB/review.log" ] && reason="$(grep '^tribunal:' "$JOB/review.log" | tail -n 1)"
    printf '%s\n%s\n%s\n' "$PENDING_MARKER" \
      "**Tribunal pending** — the review run ended without posting a review ([run log]($run_url)).${reason:+ $reason}" \
      "Add the \`$INPUT_LABEL\` label again to retry." > "$JOB/pending.md"
  fi
  gh pr comment "$(pr_number)" --repo "$GITHUB_REPOSITORY" --body-file "$JOB/pending.md"
}

# The run's page on GitHub shows what happened and the report itself, so nobody needs the
# artifact to see how a review went. GitHub caps a step's summary at 1 MiB.
cmd_summary() {
  [ -n "${GITHUB_STEP_SUMMARY:-}" ] || return 0
  local pr status url skipped reason report
  pr="$(pr_number)"; report="$JOB/tmp/tribunal-report-$pr.md"
  url="$(grep -m1 -oE '^Posted [a-z-]+: \S+' "$JOB/post.log" 2>/dev/null | sed 's/^Posted [a-z-]*: //')"
  [ -f "$JOB/run.json" ] && skipped="$(python3 -c 'import json,sys
r = json.load(open(sys.argv[1]))
if r.get("skipped"): print(r.get("reason") or "no reason given")' "$JOB/run.json" 2>/dev/null)"
  if [ -n "$url" ]; then status="✅ posted"
  elif [ -n "${skipped:-}" ]; then status="⏭️ skipped: $skipped"
  else
    reason="$(grep -m1 '^NOT POSTED:' "$JOB/post.log" 2>/dev/null)"
    [ -z "$reason" ] && reason="$(grep '^tribunal:' "$JOB/review.log" 2>/dev/null | tail -n 1)"
    status="⏳ not posted${reason:+: $reason}"
  fi
  {
    echo "### Tribunal — PR #$pr · $status"
    [ -n "$url" ] && printf '\n[See the review on the pull request](%s)\n' "$url"
    echo
    if [ -f "$report" ]; then
      head -c 900000 "$report"
      [ "$(wc -c < "$report")" -gt 900000 ] && printf '\n\n*Report cut to fit the job summary; the full one is in the run artifact.*\n'
    else
      echo "No report was written."
    fi
  } >> "$GITHUB_STEP_SUMMARY"
  return 0
}

# The label is a one-shot request: remove it so adding it again starts a new review.
cmd_cleanup() {
  local label pr slug head
  label="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$INPUT_LABEL")"
  gh api -X DELETE "repos/$GITHUB_REPOSITORY/issues/$(pr_number)/labels/$label" >/dev/null 2>&1 \
    || log "label '$INPUT_LABEL' was not on the pull request"
  # The binary's own names: the slug turns every run of non-alphanumerics into one `-`, and the
  # vote dir carries the head commit's first 8 characters.
  pr="$(pr_number)"; head="$(event pull_request.head.sha)"
  slug="$(python3 -c 'import re,sys;print(re.sub(r"[^A-Za-z0-9]+","-",sys.argv[1]))' "$GITHUB_REPOSITORY")"
  rm -rf "$JOB" "$FIXED_TMP/tribunal-methodology-$slug-$pr.md" "$FIXED_TMP/tribunal-conventions-$slug-$pr.md" \
    "$FIXED_TMP/peer-review-votes-$pr-${head:0:8}"
  return 0
}

case "${1:-}" in
  gate) cmd_gate ;;
  prepare) cmd_prepare ;;
  review) cmd_review ;;
  post) cmd_post ;;
  fallback) cmd_fallback ;;
  summary) cmd_summary ;;
  cleanup) cmd_cleanup ;;
  *) echo "usage: run.sh gate|prepare|review|post|fallback|summary|cleanup" >&2; exit 2 ;;
esac
