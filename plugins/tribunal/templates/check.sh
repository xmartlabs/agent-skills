#!/usr/bin/env bash
# Checks a review profile and its rules file with tribunal's real loaders.
#
#   plugins/tribunal/templates/check.sh                             # the shipped templates
#   plugins/tribunal/templates/check.sh <profile.json> [<rules.md>] # your repo's own files
#   plugins/tribunal/templates/check.sh --manifest <file>           # a conventionSources manifest
#
# Needs node and python3. It reads no network: the base ref is passed in, so nothing is
# fetched from GitHub. A profile with conventionSources is also run through
# load-conventions against a fake gh that serves a sample bundle
# (fake-gh-okf.mjs), which checks the sources parse and select, not that your
# docs repo is reachable. --manifest runs the loader's own manifest parser
# (convention-sources.mjs) on one file, the check a docs repo's CI can run
# before it commits a manifest. It prints one line per check and exits 1 on any
# failure.
set -uo pipefail
HERE=$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PLUGIN=$(dirname "$HERE")

say() { printf '%-9s %-44s %s\n' "$1" "$2" "$3"; }
field() { python3 -c "import sys,json;d=json.loads(sys.stdin.read());v=d.get('$1');print('' if v is None else v)"; }

if [ "${1:-}" = "--manifest" ]; then
  [ $# -eq 2 ] || { say FAIL "manifest" "usage: plugins/tribunal/templates/check.sh --manifest <file>"; exit 1; }
  out=$(node "$PLUGIN/skills/review-pr/scripts/convention-sources.mjs" --check-manifest "$2" 2>&1)
  if [ "$(printf '%s' "$out" | field ok 2>/dev/null)" = "True" ]; then say ok "manifest $2" "$(printf '%s' "$out" | field docs) docs"; exit 0; fi
  err=$(printf '%s' "$out" | field error 2>/dev/null) || err=$out
  say FAIL "manifest $2" "$err"; exit 1
fi

if [ $# -ge 1 ]; then
  abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
  PROFILES=("$(abs "$1")"); RULES=""; [ -n "${2:-}" ] && RULES=$(abs "$2")
else
  PROFILES=("$HERE/review-profile.backend.json" "$HERE/review-profile.frontend.json" "$HERE/review-profile.e2e.json"); RULES="$HERE/review-rules.md"
fi

fail=0

# A fake gh that answers conventionSources calls from the sample bundle, for
# any repo, ref or submodule path.
EVAL="$PLUGIN/skills/review-pr/eval"
OKF="${TMPDIR:-/tmp}/tribunal-check-okf"
mkdir -p "$OKF/bin" && ln -sf "$EVAL/fake-gh-okf.mjs" "$OKF/bin/gh"
python3 - "$EVAL/okf-bundle.json" "$OKF" <<'PY'
import json, sys
fx = json.load(open(sys.argv[1]))
docs = fx["pointers"]["docs"]["head"]
fx.update(pointers={}, anyPointer=docs, defaultCommit=docs)
json.dump(fx, open(sys.argv[2] + "/fixture.json", "w"))
open(sys.argv[2] + "/pr.patch", "w").write("".join(f"--- a/{p}\n+++ b/{p}\n" for p in fx["changed"]))
PY
sources() { python3 -c "
import json,sys
d=json.loads(sys.stdin.read())
if not d.get('ok'): print('FAIL', d.get('error')); sys.exit()
bad=[f\"{s['repo']}: {s['error']}\" for s in d.get('sources', []) if 'error' in s]
if bad: print('FAIL', '; '.join(bad)); sys.exit()
print('ok', '; '.join(f\"{s['repo']}: {len(s['loaded'])} loaded ({s['bytesLoaded']} of {s['budgetBytes']} bytes), {len(s['offered'])} offered\" for s in d.get('sources', [])))"; }

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
    if python3 -c "import json,sys;sys.exit(0 if json.load(open(sys.argv[1])).get('conventionSources') else 1)" "$P"; then
      out=$(cd "$PLUGIN" && PATH="$OKF/bin:$PATH" FAKE_GH_FIXTURE="$OKF/fixture.json" node "$dir/scripts/load-conventions.mjs" --profile "$P" --ref main --diff "$OKF/pr.patch" --out "$OKF/conventions-$name.md" --pr 1 2>&1)
      res=$(printf '%s' "$out" | sources)
      if [ "${res%% *}" = ok ]; then say ok "$name reads conventionSources" "${res#ok }"
      else say FAIL "$name reads conventionSources" "${res#FAIL }"; fail=1; fi
    fi
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
