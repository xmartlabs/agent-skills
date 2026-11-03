#!/usr/bin/env bash
# release.sh — freeze a version of tribunal: record, bump, commit, tag, push.
#
# WHY A SCRIPT AND NOT A CHECKLIST. The order is load-bearing and two of its
# steps are easy to get backwards:
#
#   * `release.mjs --record` must run BEFORE the commit, because the row it
#     appends to RELEASES.md is part of what the commit freezes. Commit first
#     and the commit does not contain its own record.
#   * `git push` does not push tags. Without `push.followTags` set — and it is
#     not set here — the tag stays local, so the freeze is unreachable to
#     everyone else while looking published to you.
#
#   * The notes for the row have to be written BEFORE the record, because the
#     record is what writes the row and the commit follows it immediately. This
#     script used to leave `see Known open` in that cell and print a reminder;
#     the reminder arrived after the commit, so every row was repaired by hand
#     afterwards or shipped as a placeholder. Step 0 below collects the text up
#     front and release.mjs refuses to record without it.
#
# The gate itself is release.mjs, which refuses to record on a red battery. This
# script adds no judgment; it only makes the sequence hard to perform wrongly.
#
# HOW VERSIONS MOVE. One semver number names the plugin, its tag and the
# binary it pins. A fix, docs or a template change is a patch (115.0.1); a
# feature or a new binary that breaks nothing is a minor (115.1.0); a removed
# flag or a profile that stops loading is a major (116.0.0). A freeze that pins
# a new tribunal-rust binary pins the one tagged with this same version
# (tribunal-rust-v115.1.0 ships in tribunal/v115.1.0); a freeze with no binary
# change keeps the previous pin. Versions only go up: the plugin cache keys
# updates off plugin.json, and a lower number there may never reach anyone.
# Freezes up to 115 took the next integer as N.0.0, and their binaries had a
# 0.x numbering of their own.
#
# Usage:
#   ./release.sh 115.1.0                   # freeze as tribunal/v115.1.0
#   ./release.sh 115.1.0 --dry-run         # print the plan, touch nothing
#   ./release.sh 115.1.0 --notes-file <path>   # take the row's notes from a file
#                                          # instead of opening $EDITOR
#   ./release.sh 115.1.0 --portable        # no test-data directory on this machine:
#                                          # run only the replays that need none,
#                                          # and say so in the row
set -euo pipefail

ROOT="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$ROOT/.claude-plugin/plugin.json"
# The release log is this repository's own and carries no client data, so it
# lives at the repo root. TRIBUNAL_RELEASES overrides it.
RELEASES_FILE="${TRIBUNAL_RELEASES:-$ROOT/RELEASES.md}"

VERSION="${1:-}"
DRY=""
NOTES_FILE=""
PORTABLE=""
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run)  DRY=1 ;;
    --portable) PORTABLE=--portable ;;
    --notes-file)
      NOTES_FILE="${2:-}"
      [ -n "$NOTES_FILE" ] || { printf '\033[31merror:\033[0m --notes-file needs a path\n' >&2; exit 1; }
      [ -f "$NOTES_FILE" ] || { printf '\033[31merror:\033[0m no such file: %s\n' "$NOTES_FILE" >&2; exit 1; }
      shift ;;
    *) printf '\033[31merror:\033[0m unknown argument: %s\n' "$1" >&2; exit 1 ;;
  esac
  shift
done

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

[ -n "$VERSION" ] || die "usage: ./release.sh <major.minor.patch> [--dry-run]   (e.g. 115.1.0)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be major.minor.patch, got '$VERSION'. The tag becomes tribunal/v<version>."

TAG="tribunal/v$VERSION"
SEMVER="$VERSION"

# 0 when $1 is a higher version than $2, comparing each part as a number.
semver_gt() {
  local a b i
  IFS=. read -r -a a <<<"$1"
  IFS=. read -r -a b <<<"$2"
  for i in 0 1 2; do
    [ "${a[i]:-0}" -gt "${b[i]:-0}" ] && return 0
    [ "${a[i]:-0}" -lt "${b[i]:-0}" ] && return 1
  done
  return 1
}
CURRENT=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$MANIFEST" | head -1)
semver_gt "$SEMVER" "$CURRENT" || die "$SEMVER is not higher than the current version $CURRENT. Versions only go up: /plugin update keys off this number."

cd "$ROOT"
git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"
BRANCH=$(git rev-parse --abbrev-ref HEAD)
REMOTE=$(git remote | head -1)
[ -n "$REMOTE" ] || die "no git remote configured"

# Refuse to reuse a tag. Moving one silently rewrites which tree a freeze names,
# which is the one thing a freeze log exists to prevent.
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  die "tag $TAG already exists locally — pick the next number, or delete it deliberately"
fi
if git ls-remote --tags "$REMOTE" "refs/tags/$TAG" | grep -q .; then
  die "tag $TAG already exists on $REMOTE — pick the next number"
