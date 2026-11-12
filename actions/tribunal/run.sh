#!/usr/bin/env bash
# run.sh gate|node-check|validate|prepare|restore|review|post|findings-check|baseline|fallback|summary|cleanup — the steps of the tribunal action.
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
CODEX_VERSION=0.165.0

selected_paths() {
  if [ -f "$JOB/plugin-path" ]; then PLUGIN="$(cat "$JOB/plugin-path")"; fi
  if [ -f "$JOB/homosapiens-path" ]; then HOMOSAPIENS="$(cat "$JOB/homosapiens-path")"; fi
}
selected_paths

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

# matches_paths FILE_LIST PATTERNS: exit 0 when a listed file matches an include pattern and no
# exclude pattern (`!glob`). fnmatch semantics: `*` also crosses `/`.
matches_paths() {
  python3 - "$1" "$2" <<'PY'
import fnmatch, sys
files = [l.strip() for l in open(sys.argv[1]) if l.strip()]
pats = [p.strip() for p in sys.argv[2].splitlines() if p.strip()]
inc = [p for p in pats if not p.startswith('!')]
exc = [p[1:] for p in pats if p.startswith('!')]
ok = any(any(fnmatch.fnmatch(f, p) for p in inc) and not any(fnmatch.fnmatch(f, p) for p in exc)
         for f in files)
sys.exit(0 if ok else 1)
PY
}

changed_files() { # FILE
  gh api --paginate "repos/$GITHUB_REPOSITORY/pulls/$(pr_number)/files?per_page=100" \
    --jq '.[].filename' > "$1"
}

# own_reviews FILE: the ids of the pull request's reviews that carry the footer the binary signs
# its posts with, written by the action's own identities: github-actions[bot] and the
# github-token's user when it can read one (a PAT or an App's user token; the Actions token cannot).
own_reviews() { # FILE [STATE]: with STATE, only the reviews in that state (CHANGES_REQUESTED)
  local own=("github-actions[bot]") login logins state=""
  login="$(gh api user --jq .login 2>/dev/null)" && [ -n "$login" ] && own+=("$login")
  logins="$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1:]))' "${own[@]}")"
  [ -z "${2:-}" ] || state=" and .state == \"$2\""
  gh api --paginate "repos/$GITHUB_REPOSITORY/pulls/$(pr_number)/reviews?per_page=100" \
    --jq ".[] | select((.user.login as \$l | $logins | index(\$l)) and ((.body // \"\") | contains(\"<sub>tribunal v\"))$state) | .id" \
    > "$1"
}

# The rules, in order: no pull request, a fork or no credential skips; the action's label forces a
# review and another label skips; any other event (the caller's `on:` chose it) runs unless the
# pull request is a draft, no changed file matches `paths`, or `once` and the action reviewed it.
cmd_gate() {
  decide() { log "gate: $1"; out run "$2"; }
  [ -n "$(pr_number)" ] || { decide "not a pull_request event" false; return 0; }
  [ "$(event pull_request.head.repo.full_name)" = "$GITHUB_REPOSITORY" ] \
    || { decide "fork pull request (no secrets)" false; return 0; }
  # Dependabot and similar runs get no secrets: without Claude credentials there is nothing to run.
  [ "${HAS_CLAUDE_AUTH:-}" = true ] \
    || { decide "no claude-token or anthropic-api-key (secrets are not available to this run)" false; return 0; }
  if [ "$(event action)" = labeled ]; then
    if [ "$(event label.name)" = "$INPUT_LABEL" ]; then decide "label '$INPUT_LABEL' forces a review" true
    else decide "label '$(event label.name)' is not '$INPUT_LABEL'" false; fi
    return 0
  fi
  if [ "$(event pull_request.draft)" = true ] && [ "${INPUT_DRAFTS:-false}" != true ]; then
    decide "draft pull request (drafts: false); add the '$INPUT_LABEL' label to review it" false; return 0
  fi
  mkdir -p "$JOB"
  if [ -n "${INPUT_PATHS:-}" ]; then
    changed_files "$JOB/changed-files.txt" || { log "gate: could not list the pull request's files"; return 1; }
    matches_paths "$JOB/changed-files.txt" "$INPUT_PATHS" \
      || { decide "no changed file matches 'paths'" false; return 0; }
  fi
  if [ "${INPUT_ONCE:-true}" = true ]; then
    own_reviews "$JOB/own-reviews.txt" || { log "gate: could not list the pull request's reviews"; return 1; }
    if [ -s "$JOB/own-reviews.txt" ]; then
      decide "already reviewed (once: true); add the '$INPUT_LABEL' label to review again" false; return 0
    fi
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
pick_profile() { python3 "$ACTION_DIR/runtime.py" profile; }

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
  if [ "${INPUT_CODEX:-false}" = true ]; then
    npm install --silent --no-audit --no-fund --prefix "$tools/codex" "@openai/codex@$CODEX_VERSION"
    ln -sf "$tools/codex/node_modules/.bin/codex" "$tools/bin/codex"
  fi
}

