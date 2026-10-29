#!/usr/bin/env bash
# comment-github.sh [--dry-run] PR_NUMBER BODY_FILE [FILES...]
set -u
DRY=""; [ "${1:-}" = "--dry-run" ] && { DRY=1; shift; }
[ "$#" -ge 2 ] || { echo 'usage: comment-github.sh [--dry-run] PR BODY_FILE [FILES...]' >&2; exit 2; }
PR="$1"; BODY="$2"; shift 2
[ -f "$BODY" ] || { echo "body file not found: $BODY" >&2; exit 1; }
args=(pr comment "$PR" --body-file "$BODY")
for file in "$@"; do
  [ -f "$file" ] || { echo "attachment not found: $file" >&2; exit 1; }
  args+=(--attach "$file")
done
if [ -n "$DRY" ]; then printf '%q ' gh "${args[@]}"; printf '\n'; exit 0; fi
if ! gh --version | python3 -c 'import re,sys; m=re.search(r"gh version (\d+)\.(\d+)\.(\d+)",sys.stdin.read()); sys.exit(0 if m and tuple(map(int,m.groups())) >= (2,99,0) else 1)'; then
  echo 'gh >= 2.99.0 required; update GitHub CLI (Homebrew: brew upgrade gh) — evidence pending' >&2
  exit 1
fi
if [ "$#" -gt 0 ] && ! gh pr comment --help | grep -q -- '--attach'; then
  echo 'gh lacks --attach support; update GitHub CLI — evidence pending' >&2
  exit 1
fi
gh "${args[@]}"