fi

# Refuse to freeze a stale tree, but only when staleness can actually corrupt
# the freeze. A tag names a snapshot; cutting one from a base that is missing
# commits to THIS plugin tags a version whose contents nobody reviewed under
# that number. On 2026-09-11 local main sat 8 commits behind for most of a
# session, so this is the same shape as the `git add -A` below: a condition that
# only ever held by operator discipline.
#
# Being behind on commits that do NOT touch this plugin is normal in a repo
# several people push to, and dying on it would fire constantly and teach people
# to work around the check. Those are reported and pass.
DEFAULT_BRANCH=$(git symbolic-ref --quiet --short "refs/remotes/$REMOTE/HEAD" 2>/dev/null | sed "s|^$REMOTE/||" || true)
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
# The freeze commit lands through a pull request like any other change.
[ "$BRANCH" != "$DEFAULT_BRANCH" ] || die "freeze from a branch, not $DEFAULT_BRANCH: the commit reaches $DEFAULT_BRANCH through a pull request"
git fetch --quiet "$REMOTE" "$DEFAULT_BRANCH" \
  || die "could not fetch $REMOTE/$DEFAULT_BRANCH, so this tree's freshness is unknown. A freeze pushes; if the network is down, stop here anyway."
BEHIND=$(git rev-list --count "HEAD..$REMOTE/$DEFAULT_BRANCH")
if [ "$BEHIND" -gt 0 ]; then
  MISSING_HERE=$(git rev-list --count "HEAD..$REMOTE/$DEFAULT_BRANCH" -- "$ROOT")
  if [ "$MISSING_HERE" -gt 0 ]; then
    printf '\n' >&2
    git -c core.pager=cat log --oneline "HEAD..$REMOTE/$DEFAULT_BRANCH" -- "$ROOT" >&2
    die "$MISSING_HERE commit(s) above touch this plugin and are not in this tree. Freezing here tags a version that omits them. Update first:
     git merge $REMOTE/$DEFAULT_BRANCH"
  fi
  echo "   note: $BEHIND commit(s) behind $REMOTE/$DEFAULT_BRANCH, none touching $(basename "$ROOT"). Freezing anyway."
fi

# A new binary carries this freeze's version. The pin on the default branch is
# the previous freeze's; when this tree pins another one, it has to be
# tribunal-rust-v$SEMVER.
PIN_FILE="$ROOT/rust/release.json"
pin_of() { sed -n 's/^ *"tag": *"\([^"]*\)".*/\1/p' | head -1; }
PIN=$(pin_of < "$PIN_FILE")
PREV_PIN=$(git show "$REMOTE/$DEFAULT_BRANCH:$(git rev-parse --show-prefix)rust/release.json" 2>/dev/null | pin_of || true)
if [ "$PIN" != "$PREV_PIN" ] && [ "$PIN" != "tribunal-rust-v$SEMVER" ]; then
  die "this freeze pins $PIN, a new binary, so it must be tribunal-rust-v$SEMVER: tag the binary with the version of the freeze that first ships it"
fi

step "Plan"
cat <<PLAN
  version    $SEMVER   (manifest: .claude-plugin/plugin.json, now $CURRENT)
  binary     $PIN
  tag        $TAG
  branch     $BRANCH -> $REMOTE
  harness    $("$ROOT/bin/tribunal-engine" run -- harness-version --short)
PLAN

if [ -n "$DRY" ]; then
  step "Dry run — the gate below would run, nothing would be written"
  exec node "$ROOT/rust/scripts/release.mjs" $PORTABLE
fi

# 0. The notes, first, because the record writes them and the commit follows the
#    record with no chance to intervene. An editor rather than a `read` prompt:
#    every historical row is a paragraph, and a shell prompt that eats newlines
#    is how a one-line placeholder became the norm.
step "0/5  Notes for the RELEASES.md row"
if [ -z "$NOTES_FILE" ]; then
  NOTES_FILE=$(mktemp -t tribunal-release-notes)
  trap 'rm -f "$NOTES_FILE"' EXIT
  cat > "$NOTES_FILE" <<'TEMPLATE'

# What this freeze contains. This becomes the notes cell of the RELEASES.md row
# and it is the only part of the row a human writes, so it is the only part that
# can say anything the hash cannot.
#
# Lines starting with # are stripped. Newlines are collapsed into one cell, so
# write prose, not bullets. Say:
#   - what changed, concretely enough to find it again
#   - what was MEASURED versus what was reasoned
#   - what stayed open, and point at the Known open section if it needs room
#
# Leaving this empty aborts the freeze. That is deliberate: a row with nothing
# in this cell is the placeholder this step was written to stop shipping.
TEMPLATE
  "${EDITOR:-${VISUAL:-vi}}" "$NOTES_FILE"
fi
if ! grep -qv '^[[:space:]]*\(#.*\)\?$' "$NOTES_FILE"; then
  die "the notes are empty — nothing was written, so nothing was frozen"