# The action installs the Claude CLI with npm. Runners without Node (some self-hosted ones) get it
# from actions/setup-node in the next step.
cmd_node_check() {
  if command -v npm >/dev/null 2>&1; then out missing false; else out missing true; fi
}

cmd_prepare() {
  set -e
  python3 "$ACTION_DIR/runtime.py" validate
  local codex_key="${INPUT_OPENAI_API_KEY:-}"
  unset INPUT_OPENAI_API_KEY OPENAI_API_KEY CODEX_API_KEY CODEX_HOME
  rm -rf "$JOB"
  mkdir -p "$JOB/home/.claude/skills" "$JOB/tmp" "$JOB/data" "$JOB/work" "$JOB/bin"
  date -u +%Y-%m-%dT%H:%M:%SZ > "$JOB/started-at"

  install_tools
  python3 "$ACTION_DIR/runtime.py" resolve
  selected_paths
  python3 "$ACTION_DIR/runtime.py" args > "$JOB/review-args"
  printf '%s\n' "${INPUT_VERDICT:-comment}" > "$JOB/verdict"
  printf '%s\n' "${INPUT_REVIEW_MODE:-full}" > "$JOB/review-mode"
  if [ "${INPUT_CODEX:-false}" = true ]; then
    mkdir -p "$JOB/home/.codex"
    chmod 700 "$JOB/home/.codex"
    printf '%s' "$codex_key" | HOME="$JOB/home" CODEX_HOME="$JOB/home/.codex" "$JOB/tools/bin/codex" login --with-api-key > /dev/null
    unset codex_key
    if [ -n "${INPUT_CODEX_MODEL:-}" ]; then
      printf 'model = "%s"\n' "$INPUT_CODEX_MODEL" > "$JOB/home/.codex/review.config.toml"
    fi
  fi

  # The selected freeze's binary, checked against its sha256.
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
  log "tribunal $tag ($asset)"
}

# The ref this action runs at, `<owner>/<repo>@<ref>`, read from the path GitHub checks a used action
# out to (`.../_actions/<owner>/<repo>/<ref>/actions/tribunal`); the engine records it as `actionRef`.
# Empty (and so not recorded) when the action runs from the repository itself (`uses: ./`).
action_ref() {
  case "${GITHUB_ACTION_PATH:-}" in
    */_actions/*/*/*/actions/tribunal) ;;
    *) return 0 ;;
  esac
  local rest=${GITHUB_ACTION_PATH#*/_actions/}
  rest=${rest%/actions/tribunal}
  local owner=${rest%%/*} tail=${rest#*/}
  printf '%s/%s@%s' "$owner" "${tail%%/*}" "${tail#*/}"
}

