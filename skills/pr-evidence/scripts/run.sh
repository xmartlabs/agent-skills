#!/usr/bin/env bash
# pr-evidence orchestrator. Deterministic plumbing; capture is agent-driven (see the skill).
set -u
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/scripts/lib/origin.sh"
CONFIG_DEFAULT=".pr-evidence/config.json"

cfg_get() { # cfg_get DOTTED_KEY CONFIG_PATH
  python3 - "$2" "$1" <<'PY'
import json,sys
try:
    d=json.load(open(sys.argv[1]))
except Exception:
    sys.exit(1)
cur=d
for k in sys.argv[2].split('.'):
    if isinstance(cur,dict) and k in cur: cur=cur[k]
    else: sys.exit(1)
if cur is None: sys.exit(1)   # a JSON null means "unset" -> let the caller apply its default
print(cur if not isinstance(cur,(dict,list)) else json.dumps(cur))
PY
}

cmd_check() {
  local cfg="${1:-$CONFIG_DEFAULT}"
  [ -f "$cfg" ] || { echo "config not found: $cfg" >&2; return 1; }
  # Mobile configs have no URL origin; guard the build variant + API base instead (see
  # flow/mobile-evidence-flow.md). Web (the default) keeps the testOrigin check unchanged.
  local platform; platform="$(cfg_get platform "$cfg" 2>/dev/null || true)"
  case "$platform" in
    ios|android) cmd_check_mobile "$cfg" "$platform"; return $? ;;
  esac
  local origin; origin="$(cfg_get testOrigin "$cfg")" || { echo "config invalid or missing testOrigin: $cfg" >&2; return 1; }
  if ! is_test_origin "$origin"; then
    echo "refusing to run: testOrigin '$origin' is not a local/test origin" >&2; return 1
  fi
  printf '%s\n' "$origin"
}

# Mobile guard: no origin to check, so validate config intent — the build must be declared debug,
# and any declared apiBase must not be a forbidden (production) host. This is a WEAKER guarantee
# than the web origin check (it validates declared intent, not what the binary actually talks to),
# so the target must still be a seeded, non-production backend. Prints "mobile:<platform>" on pass.
cmd_check_mobile() {
  local cfg="$1" platform="$2"
  python3 - "$cfg" <<'PY' || return 1
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.stderr.write("config invalid or unparseable\n"); sys.exit(1)
g = d.get("guard") or {}
if g.get("requireDebugBuild") is not True:
    sys.stderr.write("refusing to run: mobile config must set guard.requireDebugBuild: true\n"); sys.exit(1)
base = (g.get("apiBase") or "").strip()
forbid = [f for f in (g.get("forbidApiHosts") or []) if f]
if base:
    from urllib.parse import urlparse
    host = urlparse(base if "://" in base else "//" + base).hostname or base
    for f in forbid:
        if host == f or host.endswith("." + f):
            sys.stderr.write("refusing to run: apiBase host '%s' is a forbidden production host\n" % host); sys.exit(1)
sys.exit(0)
PY
  printf 'mobile:%s\n' "$platform"
}

cmd_host() {
  local cfg="$CONFIG_DEFAULT" dry=""
  # Order-independent flag parsing: --config / --dry-run may appear in any order before the
  # positional TICKET and FILE args (a positional-only parser silently mis-assigned them).
  while [ $# -gt 0 ]; do
    case "$1" in
      --config) cfg="$2"; shift 2 ;;
      --dry-run) dry=1; shift ;;
      --) shift; break ;;
      --*) echo "host: unknown flag '$1'" >&2; return 2 ;;
      *) break ;;
    esac
  done
  # Warn loudly if the config file is present but unparseable — otherwise every cfg_get below
  # silently falls back to hardcoded defaults, hiding the user's real (broken) config.
  if [ -f "$cfg" ] && ! cfg_get testOrigin "$cfg" >/dev/null 2>&1; then
    echo "host: warning — config '$cfg' is missing or unparseable; using built-in defaults" >&2
  fi
  local ticket="${1:-}"; [ $# -gt 0 ] && shift
  local target; target="$(cfg_get host.target "$cfg" 2>/dev/null || echo git-branch)"
  if [ "$target" = "github-attachment" ]; then
    echo "github-attachment: skip host; pass local files to run.sh comment PR BODY_FILE FILES..." >&2
    return 1
  fi
  if [ "$target" != "git-branch" ] && [ "$target" != "custom" ] && [ "$target" != "user-attachments" ]; then
    echo "unknown host target: $target" >&2; return 1
  fi
  if [ "$target" = "custom" ]; then
    local up; up="$(cfg_get host.upload "$cfg" 2>/dev/null || true)"
    [ -n "$up" ] || { echo "host: host.target is 'custom' but host.upload is missing/empty" >&2; return 1; }
    [ -n "$dry" ] && { echo "$up"; return 0; }
    eval "$up"; return $?
  fi
  if [ "$target" = "user-attachments" ]; then
    # Renders inline on private repos; needs a browser session (agent-driven only). Prints a
    # per-file "basename<TAB>url" MAPPING, not a single base URL — see the script header.
    [ -n "$dry" ] && { echo "host-user-attachments.sh $ticket $*"; return 0; }
    bash "$DIR/scripts/host-user-attachments.sh" "$ticket" "$@"; return $?
  fi
  local branch remote
  branch="$(cfg_get host.branch "$cfg" 2>/dev/null || echo pr-evidence)"
  remote="$(cfg_get host.remote "$cfg" 2>/dev/null || echo origin)"
  [ -n "$dry" ] && { echo "host-git-branch.sh $branch $remote $ticket $*"; return 0; }
  bash "$DIR/scripts/host-git-branch.sh" "$branch" "$remote" "$ticket" "$@"
}
cmd_browser() { # prints the playwright-cli --browser value; defaults to chrome
  local cfg="${1:-$CONFIG_DEFAULT}"
  [ -f "$cfg" ] || { echo "config not found: $cfg" >&2; return 1; }
  local browser; browser="$(cfg_get browser "$cfg" 2>/dev/null || echo chrome)"
  case "$browser" in
    chrome|chromium|msedge|firefox|webkit) printf '%s\n' "$browser" ;;
    *) echo "unsupported browser '$browser' (use chrome, chromium, msedge, firefox or webkit)" >&2; return 1 ;;
  esac
}
cmd_comment() {
  local dry=""; [ "${1:-}" = "--dry-run" ] && { dry=1; shift; }
  if [ -n "$dry" ]; then bash "$DIR/scripts/comment-github.sh" --dry-run "$@"; else bash "$DIR/scripts/comment-github.sh" "$@"; fi
}

case "${1:-}" in
  check) shift; cmd_check "$@" ;;
  host) shift; cmd_host "$@" ;;
  comment) shift; cmd_comment "$@" ;;
  browser) shift; cmd_browser "$@" ;;
  *) echo "usage: run.sh check|host|comment|browser [args]" >&2; exit 2 ;;
esac
