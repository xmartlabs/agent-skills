#!/usr/bin/env bash
# Fails a pull request that adds a script under plugins/tribunal outside rust/, or makes
# workflow-src/tribunal.js or templates/check.sh longer. plugins/tribunal/CLAUDE.md says why.
#
#   BASE=<base sha> HEAD=<head sha> [EXCEPTION=true] .github/scripts/tribunal-guard.sh
#
# EXCEPTION=true (the tribunal-script-exception label) prints what it found and passes.
set -euo pipefail
: "${BASE:?BASE is the base commit of the pull request}" "${HEAD:?HEAD is the head commit of the pull request}"
P=plugins/tribunal
problems=()

while IFS= read -r f; do
  case "$f" in
    *.js | *.mjs | *.cjs | *.ts | *.sh | *.bash | *.py) problems+=("adds $f") ;;
    *) if [ "$(git show "$HEAD:$f" | head -c 2)" = '#!' ]; then problems+=("adds $f, which starts with #!"); fi ;;
  esac
done < <(git diff --diff-filter=A --name-only "$BASE...$HEAD" -- "$P" ":(exclude)$P/rust")

for f in "$P/workflow-src/tribunal.js" "$P/templates/check.sh"; do
  read -r added deleted _ < <(git diff --numstat "$BASE...$HEAD" -- "$f") || continue
  if [ "$added" -gt "$deleted" ]; then problems+=("grows $f by $((added - deleted)) lines"); fi
done

if [ ${#problems[@]} -eq 0 ]; then
  echo "No new scripts under $P outside rust/."
  exit 0
fi
printf '%s\n' "${problems[@]}"
if [ "${EXCEPTION:-false}" = true ]; then
  echo "::notice::The tribunal-script-exception label is set, so this passes."
  exit 0
fi
echo "::error::New tribunal work goes in the Rust engine (plugins/tribunal/CLAUDE.md). Move it under $P/rust/, or ask a maintainer for the tribunal-script-exception label and say why in the description."
exit 1