# Run the binary with the job's isolated environment and one Claude credential.
tribunal() {
  # The binary uses an API key only when it comes as TRIBUNAL_ANTHROPIC_API_KEY, and drops any
  # other. With both credentials, both are passed: a binary that reads the key uses it (the CLI
  # ranks a key above the login), and an older one still runs on the OAuth token.
  if [ -n "${ANTHROPIC_API_KEY:-}" ]; then export TRIBUNAL_ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"; fi
  [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || unset CLAUDE_CODE_OAUTH_TOKEN
  [ -n "${TRIBUNAL_CONVENTIONS_TOKEN:-}" ] || unset TRIBUNAL_CONVENTIONS_TOKEN
  unset ANTHROPIC_API_KEY
  export HOME="$JOB/home" TMPDIR="$JOB/tmp" TRIBUNAL_DATA_DIR="$JOB/data" TRIBUNAL_PLUGIN_ROOT="$PLUGIN"
  unset OPENAI_API_KEY CODEX_API_KEY INPUT_OPENAI_API_KEY
  export CODEX_HOME="$JOB/home/.codex"
  export PATH="$JOB/tools/bin:$PATH"
  TRIBUNAL_ACTION_REF=$(action_ref) && export TRIBUNAL_ACTION_REF
  cd "$JOB/work" || return 1
  "$(cat "$JOB/bin-path")" "$@"
}

# Never let the engine's human-review fallback stand in for a verified CI baseline.
cmd_restore() {
  mkdir -p "$JOB/data"
  rm -f "$JOB/data/metrics.jsonl"
  printf '%s\n' full > "$JOB/effective-mode"
  if [ "$(cat "$JOB/review-mode")" = incremental ]; then
    if python3 "$ACTION_DIR/runtime.py" restore 2> "$JOB/baseline-reason"; then
      printf '%s\n' incremental > "$JOB/effective-mode"
      log "restored a verified incremental baseline"
    else
      log "using a full review: $(cat "$JOB/baseline-reason")"
    fi
  fi
}

cmd_review() {
  local args=() a expected live rc
  # Secrets are masked already; a token minted by an earlier step may not be.
  if [ -n "${TRIBUNAL_CONVENTIONS_TOKEN:-}" ]; then echo "::add-mask::$TRIBUNAL_CONVENTIONS_TOKEN"; fi
  while IFS= read -r a; do [ -n "$a" ] && args+=("$a"); done < "$JOB/profile-args"
  while IFS= read -r a; do [ -n "$a" ] && args+=("$a"); done < "$JOB/review-args"
  : > "$JOB/review.log"
  expected="$(event pull_request.head.sha)"
  live="$(gh api "repos/$GITHUB_REPOSITORY/pulls/$(pr_number)" --jq .head.sha)" || {
    log "cannot verify the current PR head" 2>&1 | tee -a "$JOB/review.log"; return 1;
  }
  if [ "$live" != "$expected" ]; then
    log "PR head moved before review; add the label again" 2>&1 | tee -a "$JOB/review.log"; return 1
  fi
  if [ "$(cat "$JOB/effective-mode" 2>/dev/null || echo full)" != incremental ]; then
    args+=(--head-sha "$expected" --full)
  fi
  HOME="$JOB/home" gh auth setup-git >&2 || return 1
  tribunal review --pr "$(pr_number)" --repo "$GITHUB_REPOSITORY" --no-post --out "$JOB/run.json" "${args[@]}" 2>&1 | tee -a "$JOB/review.log"
  rc=$?
  [ "$rc" -eq 0 ] || return "$rc"
  python3 "$ACTION_DIR/runtime.py" check-head 2>&1 | tee -a "$JOB/review.log"
}

# Keep existing standing-review behavior. The verdict is opt-in; HEAD checks are not.
cmd_post() {
  python3 "$ACTION_DIR/runtime.py" check-head 2>&1 | tee "$JOB/post.log" || return 1
  local args=(--from "$JOB/run.json" --over-standing-review) state rc
  state="$(python3 "$ACTION_DIR/runtime.py" post-state)" || return 1
  [ -z "$state" ] || args+=(--verdict "$state")
  # The same model/effort applies to review and report composition.
  local name value
  for name in model effort; do
    value="$(python3 - "$JOB/review-args" "--$name" <<'PYARG'
import sys
args = open(sys.argv[1]).read().splitlines()
print(args[args.index(sys.argv[2]) + 1] if sys.argv[2] in args else '')
PYARG
)"
    [ -z "$value" ] || args+=("--$name" "$value")
  done
  tribunal post "${args[@]}" 2>&1 | tee -a "$JOB/post.log"
  rc=$?
  [ "$rc" -eq 0 ] || return "$rc"
  if [ "$(cat "$JOB/verdict")" = no-approve ] && [ "$state" = comment ]; then dismiss_change_requests; fi
}