fi
echo "   $(grep -cv '^[[:space:]]*\(#.*\)\?$' "$NOTES_FILE") line(s) of notes"

# 1. The gate. release.mjs exits non-zero on a red battery and refuses --record,
#    so this is the step that decides whether a freeze happens at all. The tag is
#    passed in because release.mjs cannot derive it: it does not exist until 4/5.
step "1/5  Recording the freeze (gate: Rust suite and binary pin)"
node "$ROOT/rust/scripts/release.mjs" $PORTABLE --record --tag "$TAG" --notes-file "$NOTES_FILE"

# The row is the artifact this whole script exists to produce, so verify it
# rather than trusting that the writer wrote it.
#
# Scoped to the row naming THIS tag, never the whole file: 15 of the 44 rows
# already in RELEASES.md end in the old `see Known open` placeholder, so a
# file-wide check would refuse every freeze forever. Those rows are history and
# are left as they are; this guard only keeps NEW ones honest.
NEW_ROW=$(grep -F "| \`$TAG\` |" "$RELEASES_FILE" || true)
[ -n "$NEW_ROW" ] || die "the new row does not name $TAG — check RELEASES.md before continuing"
case "$NEW_ROW" in
  *"see Known open |") die "the new row's notes cell is still the placeholder — refusing to freeze it" ;;
  *"| untagged |"*)    die "the new row is untagged — release.mjs did not receive --tag" ;;
esac


# 2. The manifest version is the cache key the team's updates key off. It has to
#    move with the tag or `/plugin update` reports "already at the latest
#    version" and nobody receives the freeze.
step "2/5  Bumping the manifest to $SEMVER"
node -e '
  const fs = require("fs");
  const p = process.argv[1], v = process.argv[2];
  const m = JSON.parse(fs.readFileSync(p, "utf8"));
  const before = m.version;
  m.version = v;
  fs.writeFileSync(p, JSON.stringify(m, null, 2) + "\n");
  console.log(`   ${before} -> ${v}`);
' "$MANIFEST" "$SEMVER"

step "3/5  Committing"
# Scoped to this plugin's subtree. This was `git add -A`, which stages the WHOLE
# worktree no matter the cwd, so any unrelated work in progress elsewhere in the
# repo lands inside the freeze commit. Forty-odd releases came out clean not
# because anything prevented that but because the tree happened to be clean each
# time; on 2026-09-11 the same command would have swept five unrelated paths.
# Nothing is silently dropped either — whatever stays out is printed below.
git add -- "$ROOT" "$RELEASES_FILE"
git -c core.pager=cat diff --cached --stat | tail -20

PLUGIN_PREFIX=$(git rev-parse --show-prefix)
REPO_TOP=$(git rev-parse --show-toplevel)
RELEASES_REL=$(cd "$(dirname "$RELEASES_FILE")" && git rev-parse --show-prefix)$(basename "$RELEASES_FILE")
OUTSIDE=$(git -C "$REPO_TOP" status --porcelain -- . ":(exclude)$PLUGIN_PREFIX" ":(exclude)$RELEASES_REL")
if [ -n "$OUTSIDE" ]; then
  printf '\n\033[33mleft out of this freeze\033[0m (outside %s):\n' "$PLUGIN_PREFIX"
  printf '%s\n' "$OUTSIDE" | sed 's/^/  /'
  echo "  If any of that belongs in the freeze, stage it yourself and re-run."
fi

printf '\nCommit these? [y/N] '
read -r ans; [ "$ans" = "y" ] || die "aborted before commit — nothing was pushed, but RELEASES.md and the manifest are already modified"
git commit -m "chore(tribunal): freeze $TAG"

step "4/5  Tagging"
git tag -a "$TAG" -m "tribunal $SEMVER: offline battery green at $("$ROOT/bin/tribunal-engine" run -- harness-version --short)"
echo "   $TAG -> $(git rev-parse --short "$TAG")"

step "5/5  Pushing (branch and tag are two separate pushes)"
printf 'Push %s and %s to %s? [y/N] ' "$BRANCH" "$TAG" "$REMOTE"
read -r ans
if [ "$ans" != "y" ]; then
  echo "   skipped. The commit and tag exist locally; push them with:"
  echo "     git push -u $REMOTE $BRANCH"
  echo "     git push $REMOTE $TAG"
  exit 0
fi
git push -u "$REMOTE" "$BRANCH"
git push "$REMOTE" "$TAG"

step "Done"
echo "   $TAG is on $REMOTE, and $BRANCH is pushed. NOT published yet."
echo "   The marketplace entry is \"source\": \"./plugins/tribunal\" — a path with no"
echo "   ref pinned — so installs track the default branch and the tag distributes"
echo "   nothing on its own. Teammates receive $SEMVER once this branch's manifest"
echo "   lands on main via PR, which is the one step this script cannot do."
