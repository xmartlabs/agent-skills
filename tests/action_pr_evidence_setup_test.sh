#!/usr/bin/env bash
# Offline regression tests for the action-owned GitHub CLI, before either gate entrypoint.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RUN="$DIR/actions/pr-evidence/run.sh"
echo "action_pr_evidence_setup_test:"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/releases"
# Restrict PATH so a developer's installed gh cannot accidentally satisfy the tests.
for cmd in bash dirname mkdir mktemp rm mv ln tar grep awk python3; do
  ln -s "$(command -v "$cmd")" "$TMP/bin/$cmd"
done
cat > "$TMP/bin/uname" <<'EOF'
#!/bin/bash
printf '%s\n' "${TEST_ARCH:-x86_64}"
EOF
cat > "$TMP/bin/sha256sum" <<'EOF'
#!/bin/bash
python3 - "$@" <<'PY'
import hashlib, pathlib, sys
assert sys.argv[1] == '-c'
for line in pathlib.Path(sys.argv[2]).read_text().splitlines():
    expected, name = line.split()
    if hashlib.sha256(pathlib.Path(name).read_bytes()).hexdigest() != expected:
        sys.exit(1)
PY
EOF
cat > "$TMP/bin/curl" <<'EOF'
#!/bin/bash
while [ "$1" != -o ]; do shift; done
out="$2"; url="$3"
printf '%s\n' "$url" >> "$FIXTURES/downloads"
[ "${DOWNLOAD_FAIL:-false}" != true ] || exit 22
/bin/cp "$FIXTURES/${url##*/}" "$out"
EOF
chmod +x "$TMP/bin/"{uname,sha256sum,curl}
export FIXTURES="$TMP/releases"
for arch in amd64 arm64; do
  name="gh_2.103.3_linux_$arch"
  mkdir -p "$TMP/archive/$name/bin"
  cat > "$TMP/archive/$name/bin/gh" <<'EOF'
#!/bin/bash
case "$1" in
  --version) echo 'gh version 2.103.3 (fixture)' ;;
  api)
    case "$2" in
      user) exit 1 ;;
      --paginate) echo src/widget.ts ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
EOF
  chmod +x "$TMP/archive/$name/bin/gh"
  tar -czf "$FIXTURES/$name.tar.gz" -C "$TMP/archive" "$name"
done
python3 - "$FIXTURES" <<'PY' > "$FIXTURES/gh_2.103.3_checksums.txt"
import hashlib, pathlib, sys
for path in sorted(pathlib.Path(sys.argv[1]).glob('*.tar.gz')):
    print(hashlib.sha256(path.read_bytes()).hexdigest(), '', path.name)
PY
cp "$FIXTURES/gh_2.103.3_checksums.txt" "$TMP/good-checksums"
new_job() {
  rm -rf "$TMP/runner"; mkdir -p "$TMP/runner"
  export RUNNER_TEMP="$TMP/runner" GITHUB_PATH="$TMP/runner/path"
  : > "$FIXTURES/downloads"
}
setup_gh() { PATH="$TMP/bin" bash "$RUN" setup-gh > "$TMP/setup.log" 2>&1; }
new_job
assert_ok "installs without any ambient gh" setup_gh
assert_ok "publishes the job-local tools directory" grep -qxF "$RUNNER_TEMP/pr-evidence/tools/bin" "$GITHUB_PATH"
assert_eq "the installed CLI is pinned" 'gh version 2.103.3 (fixture)' "$("$RUNNER_TEMP/pr-evidence/tools/bin/gh" --version 2>/dev/null)"
assert_eq "downloads the archive and checksums" 2 "$(wc -l < "$FIXTURES/downloads" | tr -d ' ')"
assert_ok "repeated setup succeeds" setup_gh
assert_eq "repeated setup reuses its own installation" 2 "$(wc -l < "$FIXTURES/downloads" | tr -d ' ')"

cat > "$TMP/bin/gh" <<'EOF'
#!/bin/bash
echo 'broken ambient CLI' >&2
exit 1
EOF
chmod +x "$TMP/bin/gh"
new_job
assert_ok "ignores a broken ambient gh" setup_gh
assert_eq "uses the private CLI instead of the ambient gh" 'gh version 2.103.3 (fixture)' "$(PATH="$RUNNER_TEMP/pr-evidence/tools/bin:$TMP/bin" gh --version 2>/dev/null)"

