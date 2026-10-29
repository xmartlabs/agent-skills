#!/usr/bin/env bash
# Exercises the default builtin host target (scripts/host-git-branch.sh) against a local
# bare repo fixture — no network. url.insteadOf lets us keep a github.com remote URL (so the
# slug/URL logic is exercised for real) while fetch/push actually hit the local bare repo.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
HOST="$DIR/skills/pr-evidence/scripts/host-git-branch.sh"
echo "host_git_branch_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
git init -q --bare "$TMP/remote.git"
git init -q "$TMP/src"
cd "$TMP/src" || exit 1
git config user.email t@t; git config user.name t
git remote add origin https://github.com/org/repo.git
git config url."$TMP/remote.git".insteadOf https://github.com/org/repo.git
printf 'hi\n' > README; git add README; git commit -q -m init
git push -q origin HEAD:refs/heads/main
git remote set-head origin main >/dev/null 2>&1 || true
printf 'png-bytes\n' > "$TMP/shot.png"

is_nonzero() { [ "$1" -ne 0 ]; }

# 1. Branch does not exist yet -> creates it; prints a github raw URL with a real 40-hex SHA.
OUT="$(bash "$HOST" pr-evidence origin PC-1 "$TMP/shot.png" 2>/dev/null)"; RC=$?
assert_eq "new-branch host exits 0" "0" "$RC"
assert_ok "new-branch url is github raw + 40-hex sha" \
  bash -c 'printf "%s" "$1" | grep -qE "^https://github\.com/org/repo/raw/[0-9a-f]{40}/\.pr-evidence/PC-1/"' _ "$OUT"
# The new branch is an orphan holding only evidence, not a copy of the repo it was created from.
hosted_files() { git --git-dir="$TMP/remote.git" ls-tree -r --name-only pr-evidence; }
assert_eq "new branch holds the evidence" "1" "$(hosted_files | grep -c '^\.pr-evidence/PC-1/')"
assert_eq "new branch has no repo files" "" "$(hosted_files | grep -v '^\.pr-evidence/')"
assert_eq "new branch has no parent commit" "" "$(git --git-dir="$TMP/remote.git" rev-parse --verify -q pr-evidence^)"

# 2. Branch now exists -> a second run appends and still prints a valid URL.
OUT2="$(bash "$HOST" pr-evidence origin PC-2 "$TMP/shot.png" 2>/dev/null)"; RC2=$?
assert_eq "existing-branch host exits 0" "0" "$RC2"
assert_ok "existing-branch url valid" \
  bash -c 'printf "%s" "$1" | grep -qE "raw/[0-9a-f]{40}/\.pr-evidence/PC-2/"' _ "$OUT2"
assert_eq "existing branch keeps earlier evidence" "1" "$(hosted_files | grep -c '^\.pr-evidence/PC-1/')"
assert_eq "existing branch gains the new evidence" "1" "$(hosted_files | grep -c '^\.pr-evidence/PC-2/')"
assert_eq "existing branch still has no repo files" "" "$(hosted_files | grep -v '^\.pr-evidence/')"

# 3. Token-in-URL remote (common CI/bot form) must NOT leak the token into the printed URL.
git remote set-url origin "https://x-access-token:SECRETTOKEN@github.com/org/repo.git"
git config url."$TMP/remote.git".insteadOf "https://x-access-token:SECRETTOKEN@github.com/org/repo.git"
OUT3="$(bash "$HOST" pr-evidence origin PC-3 "$TMP/shot.png" 2>/dev/null)"; RC3=$?
assert_eq "token-remote host exits 0" "0" "$RC3"
assert_fail "token absent from url" bash -c 'printf "%s" "$1" | grep -q SECRETTOKEN' _ "$OUT3"
assert_ok "token-remote url is clean github raw" \
  bash -c 'printf "%s" "$1" | grep -qE "^https://github\.com/org/repo/raw/"' _ "$OUT3"

# 4. All FILE args missing -> fail closed: non-zero exit AND no URL on stdout.
OUT4="$(bash "$HOST" pr-evidence origin PC-4 "$TMP/nope.png" 2>/dev/null)"; RC4=$?
assert_ok "missing-file host exits non-zero" is_nonzero "$RC4"
assert_eq "missing-file host prints no url" "" "$OUT4"

# 5. Duplicate artifact basenames -> fail closed rather than silently dropping one.
mkdir -p "$TMP/a" "$TMP/b"; printf '1\n' > "$TMP/a/dup.png"; printf '2\n' > "$TMP/b/dup.png"
OUT5="$(bash "$HOST" pr-evidence origin PC-5 "$TMP/a/dup.png" "$TMP/b/dup.png" 2>/dev/null)"; RC5=$?
assert_ok "collision host exits non-zero" is_nonzero "$RC5"
assert_eq "collision host prints no url" "" "$OUT5"

# 6. Path-traversal ticket -> rejected before touching git.
OUT6="$(bash "$HOST" pr-evidence origin "../evil" "$TMP/shot.png" 2>/dev/null)"; RC6=$?
assert_ok "traversal ticket rejected" is_nonzero "$RC6"
assert_eq "traversal ticket prints no url" "" "$OUT6"

# 7. Push failure (unreachable remote) -> fail closed: non-zero exit AND no URL.
git init -q "$TMP/src2"; cd "$TMP/src2" || exit 1
git config user.email t@t; git config user.name t
printf 'hi\n' > README; git add README; git commit -q -m init
git remote add origin https://github.com/org/repo.git
git config url."$TMP/nonexistent.git".insteadOf https://github.com/org/repo.git
OUT7="$(bash "$HOST" pr-evidence origin PC-7 "$TMP/shot.png" 2>/dev/null)"; RC7=$?
assert_ok "push-fail host exits non-zero" is_nonzero "$RC7"
assert_eq "push-fail host prints no url" "" "$OUT7"

# 8. Non-github remote -> the git-branch builtin can't emit a raw URL; fail closed.
git remote set-url origin "https://gitlab.com/org/repo.git"
git config url."$TMP/remote.git".insteadOf https://gitlab.com/org/repo.git
OUT8="$(bash "$HOST" pr-evidence origin PC-8 "$TMP/shot.png" 2>/dev/null)"; RC8=$?
assert_ok "non-github remote exits non-zero" is_nonzero "$RC8"
assert_eq "non-github remote prints no url" "" "$OUT8"

finish_tests