# A comment does not clear the action's earlier change request, which would keep blocking the merge
# after the findings it asked for are fixed. Best effort: a failure is logged, never fatal.
dismiss_change_requests() {
  local id
  own_reviews "$JOB/change-requests" CHANGES_REQUESTED 2>> "$JOB/post.log" || {
    echo "::warning::tribunal: could not list earlier change requests to dismiss"; return 0; }
  while IFS= read -r id; do
    [ -n "$id" ] || continue
    if gh api -X PUT "repos/$GITHUB_REPOSITORY/pulls/$(pr_number)/reviews/$id/dismissals" \
        -f event=DISMISS -f message="Superseded by tribunal's review of $(event pull_request.head.sha | cut -c1-8), which does not request changes." \
        > /dev/null 2>> "$JOB/post.log"; then
      log "dismissed the earlier change request $id" 2>&1 | tee -a "$JOB/post.log" >&2
    else
      echo "::warning::tribunal: could not dismiss the earlier change request $id"
    fi
  done < "$JOB/change-requests"
  return 0
}

cmd_baseline() {
  out saved false
  if python3 "$ACTION_DIR/runtime.py" save 2> "$JOB/baseline-save-reason"; then
    out saved true
  else
    log "baseline not saved: $(cat "$JOB/baseline-save-reason")"
  fi
}

# The "Tribunal findings" commit status on the reviewed head. Best effort: a missing
# `statuses: write` or a count that cannot be made never fails the job, and the summary says what
# happened.
cmd_findings_check() {
  local line
  if ! line="$(python3 "$ACTION_DIR/runtime.py" findings-check 2> "$JOB/findings-check.err")"; then
    printf 'Findings status not created: %s\n' "$(tail -n 1 "$JOB/findings-check.err")" | tee "$JOB/findings-line" >&2
    return 0
  fi
  if gh api -X POST "repos/$GITHUB_REPOSITORY/statuses/$(event pull_request.head.sha)" --input "$JOB/status.json" > /dev/null 2> "$JOB/findings-check.err"; then
    printf 'Findings status: %s\n' "$line" | tee "$JOB/findings-line" >&2
  else
    printf 'Findings status not created (%s). Give the job `statuses: write`.\n' "$(tail -n 1 "$JOB/findings-check.err")" \
      | tee "$JOB/findings-line" >&2
    echo "::warning::tribunal: the findings status was not created; give the job statuses: write"
  fi
  return 0
}

# Why nothing was posted: the post's own line, else the last line the binary printed, else the
# setup failed before either ran.
not_posted_reason() {
  local reason=""
  [ -f "$JOB/post.log" ] && reason="$(grep -m1 '^NOT POSTED:' "$JOB/post.log")"
  [ -z "$reason" ] && [ -f "$JOB/review.log" ] && reason="$(grep '^tribunal:' "$JOB/review.log" | tail -n 1)"
  [ -z "$reason" ] && [ ! -f "$JOB/review.log" ] && reason="the action's setup failed before the review started; see the run log"
  printf '%s' "$reason"
}

