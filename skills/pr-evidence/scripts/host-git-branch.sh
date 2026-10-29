#!/usr/bin/env bash
# host-git-branch.sh BRANCH REMOTE TICKET FILE...  -> prints a SHA-pinned raw base URL.
#
# FAIL-CLOSED by contract (flow doc Stage 5): on ANY failure this prints nothing to stdout
# and exits non-zero, so the caller reports "evidence pending" rather than a broken link.
# It pushes evidence into a dedicated hosting branch via a DETACHED throwaway worktree — no
# persistent local branch is ever created, so a failed push can't leave stray state that
# breaks future runs.
set -u
BRANCH="$1"; REMOTE="$2"; TICKET="$3"; shift 3

fail() { echo "host-git-branch: $1" >&2; exit 1; }

# Reject a TICKET that could escape the evidence directory (path traversal / absolute path).
case "$TICKET" in
  ""|*/*|*..*) fail "invalid ticket '$TICKET' (must not be empty or contain '/' or '..')" ;;
esac

TS="$(git log -1 --format=%cd --date=format:%Y%m%d-%H%M%S 2>/dev/null || echo 00000000-000000)"
DEST=".pr-evidence/$TICKET/$TS"
WT="$(mktemp -d)/wt"

cleanup() { git worktree remove "$WT" --force >/dev/null 2>&1; git worktree prune >/dev/null 2>&1; }
trap cleanup EXIT

# Start the worktree at the current tip of the hosting branch (so evidence accumulates),
# falling back to the remote default and then local HEAD. Detached: creates no local branch.
git fetch "$REMOTE" "$BRANCH" --quiet 2>/dev/null || true
if git show-ref --verify --quiet "refs/remotes/$REMOTE/$BRANCH"; then START="$REMOTE/$BRANCH"
elif git show-ref --verify --quiet "refs/remotes/$REMOTE/HEAD"; then START="$REMOTE/HEAD"
else START="HEAD"; fi
git worktree add --detach "$WT" "$START" >/dev/null 2>&1 \
  || git worktree add --detach "$WT" >/dev/null 2>&1 \
  || fail "could not create worktree"

mkdir -p "$WT/$DEST" || fail "could not create evidence dir"
copied=0
for f in "$@"; do
  [ -f "$f" ] || { echo "host-git-branch: skipping missing file '$f'" >&2; continue; }
  base="$(basename "$f")"
  [ -e "$WT/$DEST/$base" ] && fail "duplicate artifact basename '$base' — would silently overwrite"
  cp "$f" "$WT/$DEST/$base" || fail "could not copy '$f'"
  copied=$((copied + 1))
done
[ "$copied" -gt 0 ] || fail "no artifact files to host"

# Commit in the detached worktree and push straight to the branch ref — no local branch.
git -C "$WT" add -f "$DEST" >/dev/null 2>&1 || fail "git add failed"
git -C "$WT" commit -q -m "$TICKET PR evidence (not for merge)" >/dev/null 2>&1 || fail "git commit failed"
git -C "$WT" push "$REMOTE" "HEAD:refs/heads/$BRANCH" -q >/dev/null 2>&1 || fail "git push failed"

# The pushed commit IS the worktree HEAD — resolve it directly (never a ref-name echo-back)
# and require a real 40-hex SHA before emitting a URL.
SHA="$(git -C "$WT" rev-parse --verify HEAD 2>/dev/null)" || fail "could not resolve commit SHA"
case "$SHA" in ""|*[!0-9a-f]*) fail "unexpected commit SHA '$SHA'" ;; esac
[ "${#SHA}" -eq 40 ] || fail "unexpected commit SHA length"

# Derive owner/repo from the remote. Strip scheme and userinfo (so a token in a
# https://x-access-token:TOKEN@github.com/... remote never leaks into the URL), then require
# github.com — the raw-URL form we emit is GitHub-specific.
# Read the literal configured URL (not `git remote get-url`, which applies url.insteadOf
# transport rewrites) — the owner/repo we want to link is the logical GitHub remote, not
# whatever mirror/proxy the fetch happens to use.
url="$(git config --get "remote.$REMOTE.url" 2>/dev/null)" || fail "could not read remote url"
[ -n "$url" ] || fail "remote '$REMOTE' has no configured url"
slug="$(printf '%s' "$url" | sed -E 's#^[a-zA-Z][a-zA-Z0-9+.-]*://##; s#^[^@/]*@##; s#\.git/?$##')"
case "$slug" in
  github.com[:/]*)
    slug="${slug#github.com}"
    slug="$(printf '%s' "$slug" | sed -E 's#^:[0-9]+/#/#; s#^[:/]##')"
    ;;
  *) fail "remote '$REMOTE' ($url) is not github.com — git-branch host only emits GitHub raw URLs" ;;
esac
[ -n "$slug" ] || fail "could not parse owner/repo from remote '$url'"

printf 'https://github.com/%s/raw/%s/%s\n' "$slug" "$SHA" "$DEST"
