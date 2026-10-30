#!/usr/bin/env bash
# run.sh gate|prepare|install|warmup|capture|fallback|summary|cleanup — the steps of the pr-evidence
# action.
#
# Each step is a subcommand so it can be tested on its own. State shared between steps lives in
# one job directory under RUNNER_TEMP (per job, also on persistent self-hosted runners): the
# start time, the port, the playwright-cli session, the changed files and Claude's config dir.
set -u

ACTION_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_SRC="$ACTION_DIR/../../skills/pr-evidence"
JOB="${RUNNER_TEMP:?RUNNER_TEMP is not set}/pr-evidence"
POSTED_MARKER='<!-- pr-evidence:posted -->'
PENDING_MARKER='<!-- pr-evidence:pending -->'
GH_VERSION="${PR_EVIDENCE_GH_VERSION:-2.103.3}"
PLAYWRIGHT_CLI_VERSION="${PR_EVIDENCE_PLAYWRIGHT_CLI_VERSION:-0.1.22}"
CLAUDE_CODE_VERSION="${PR_EVIDENCE_CLAUDE_CODE_VERSION:-2.1.312}"

log() { echo "pr-evidence: $*" >&2; }
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

# Comments written by the action's own identities: github-actions[bot], the github-token's user
# when it can read one (a PAT or an App's user token), and the attach-token user. Prints FILTER (a
# jq filter) applied to each of them, newest last.
own_comments() { # FILTER [SINCE_ISO]
  local since="${2:-}" logins='["github-actions[bot]"]'
  [ -f "$JOB/own-logins.json" ] && logins="$(cat "$JOB/own-logins.json")"
  gh api --paginate "repos/$GITHUB_REPOSITORY/issues/$(pr_number)/comments?per_page=100${since:+&since=$since}" \
    --jq ".[] | select(.user.login as \$l | $logins | index(\$l)) | $1"
}

# The bodies of the action's own comments, one JSON string per line, newest last.
own_comment_bodies() { own_comments '.body | @json' "${1:-}"; } # [SINCE_ISO]

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

cmd_gate() {
  local action label draft head_repo
  action="$(event action)"; label="$(event label.name)"; draft="$(event pull_request.draft)"
  head_repo="$(event pull_request.head.repo.full_name)"
  mkdir -p "$JOB"
  decide() { log "gate: $1"; out run "$2"; }

  [ -n "$(pr_number)" ] || { decide "not a pull_request event" false; return 0; }
  [ "$head_repo" = "$GITHUB_REPOSITORY" ] || { decide "fork pull request" false; return 0; }
  # Dependabot and similar runs get no secrets: without Claude credentials there is nothing to run.
  [ "${HAS_CLAUDE_AUTH:-}" = true ] \
    || { decide "no claude-token or anthropic-api-key (secrets are not available to this run)" false; return 0; }
  local own=("github-actions[bot]") login
  login="$(gh api user --jq .login 2>/dev/null)" && [ -n "$login" ] && own+=("$login")
  if [ -n "${ATTACH_TOKEN:-}" ]; then
    login="$(GH_TOKEN="$ATTACH_TOKEN" gh api user --jq .login 2>/dev/null)" && [ -n "$login" ] && own+=("$login") \
      || log "attach-token cannot read its own user"
  fi
  python3 -c 'import json,sys;print(json.dumps(sys.argv[1:]))' "${own[@]}" > "$JOB/own-logins.json"
  if [ "$action" = labeled ]; then
    if [ "$label" = "$INPUT_LABEL" ]; then decide "label '$label' forces a run" true
    else decide "label '$label' is not '$INPUT_LABEL'" false; fi
    return 0
  fi
  # Automatic events: drafts, paths and once (in that order) can each skip the run.
  [ "$draft" = true ] && [ "${INPUT_DRAFTS:-false}" != true ] \
    && { decide "draft pull request" false; return 0; }
  if [ -n "${INPUT_PATHS:-}" ]; then
    changed_files "$JOB/changed-files.txt" || { log "could not list changed files"; return 1; }
    matches_paths "$JOB/changed-files.txt" "$INPUT_PATHS" \
      || { decide "no changed file matches 'paths'" false; return 0; }
  fi
  if [ "${INPUT_ONCE:-true}" != false ] && own_comment_bodies | grep -qF "$POSTED_MARKER"; then
    decide "evidence already posted; add the '$INPUT_LABEL' label to run again" false
    return 0
  fi
  decide "$action" true
}