cmd_fallback() {
  mkdir -p "$JOB"
  local run_url="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID" skipped="" reason=""
  [ -f "$JOB/run.json" ] && skipped="$(python3 -c 'import json,sys
r = json.load(open(sys.argv[1]))
if r.get("skipped"): print(r.get("reason") or "no reason given")' "$JOB/run.json" 2>/dev/null)"
  if [ -n "$skipped" ]; then
    printf '%s\n%s\n' "$PENDING_MARKER" \
      "**Tribunal skipped** this pull request: $skipped. ([run log]($run_url))" > "$JOB/pending.md"
  else
    reason="$(not_posted_reason)"
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
  url="$(grep -m1 -oE '^Posted [A-Za-z _-]+: \S+' "$JOB/post.log" 2>/dev/null | sed -E 's/^Posted [A-Za-z _-]+: //')"
  [ -f "$JOB/run.json" ] && skipped="$(python3 -c 'import json,sys
r = json.load(open(sys.argv[1]))
if r.get("skipped"): print(r.get("reason") or "no reason given")' "$JOB/run.json" 2>/dev/null)"
  if [ -n "$url" ]; then status="✅ posted"
  elif [ -n "${skipped:-}" ]; then status="⏭️ skipped: $skipped"
  else
    reason="$(not_posted_reason)"
    status="⏳ not posted${reason:+: $reason}"
  fi
  {
    echo "### Tribunal — PR #$pr · $status"
    [ -n "$url" ] && printf '\n[See the review on the pull request](%s)\n' "$url"
    echo
    if [ -f "$JOB/selection.json" ]; then
      python3 - "$JOB/selection.json" <<'PYINFO'
import json, sys
s = json.load(open(sys.argv[1]))
print(f"Tribunal `{s['version']}` · commit `{s['commit']}` · binary `{s['binary']}`")
PYINFO
    fi
    if [ -f "$JOB/run.json" ] || [ -f "$JOB/effective-mode" ]; then
      python3 - "$JOB" <<'PYMODE'
import json, pathlib, sys
job = pathlib.Path(sys.argv[1])
try:
    metrics = json.loads((job / 'run.json').read_text()).get('metrics') or {}
except (OSError, ValueError, AttributeError):
    metrics = {}
if not isinstance(metrics, dict):
    metrics = {}
mode = metrics.get('reviewMode')
if mode not in ('full', 'incremental'):
    try:
        mode = (job / 'effective-mode').read_text().strip()
    except OSError:
        mode = None
if mode in ('full', 'incremental'):
    print(f'\nReview mode: `{mode}`')
fallback = metrics.get('incrementalFallback')
if fallback:
    print('Incremental fallback: ' + str(fallback).replace('\n', ' '))
PYMODE
    fi
    [ ! -f "$JOB/findings-line" ] || { echo; cat "$JOB/findings-line"; }
    [ ! -f "$JOB/findings-table.md" ] || { echo; cat "$JOB/findings-table.md"; }
    [ ! -f "$JOB/baseline-reason" ] || cat "$JOB/baseline-reason"
    [ ! -f "$JOB/baseline-save-reason" ] || cat "$JOB/baseline-save-reason"
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
  local label pr slug head actual_head
  label="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$INPUT_LABEL")"
  gh api -X DELETE "repos/$GITHUB_REPOSITORY/issues/$(pr_number)/labels/$label" >/dev/null 2>&1 \
    || log "label '$INPUT_LABEL' was not on the pull request"
  # The binary's own names: the slug turns every run of non-alphanumerics into one `-`, and the
  # vote dir carries the head commit's first 8 characters.
  pr="$(pr_number)"; head="$(event pull_request.head.sha)"
  slug="$(python3 -c 'import re,sys;print(re.sub(r"[^A-Za-z0-9]+","-",sys.argv[1]))' "$GITHUB_REPOSITORY")"
  actual_head="$(python3 - "$JOB/run.json" <<'PYCLEAN'
import json, re, sys
try:
    head = json.load(open(sys.argv[1])).get('metrics', {}).get('headSha', '')
    print(head[:8] if isinstance(head, str) and re.fullmatch(r'[0-9a-f]{8,40}', head) else '')
except (OSError, ValueError, TypeError):
    print('')
PYCLEAN
)"
  [ -z "$actual_head" ] || rm -rf "$FIXED_TMP/peer-review-votes-$pr-$actual_head"
  rm -rf "$JOB" "$FIXED_TMP/tribunal-methodology-$slug-$pr.md" "$FIXED_TMP/tribunal-conventions-$slug-$pr.md" \
    "$FIXED_TMP/peer-review-votes-$pr-${head:0:8}"
  return 0
}

case "${1:-}" in
  validate) python3 "$ACTION_DIR/runtime.py" validate ;;
  gate) cmd_gate ;;
  node-check) cmd_node_check ;;
  prepare) cmd_prepare ;;
  restore) cmd_restore ;;
  baseline) cmd_baseline ;;
  findings-check) cmd_findings_check ;;
  review) cmd_review ;;
  post) cmd_post ;;
  fallback) cmd_fallback ;;
  summary) cmd_summary ;;
  cleanup) cmd_cleanup ;;
  *) echo "usage: run.sh gate|node-check|validate|prepare|restore|review|post|findings-check|baseline|fallback|summary|cleanup" >&2; exit 2 ;;
esac
