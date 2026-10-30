#!/usr/bin/env bash
# Checks the tribunal action's manifests for mistakes actionlint does not catch: GitHub evaluates
# `${{ }}` everywhere in an action manifest, descriptions included, and fails to load the action
# when an expression names a context actions cannot use (for example `secrets`).
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/helper.sh"
echo "tribunal action manifest_test:"

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
while IFS= read -r f; do MANIFESTS+=("$f"); done < <(find "$HERE/.." -name action.yml | sort)
assert_ok "there are action manifests to check" test "${#MANIFESTS[@]}" -gt 0
assert_eq "no expression inside a description" "" "$(expressions_in_descriptions "${MANIFESTS[@]}")"
assert_eq "no secrets or vars context in an action" "" "$(forbidden_contexts "${MANIFESTS[@]}")"

# defaults FILE: "name=default" for each input of an action manifest, sorted by name.
defaults() {
  python3 - "$1" <<'PY'
import re, sys
section, name, found = None, None, {}
for line in open(sys.argv[1]):
    if re.match(r"\S", line):
        section = line.split(":")[0]
    elif section == "inputs" and re.match(r"  [\w-]+:\s*$", line):
        name = line.strip()[:-1]; found[name] = ""
    elif section == "inputs" and name and re.match(r"    default:", line):
        found[name] = line.split(":", 1)[1].strip()
for k in sorted(found):
    print(f"{k}={found[k]}")
PY
}

ACTION="$HERE/../action.yml" GATE="$HERE/../gate/action.yml"
assert_ok "there is a gate action" test -f "$GATE"
SHARED="$(defaults "$ACTION" | grep -E '^(github-token|claude-token|anthropic-api-key|label|paths|once|drafts)=')"
assert_eq "the action has every shared input" 7 "$(printf '%s\n' "$SHARED" | grep -c .)"
assert_eq "the gate takes the action's shared inputs, with the same defaults" "$SHARED" "$(defaults "$GATE" 2>/dev/null)"
assert_eq "paths defaults to any file" "paths=''" "$(printf '%s\n' "$SHARED" | grep '^paths=')"
assert_eq "once defaults to true" "once='true'" "$(printf '%s\n' "$SHARED" | grep '^once=')"
assert_eq "drafts defaults to false" "drafts='false'" "$(printf '%s\n' "$SHARED" | grep '^drafts=')"
assert_ok "the gate outputs run" grep -qE '^  run:' "$GATE"
assert_ok "the gate runs the action's own gate" grep -qF 'bash "$GITHUB_ACTION_PATH/../run.sh" gate' "$GATE"
# gate_env FILE: the env lines of the step with `id: gate`, unindented.
gate_env() {
  awk '/^    - id: gate$/ { g = 1; next } g && /^    - / { g = 0 } g && /^        [A-Z_]+: / { sub(/^ +/, ""); print }' "$1" 2>/dev/null
}
assert_eq "the gate computes the credential as the action does" \
  "$(gate_env "$ACTION" | grep '^HAS_CLAUDE_AUTH:')" "$(gate_env "$GATE" | grep '^HAS_CLAUDE_AUTH:')"
for v in GH_TOKEN INPUT_LABEL INPUT_PATHS INPUT_ONCE INPUT_DRAFTS; do
  assert_ok "the action's gate step gets $v" grep -q "^$v: \${{ inputs\." <(gate_env "$ACTION")
  assert_eq "the gate action passes $v as the action does" \
    "$(gate_env "$ACTION" | grep "^$v:")" "$(gate_env "$GATE" | grep "^$v:")"
done

finish_tests