# The config runs code (its `boot` command), so it comes from the base branch, never the PR head.
# Until the base has one (the PR that adds it), the PR's own copy is used: it adds no trust the
# calling workflow, which already runs from the PR, does not have.
cmd_prepare() {
  set -e
  local cfg="$INPUT_CONFIG" base_sha port host
  base_sha="$(event pull_request.base.sha)"
  mkdir -p "$JOB"
  date -u +%Y-%m-%dT%H:%M:%SZ > "$JOB/started-at"
  printf '%s\n' "$cfg" > "$JOB/config-path"
  [ -f "$cfg" ] && cp "$cfg" "$JOB/config.original.json"
  if gh api -H 'Accept: application/vnd.github.raw' \
      "repos/$GITHUB_REPOSITORY/contents/$cfg?ref=$base_sha" > "$JOB/config.json" 2> "$JOB/config.err"; then
    log "using $cfg from the base branch ($base_sha)"
  elif grep -q 'HTTP 404' "$JOB/config.err"; then
    [ -f "$cfg" ] || { log "no $cfg on the base branch or in the pull request"; return 1; }
    log "the base branch has no $cfg; using the pull request's copy"
    cp "$cfg" "$JOB/config.json"
  else
    cat "$JOB/config.err" >&2; log "could not read $cfg from the base branch"; return 1
  fi

  port="$(pick_port)"; echo "$port" > "$JOB/port"
  if [ -n "${ATTACH_TOKEN:-}" ]; then host=github-attachment; else host=git-branch; fi
  python3 - "$JOB/config.json" "$port" "$host" <<'PY'
import json, sys
from urllib.parse import urlsplit, urlunsplit
path, port, host = sys.argv[1], int(sys.argv[2]), sys.argv[3]
c = json.load(open(path))
if c.get('testOrigin'):
    u = urlsplit(c['testOrigin'])
    host_part = f"[{u.hostname}]" if ":" in (u.hostname or "") else u.hostname
    c['testOrigin'] = urlunsplit((u.scheme, f"{host_part}:{port}", u.path, u.query, u.fragment))
h = c.get('host') or {}
h['target'] = host
c['host'] = h
json.dump(c, open(path, 'w'), indent=2)
PY
  mkdir -p "$(dirname "$cfg")" && cp "$JOB/config.json" "$cfg"
  bash "$SKILL_SRC/scripts/run.sh" check "$cfg" >&2

  echo "pr-evidence-$GITHUB_RUN_ID-${GITHUB_RUN_ATTEMPT:-1}" > "$JOB/session"
  [ -f "$JOB/changed-files.txt" ] || changed_files "$JOB/changed-files.txt"
  mkdir -p "$JOB/claude/skills" && cp -R "$SKILL_SRC" "$JOB/claude/skills/pr-evidence"
  log "port $port, host $host, $(wc -l < "$JOB/changed-files.txt" | tr -d ' ') changed files"
}

pick_port() {
  node -e '
    const net = require("node:net")
    const isFree = (p) => new Promise((ok) => {
      const s = net.createServer().once("error", () => ok(false))
      s.listen(p, "127.0.0.1", () => s.close(() => ok(true)))
    })
    ;(async () => { let p; do p = 20000 + Math.floor(Math.random() * 12000); while (!(await isFree(p))); console.log(p) })()'
}

gh_at_least() { # MAJOR MINOR
  command -v gh >/dev/null || return 1
  gh --version | python3 -c 'import re,sys; m=re.search(r"gh version (\d+)\.(\d+)",sys.stdin.read()); sys.exit(0 if m and (int(m[1]),int(m[2]))>=(int(sys.argv[1]),int(sys.argv[2])) else 1)' "$1" "$2"
}

