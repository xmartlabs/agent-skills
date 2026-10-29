#!/usr/bin/env bash
# Checks every actions/**/action.yml for mistakes actionlint does not catch: GitHub evaluates
# `${{ }}` everywhere in an action manifest, descriptions included, and fails to load the action
# when an expression names a context actions cannot use (for example `secrets`).
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
echo "action_manifests_test:"

# Prints "file:line" for each `${{` inside a description (the key's line and its continuation).
expressions_in_descriptions() {
  python3 - "$@" <<'PY'
import re, sys
for path in sys.argv[1:]:
    desc_indent = None
    for n, line in enumerate(open(path), 1):
        indent = len(line) - len(line.lstrip())
        if desc_indent is not None and line.strip() and indent <= desc_indent:
            desc_indent = None
        if re.match(r"\s*description:", line):
            desc_indent = indent
        if desc_indent is not None and "${{" in line:
            print(f"{path}:{n}")
PY
}

# Prints "file: name" for each context an action manifest cannot read.
forbidden_contexts() {
  grep -HnoE '\$\{\{[^}]*\b(secrets|vars)\.' "$@" || true
}

MANIFESTS=()
while IFS= read -r f; do MANIFESTS+=("$f"); done < <(find "$DIR/actions" -name action.yml | sort)
assert_ok "there are action manifests to check" test "${#MANIFESTS[@]}" -gt 0
assert_eq "no expression inside a description" "" "$(expressions_in_descriptions "${MANIFESTS[@]}")"
assert_eq "no secrets or vars context in an action" "" "$(forbidden_contexts "${MANIFESTS[@]}")"

finish_tests
