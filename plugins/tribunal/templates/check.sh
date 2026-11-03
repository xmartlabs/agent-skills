#!/usr/bin/env bash
# Checks a review profile and its rules file with tribunal's real loaders.
#
#   plugins/tribunal/templates/check.sh                             # the shipped templates
#   plugins/tribunal/templates/check.sh <profile.json> [<rules.md>] # your repo's own files
#
# Needs python3 and the tribunal binary, which bin/tribunal-engine downloads on first use. Past
# that it reads no network: the base ref is passed in. One line per check, exit 1 on any failure.
set -uo pipefail
HERE=$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ENGINE="$(dirname "$HERE")/bin/tribunal-engine"

if [ $# -ge 1 ]; then
  abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
  PROFILES=("$(abs "$1")"); RULES=""; [ -n "${2:-}" ] && RULES=$(abs "$2")
else
  PROFILES=("$HERE"/review-profile.{backend,frontend,e2e,python}.json); RULES="$HERE/review-rules.md"
fi

fail=0
say() { printf '%-9s %-44s %s\n' "$1" "$2" "$3"; }
field() { python3 -c "import sys,json;d=json.loads(sys.stdin.read().strip().splitlines()[-1]);v=d.get('$1');print('' if v is None else v)" 2>/dev/null; }

for P in "${PROFILES[@]}"; do
  [ -f "$P" ] || { say FAIL "$P" "no such file"; fail=1; continue; }
  REPO=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('repo',''))" "$P")
  KIND=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('reviewer',''))" "$P")
  NAMED=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('projectRules') or '')" "$P")
  RULEARGS=(); [ -n "$RULES" ] && RULEARGS=(--rules "$RULES")
  [ -n "$NAMED" ] && [ -z "$RULES" ] && { say FAIL "$(basename "$P")" "names projectRules $NAMED but no rules file was given"; fail=1; continue; }
  out=$("$ENGINE" run -- load-profile --pr 1 --repo "$REPO" --ref main --profile "$P" ${RULEARGS[@]+"${RULEARGS[@]}"} 2>&1)
  if [ "$(printf '%s' "$out" | field ok)" = "True" ]; then say ok "tribunal loads $(basename "$P")" "$(printf '%s' "$out" | field warnings | sed 's/^\[\]$//')"
  else say FAIL "tribunal loads $(basename "$P")" "$(printf '%s' "$out" | field error || printf '%s' "$out" | tail -1)"; fail=1; fi
  if [ -n "$RULES" ]; then
    out=$("$ENGINE" run -- assemble-methodology --kind "$KIND" --rules "$RULES" --out "${TMPDIR:-/tmp}/tribunal-check-$KIND.md" 2>&1)
    if [ "$(printf '%s' "$out" | field ok)" != "True" ]; then say FAIL "tribunal assembles $KIND rules" "$(printf '%s' "$out" | field error || printf '%s' "$out" | tail -1)"; fail=1
    elif [ "$(printf '%s' "$out" | field unknown)" != "[]" ]; then
      say FAIL "tribunal assembles $KIND rules" "a block names no slot: $(printf '%s' "$out" | field unknown)"; fail=1
    else say ok "tribunal assembles $KIND rules" "filled: $(printf '%s' "$out" | field filled)"; fi
  fi
done
exit $fail