# Tools go to the job directory and onto GITHUB_PATH; nothing is installed system-wide.
cmd_install() {
  set -e
  local tools="$JOB/tools" arch
  mkdir -p "$tools/bin"; echo "$tools/bin" >> "${GITHUB_PATH:-/dev/null}"
  export PATH="$tools/bin:$PATH"
  case "$(uname -m)" in x86_64) arch=amd64 ;; aarch64 | arm64) arch=arm64 ;; *) arch="$(uname -m)" ;; esac
  if ! gh_at_least 2 99; then
    local rel="https://github.com/cli/cli/releases/download/v${GH_VERSION}" tgz="gh_${GH_VERSION}_linux_${arch}.tar.gz"
    curl -fsSL -o "$tools/$tgz" "$rel/$tgz"
    curl -fsSL -o "$tools/gh-checksums.txt" "$rel/gh_${GH_VERSION}_checksums.txt"
    (cd "$tools" && grep " $tgz\$" gh-checksums.txt | sha256sum -c -)
    tar -xzf "$tools/$tgz" -C "$tools"
    ln -sf "$tools/gh_${GH_VERSION}_linux_${arch}/bin/gh" "$tools/bin/gh"
  fi
  if ! command -v playwright-cli >/dev/null; then
    npm install --silent --no-audit --no-fund --prefix "$tools/pwcli" "@playwright/cli@${PLAYWRIGHT_CLI_VERSION}"
    ln -sf "$tools/pwcli/node_modules/.bin/playwright-cli" "$tools/bin/playwright-cli"
  fi
  # playwright-cli bundles its own Playwright, so it installs its own browser revision.
  local browser; browser="$(bash "$SKILL_SRC/scripts/run.sh" browser "$(cat "$JOB/config-path")")"
  if sudo -n true 2>/dev/null; then playwright-cli install-browser "$browser" --with-deps
  else playwright-cli install-browser "$browser"; fi
  # ffmpeg only turns recordings into GIFs; without it the skill captures screenshots instead.
  if ! command -v ffmpeg >/dev/null; then
    if sudo -n true 2>/dev/null && command -v apt-get >/dev/null; then
      { sudo -n apt-get update -qq && sudo -n apt-get install -y -qq ffmpeg; } >/dev/null || log "ffmpeg install failed; recordings fall back to screenshots"
    else
      log "ffmpeg unavailable; recordings fall back to screenshots"
    fi
  fi
  npm install --silent --no-audit --no-fund --prefix "$tools/claude" "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}"
  ln -sf "$tools/claude/node_modules/.bin/claude" "$tools/bin/claude"
}

# Boot the app once and request INPUT_WARMUP_PATH, so the first compile does not eat into the
# skill's readiness poll. Best effort: Claude still boots and checks the app itself.
cmd_warmup() {
  local port boot origin pid ok=false
  port="$(cat "$JOB/port")"
  boot="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("boot",""))' "$JOB/config.json")"
  origin="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("testOrigin",""))' "$JOB/config.json")"
  [ -n "$boot" ] && [ -n "$origin" ] || { log "warm-up skipped: config has no boot or testOrigin"; return 0; }
  # Own process group, so the whole boot tree (e.g. a dev server it spawns) stops with it.
  if command -v setsid >/dev/null; then PORT="$port" setsid bash -c "$boot" > "$JOB/warmup.log" 2>&1 &
  else PORT="$port" bash -c "$boot" > "$JOB/warmup.log" 2>&1 & fi
  pid=$!
  local deadline=$((SECONDS + ${PR_EVIDENCE_WARMUP_SECONDS:-900}))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if curl -sf -o /dev/null --max-time "$((deadline - SECONDS))" -L "${origin%/}/${INPUT_WARMUP_PATH#/}"; then ok=true; break; fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 2
  done
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  stop_port "$port"
  if [ "$ok" = true ]; then log "warm-up answered on $origin"; else log "warm-up: the app did not answer"; tail -n 40 "$JOB/warmup.log" >&2; fi
}

