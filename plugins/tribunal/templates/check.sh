#!/usr/bin/env bash
# Checks a review profile and its rules file with tribunal's real loaders.
#
#   plugins/tribunal/templates/check.sh                             # the shipped templates
#   plugins/tribunal/templates/check.sh <profile.json> [<rules.md>] # your repo's own files
#
# Needs node and python3. It reads no network: the base ref is passed in, so nothing is
# fetched from GitHub. It prints one line per check and exits 1 on any failure.
set -uo pipefail
HERE=$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PLUGIN=$(dirname "$HERE")

if [ $# -ge 1 ]; then
  abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
  PROFILES=("$(abs "$1")"); RULES=""; [ -n "${2:-}" ] && RULES=$(abs "$2")
else
  PROFILES=("$HERE/review-profile.backend.json" "$HERE/review-profile.frontend.json" "$HERE/review-profile.e2e.json"); RULES="$HERE/review-rules.md"
fi

fail=0
say() { printf '%-9s %-44s %s\n' "$1" "$2" "$3"; }
field() { python3 -c "import sys,json;d=json.loads(sys.stdin.read());v=d.get('$1');print('' if v is None else v)"; }

for P in "${PROFILES[@]}"; do
  [ -f "$P" ] || { say FAIL "$P" "no such file"; fail=1; continue; }
  REPO=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('repo',''))" "$P")
  KIND=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('reviewer',''))" "$P")
  NAMED=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('projectRules') or '')" "$P")
  RULEARGS=(); [ -n "$RULES" ] && RULEARGS=(--rules "$RULES")
  [ -n "$NAMED" ] && [ -z "$RULES" ] && { say FAIL "$(basename "$P")" "names projectRules $NAMED but no rules file was given"; fail=1; continue; }
  name=tribunal; dir=skills/review-pr
  {
    out=$(cd "$PLUGIN" && node "$dir/scripts/load-profile.mjs" --pr 1 --repo "$REPO" --ref main --profile "$P" ${RULEARGS[@]+"${RULEARGS[@]}"} 2>&1)
    if [ "$(printf '%s' "$out" | field ok)" = "True" ]; then say ok "$name loads $(basename "$P")" "$(printf '%s' "$out" | field warnings | sed 's/^\[\]$//')"
    else say FAIL "$name loads $(basename "$P")" "$(printf '%s' "$out" | field error)"; fail=1; fi
    if [ -n "$RULES" ]; then
      out=$(node "$PLUGIN/$dir/scripts/assemble-methodology.mjs" --kind "$KIND" --rules "$RULES" --out "${TMPDIR:-/tmp}/tribunal-check-$name.md" 2>&1)
      if [ "$(printf '%s' "$out" | field ok)" != "True" ]; then say FAIL "$name assembles $KIND rules" "$(printf '%s' "$out" | field error)"; fail=1
      elif [ "$(printf '%s' "$out" | python3 -c "import sys,json;print(len(json.loads(sys.stdin.read())['unknown']))")" != "0" ]; then
        say FAIL "$name assembles $KIND rules" "a block names no slot: $(printf '%s' "$out" | field unknown)"; fail=1
      else say ok "$name assembles $KIND rules" "filled: $(printf '%s' "$out" | field filled)"; fi
    fi
  }
done
exit $fail
