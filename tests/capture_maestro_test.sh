#!/usr/bin/env bash
# Exercises the mobile capture driver (scripts/capture-maestro.sh) with FAKE maestro+ffmpeg on
# PATH — no device, no network. Verifies the artifact mapping and fail-closed on every error path.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
CAP="$DIR/skills/pr-evidence/scripts/capture-maestro.sh"
echo "capture_maestro_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/out"
printf 'appId: x\n---\n- launchApp\n' > "$TMP/flow.yaml"
is_nonzero() { [ "$1" -ne 0 ]; }
contains() { printf '%s' "$2" | grep -qF "$1"; }

# ffmpeg mock: write a non-empty gif to its last arg (the output path).
cat > "$TMP/bin/ffmpeg" <<'SH'
#!/usr/bin/env bash
out="${@: -1}"; printf 'GIF89a-fake\n' > "$out"
SH
# maestro mock (success): `... record --local FLOW OUT.mp4` -> write a non-empty mp4 to the last arg.
cat > "$TMP/bin/maestro" <<'SH'
#!/usr/bin/env bash
out="${@: -1}"; printf 'fake-mp4\n' > "$out"
SH
chmod +x "$TMP/bin/ffmpeg" "$TMP/bin/maestro"

# 1. Success -> gif + mp4 mapping lines, exit 0.
OUT="$(PATH="$TMP/bin:$PATH" bash "$CAP" --flow "$TMP/flow.yaml" --out "$TMP/out/ev" --platform ios 2>/dev/null)"; RC=$?
assert_eq "success exits 0" "0" "$RC"
assert_ok "maps gif artifact" contains "ev.gif	$TMP/out/ev.gif" "$OUT"
assert_ok "maps mp4 artifact" contains "ev.mp4	$TMP/out/ev.mp4" "$OUT"
assert_ok "gif file exists" test -s "$TMP/out/ev.gif"
assert_ok "mp4 file exists" test -s "$TMP/out/ev.mp4"

# 2. maestro fails (a flow assertion failed) -> fail closed.
cat > "$TMP/bin/maestro" <<'SH'
#!/usr/bin/env bash
echo "assertion failed" >&2; exit 1
SH
chmod +x "$TMP/bin/maestro"
OUT2="$(PATH="$TMP/bin:$PATH" bash "$CAP" --flow "$TMP/flow.yaml" --out "$TMP/out/ev2" 2>/dev/null)"; RC2=$?
assert_ok "maestro-fail exits non-zero" is_nonzero "$RC2"
assert_eq "maestro-fail prints nothing" "" "$OUT2"

# 3. maestro produces an empty video -> fail closed (no misleading artifact).
cat > "$TMP/bin/maestro" <<'SH'
#!/usr/bin/env bash
out="${@: -1}"; : > "$out"   # empty file
SH
chmod +x "$TMP/bin/maestro"
OUT3="$(PATH="$TMP/bin:$PATH" bash "$CAP" --flow "$TMP/flow.yaml" --out "$TMP/out/ev3" 2>/dev/null)"; RC3=$?
assert_ok "empty-video exits non-zero" is_nonzero "$RC3"
assert_eq "empty-video prints nothing" "" "$OUT3"

# 4. Missing flow file -> fail closed.
OUT4="$(PATH="$TMP/bin:$PATH" bash "$CAP" --flow "$TMP/nope.yaml" --out "$TMP/out/ev4" 2>/dev/null)"; RC4=$?
assert_ok "missing-flow exits non-zero" is_nonzero "$RC4"
assert_eq "missing-flow prints nothing" "" "$OUT4"

# 5. maestro not installed -> fail closed (PATH without a maestro; ffmpeg-only mock dir + coreutils).
mkdir -p "$TMP/bin2"; cp "$TMP/bin/ffmpeg" "$TMP/bin2/ffmpeg"
OUT5="$(PATH="$TMP/bin2:/usr/bin:/bin" bash "$CAP" --flow "$TMP/flow.yaml" --out "$TMP/out/ev5" 2>/dev/null)"; RC5=$?
assert_ok "missing-maestro exits non-zero" is_nonzero "$RC5"
assert_eq "missing-maestro prints nothing" "" "$OUT5"

finish_tests