# Render prompt.md and run Claude with the skill. Claude publishes through the skill's run.sh.
cmd_capture() {
  local pr host publish instructions token
  pr="$(pr_number)"
  host="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["host"]["target"])' "$JOB/config.json")"
  if [ "$host" = github-attachment ]; then
    token="$ATTACH_TOKEN"
    publish="Post exactly one comment with \`$JOB/claude/skills/pr-evidence/scripts/run.sh comment $pr <body-file> <files...>\`, referencing each file by its local path in the body (GitHub replaces the paths with the uploaded attachments)."
  else
    token="$GITHUB_TOKEN_INPUT"
    publish="1. Host the media first: \`$JOB/claude/skills/pr-evidence/scripts/run.sh host --config $(cat "$JOB/config-path") pr-$pr <files...>\` prints a base URL; each file is \`<base URL>/<file name>\`.
2. Embed those URLs in the body file, then post exactly one comment with \`$JOB/claude/skills/pr-evidence/scripts/run.sh comment $pr <body-file>\` and NO file arguments (file arguments switch to native attachments, which this token cannot upload)."
  fi
  instructions=""
  [ -n "${INPUT_INSTRUCTIONS:-}" ] && instructions="Before booting, read \`$INPUT_INSTRUCTIONS\`: it is this repository's own guidance on what to capture and how (personas, data, routes, screenshot or recording), and it wins over this prompt on those. The environment, script paths and publishing steps below always apply, even where that document names other paths."
  P_PR_NUMBER="$pr" P_REPO="$GITHUB_REPOSITORY" P_SKILL_DIR="$JOB/claude/skills/pr-evidence" \
  P_CHANGED_FILE="$JOB/changed-files.txt" P_CONFIG="$(cat "$JOB/config-path")" \
  P_PUBLISH="$publish" P_INSTRUCTIONS="$instructions" \
    python3 - "$ACTION_DIR/prompt.md" "$JOB/prompt.md" <<'PY'
import os, re, sys
t = open(sys.argv[1]).read()
t = re.sub(r"\{\{([A-Z_]+)\}\}", lambda m: os.environ["P_" + m.group(1)], t)
open(sys.argv[2], "w").write(t)
PY
  # git-branch commits as github-actions[bot] and pushes through origin with github-token, scoped
  # to Claude's commands by env. The empty first value resets any header actions/checkout left in
  # the repo config, so git sends one Authorization header, not two.
  local basic; basic="$(printf 'x-access-token:%s' "$GITHUB_TOKEN_INPUT" | base64 | tr -d '\n')"
  echo "::add-mask::$basic"
  export GIT_AUTHOR_NAME="github-actions[bot]" GIT_COMMITTER_NAME="github-actions[bot]"
  export GIT_AUTHOR_EMAIL="41898282+github-actions[bot]@users.noreply.github.com"
  export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
  # One credential reaches Claude: the API key when given (billed per use), else the OAuth token
  # (a Claude subscription). An empty variable is unset, so the CLI never sees a blank credential.
  if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] && log "both claude-token and anthropic-api-key are set; using the API key"
    unset CLAUDE_CODE_OAUTH_TOKEN
  else
    unset ANTHROPIC_API_KEY
  fi
  set -o pipefail
  # shellcheck disable=SC2086 # INPUT_CLAUDE_ARGS is a list of arguments
  GH_TOKEN="$token" PORT="$(cat "$JOB/port")" SESS="$(cat "$JOB/session")" \
  CLAUDE_CONFIG_DIR="$JOB/claude" \
  GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0="http.https://github.com/.extraheader" GIT_CONFIG_VALUE_0="" \
  GIT_CONFIG_KEY_1="http.https://github.com/.extraheader" GIT_CONFIG_VALUE_1="AUTHORIZATION: basic $basic" \
    claude -p --max-turns "$INPUT_MAX_TURNS" \
      --allowedTools Skill,Bash,Read,Write,Edit,Grep,Glob ${INPUT_CLAUDE_ARGS:-} \
      < "$JOB/prompt.md" 2>&1 | tee "$JOB/claude.log"
}

# Why nothing was posted, for the job summary: the capture never started when setup failed.
not_posted_reason() {
  if [ -f "$JOB/claude.log" ]; then printf 'the evidence run ended without posting a result'
  else printf "the action's setup failed before the capture started"; fi
}

# Post "evidence pending" unless this run already reported (posted or pending).
cmd_fallback() {
  local since; since="$(cat "$JOB/started-at" 2>/dev/null || echo 1970-01-01T00:00:00Z)"
  if own_comment_bodies "$since" | grep -qE 'pr-evidence:(posted|pending)'; then
    log "the run already reported"; return 0
  fi
  local run_url="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
  printf '%s\n%s\n%s\n' "$PENDING_MARKER" \
    "**Evidence pending** — the evidence run ended without posting a result ([run log]($run_url))." \
    "Re-run the job or add the \`$INPUT_LABEL\` label to retry." > "$JOB/pending.md"
  gh pr comment "$(pr_number)" --repo "$GITHUB_REPOSITORY" --body-file "$JOB/pending.md"
}

# The run's page on GitHub says whether evidence was posted, with a link to it. GitHub caps a step's
# summary at 1 MiB.
cmd_summary() {
  [ -n "${GITHUB_STEP_SUMMARY:-}" ] || return 0
  local since urls url="" status
  since="$(cat "$JOB/started-at" 2>/dev/null || echo 1970-01-01T00:00:00Z)"
  if ! urls="$(own_comments "select(.body | contains(\"$POSTED_MARKER\")) | .html_url" "$since")"; then
    status="⚠️ unknown: could not list the pull request's comments"
  else
    url="$(printf '%s\n' "$urls" | tail -n 1)"
    if [ -n "$url" ]; then status="✅ posted"; else status="⏳ not posted: $(not_posted_reason)"; fi
  fi
  {
    echo "### PR evidence — PR #$(pr_number) · $status"
    if [ -n "$url" ]; then printf '\n[See the evidence on the pull request](%s)\n' "$url"; fi
  } | head -c 900000 >> "$GITHUB_STEP_SUMMARY"
  return 0
}

stop_port() { # PORT: stop whatever listens on it (the port is unique on the host for this job)
  if command -v fuser >/dev/null; then fuser -k -TERM "$1/tcp" 2>/dev/null || true
  elif command -v lsof >/dev/null; then lsof -t -iTCP:"$1" -sTCP:LISTEN | xargs -r kill 2>/dev/null || true
  fi
}

# Close only this job's browser session (never kill-all: jobs may share the host), stop the app,
# restore the config file, remove the label and remove the job directory. The label is a one-shot
# request: removing it lets adding it again start a new run.
cmd_cleanup() {
  if [ -n "${INPUT_LABEL:-}" ]; then
    local label err
    label="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$INPUT_LABEL")"
    if ! err="$(gh api -X DELETE "repos/$GITHUB_REPOSITORY/issues/$(pr_number)/labels/$label" 2>&1 >/dev/null)"; then
      case "$err" in
        *"HTTP 404"*) log "label '$INPUT_LABEL' was not on the pull request" ;;
        *) log "could not remove the label '$INPUT_LABEL': $err" ;;
      esac
    fi
  fi
  [ -f "$JOB/session" ] && PATH="$JOB/tools/bin:$PATH" playwright-cli -s="$(cat "$JOB/session")" close >/dev/null 2>&1
  [ -f "$JOB/port" ] && stop_port "$(cat "$JOB/port")"
  if [ -f "$JOB/config-path" ]; then
    local cfg; cfg="$(cat "$JOB/config-path")"
    if [ -f "$JOB/config.original.json" ]; then cp "$JOB/config.original.json" "$cfg"; else rm -f "$cfg"; fi
  fi
  rm -rf "$JOB"
  return 0
}

case "${1:-}" in
  gate) cmd_gate ;;
  prepare) cmd_prepare ;;
  install) cmd_install ;;
  warmup) cmd_warmup ;;
  capture) cmd_capture ;;
  fallback) cmd_fallback ;;
  summary) cmd_summary ;;
  cleanup) cmd_cleanup ;;
  *) echo "usage: run.sh gate|prepare|install|warmup|capture|fallback|summary|cleanup" >&2; exit 2 ;;
esac