printf '#!/bin/bash\necho "gh version 99.0.0 (ambient)"\n' > "$TMP/bin/gh"
new_job
assert_ok "ignores even a healthy ambient gh" setup_gh
assert_eq "always installs the pinned release" 2 "$(wc -l < "$FIXTURES/downloads" | tr -d ' ')"

new_job
assert_ok "installs the arm64 archive" env TEST_ARCH=aarch64 PATH="$TMP/bin" bash "$RUN" setup-gh > "$TMP/arm.log" 2>&1
assert_ok "selects arm64" grep -q 'gh_2.103.3_linux_arm64.tar.gz' "$FIXTURES/downloads"

new_job
printf '%064d  gh_2.103.3_linux_amd64.tar.gz\n' 0 > "$FIXTURES/gh_2.103.3_checksums.txt"
assert_fail "checksum mismatch fails closed" setup_gh
assert_fail "checksum failure does not publish a binary" test -e "$RUNNER_TEMP/pr-evidence/tools/bin/gh"
assert_fail "checksum failure does not publish PATH" test -s "$GITHUB_PATH"
cp "$TMP/good-checksums" "$FIXTURES/gh_2.103.3_checksums.txt"
assert_ok "a failed download can be retried safely" setup_gh

new_job
printf '%s\n' 'unrelated checksum entry' > "$FIXTURES/gh_2.103.3_checksums.txt"
assert_fail "a missing checksum entry fails closed" setup_gh
assert_fail "missing checksum does not publish a binary" test -e "$RUNNER_TEMP/pr-evidence/tools/bin/gh"
cp "$TMP/good-checksums" "$FIXTURES/gh_2.103.3_checksums.txt"

new_job
assert_fail "download errors fail closed" env DOWNLOAD_FAIL=true PATH="$TMP/bin" bash "$RUN" setup-gh > "$TMP/download.log" 2>&1
assert_fail "download failure does not publish a binary" test -e "$RUNNER_TEMP/pr-evidence/tools/bin/gh"
new_job
assert_fail "unsupported architectures fail clearly" env TEST_ARCH=unsupported PATH="$TMP/bin" bash "$RUN" setup-gh > "$TMP/arch.log" 2>&1
assert_eq "unsupported architectures download nothing" '' "$(cat "$FIXTURES/downloads")"

# Run the manifests' setup and gate commands as separate steps, applying GITHUB_PATH between them.
for manifest in action.yml gate/action.yml; do
  new_job
  export GITHUB_ACTION_PATH="$DIR/actions/pr-evidence/$(dirname "$manifest")"
  export GITHUB_REPOSITORY=acme/web-app GITHUB_EVENT_PATH="$RUNNER_TEMP/event.json"
  export GITHUB_OUTPUT="$RUNNER_TEMP/out" HAS_CLAUDE_AUTH=true INPUT_PATHS='src/*' INPUT_ONCE=false
  printf '%s\n' '{"action":"opened","pull_request":{"number":12,"draft":false,"head":{"repo":{"full_name":"acme/web-app"}}}}' > "$GITHUB_EVENT_PATH"
  python3 - "$DIR/actions/pr-evidence/$manifest" "$TMP/steps" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
commands = re.findall(r'^      run: (.+)$', text, re.M)
# Keep this executable wiring check strict: setup must precede the first gate.
assert len(commands) >= 2 and commands[0].endswith(' setup-gh') and commands[1].endswith(' gate')
for i, command in enumerate(commands[:2]):
    open(sys.argv[2] + str(i), 'w').write(command + '\n')
PY
  rc=$?
  assert_eq "$manifest: bootstrap precedes gate" 0 "$rc"
  if [ "$rc" = 0 ]; then
    assert_ok "$manifest: setup step succeeds" env PATH="$TMP/bin" bash -e "$TMP/steps0" > "$TMP/entry.log" 2>&1
    assert_ok "$manifest: gate works with the installed CLI" env PATH="$(head -1 "$GITHUB_PATH"):$TMP/bin" bash -e "$TMP/steps1" >> "$TMP/entry.log" 2>&1
    assert_ok "$manifest: gate decides to capture" grep -qx 'run=true' "$GITHUB_OUTPUT"
  fi
done
finish_tests
