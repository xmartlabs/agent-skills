#!/usr/bin/env bash
# host-user-attachments.sh TICKET FILE...  -> prints one "basename<TAB>url" line per file.
#
# Emits GitHub user-attachments CDN URLs (https://github.com/user-attachments/assets/<uuid>) that
# render inline in issue/PR comments EVEN ON PRIVATE REPOS — unlike a repo-file/raw URL, which a
# non-collaborator (or GitHub's image proxy) can't fetch. This is the right host target for
# client/private work where the git-branch builtin's raw URLs won't render for everyone.
#
# It wraps the `gh image` extension (github.com/drogers0/gh-image), which performs the web-UI
# drag-and-drop upload flow (/upload/policies/assets -> S3 -> finalize) using a browser
# `user_session` cookie and returns the user-attachments URL. That cookie is only available in an
# interactive / agent-driven context: a TOKEN-ONLY CI runner CANNOT use this target (there is no
# public API for it — see flow doc Stage 5). Wrapping a dedicated, updatable extension keeps the
# fragile, undocumented upload flow OUT of this kit.
#
# Unlike host-git-branch.sh (which prints a single base URL the caller appends filenames to),
# user-attachments assets are opaque per-file UUIDs, so this prints an explicit file->url MAPPING.
#
# FAIL-CLOSED by contract (flow doc Stage 5): on ANY failure — no extension, no session, an upload
# error, a parse/count mismatch — this prints NOTHING to stdout and exits non-zero, so the caller
# reports "evidence pending" rather than emitting a broken/missing link.
set -u
TICKET="${1:-}"; [ "$#" -gt 0 ] && shift   # accepted for signature parity; not needed here
: "${TICKET:=}"

fail() { echo "host-user-attachments: $1" >&2; exit 1; }

command -v gh >/dev/null 2>&1 || fail "gh not found on PATH"
# Probe the extension without triggering an upload or an auth prompt.
gh image --help >/dev/null 2>&1 \
  || fail "the 'gh image' extension is not installed (run: gh extension install drogers0/gh-image)"

[ "$#" -gt 0 ] || fail "no artifact files to host"
for f in "$@"; do [ -f "$f" ] || fail "missing file '$f'"; done

# One `gh image` call uploads all files and prints one reference line per file, in input order.
out="$(gh image "$@" 2>/dev/null)" || fail "gh image upload failed"

# Extract the user-attachments URL from each output line. gh image renders images as
# `![name](URL)`, videos as a bare URL, and other files as `[name](URL)` — so pull the URL
# token directly rather than assuming a markdown-link shape.
urls=()
while IFS= read -r line; do
  url="$(printf '%s\n' "$line" | grep -oE 'https://github\.com/user-attachments/assets/[^ )]+' | head -1)"
  [ -n "$url" ] && urls+=("$url")
done <<EOF
$out
EOF

[ "${#urls[@]}" -eq "$#" ] || fail "expected $# url(s) from 'gh image', parsed ${#urls[@]}"

i=0
for f in "$@"; do
  printf '%s\t%s\n' "$(basename "$f")" "${urls[$i]}"
  i=$((i + 1))
done
