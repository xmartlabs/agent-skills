#!/usr/bin/env bash
# Exercises the user-attachments host builtin (scripts/host-user-attachments.sh) with a FAKE `gh`
# on PATH — no network, no real extension. Verifies the file->url mapping, both output shapes
# gh image can emit (image `![](url)` and bare-url video), and fail-closed on every error path.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
HOST="$DIR/skills/pr-evidence/scripts/host-user-attachments.sh"
echo "host_user_attachments_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
printf 'gif\n' > "$TMP/a.gif"; printf 'mp4\n' > "$TMP/b.mp4"
is_nonzero() { [ "$1" -ne 0 ]; }

# Success fake: `gh image --help` ok; `gh image FILES` prints one ref line per file (in order),
# images as ![name](url) and .mp4 as a bare url (mirrors gh-image's real behavior).
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
[ "$1" = "image" ] || exit 0
shift
[ "$1" = "--help" ] && exit 0
n=0
for f in "$@"; do
  n=$((n + 1))
  case "$f" in
    *.mp4) printf 'https://github.com/user-attachments/assets/uuid-%s\n' "$n" ;;
    *)     printf '![%s](https://github.com/user-attachments/assets/uuid-%s)\n' "$(basename "$f")" "$n" ;;
  esac
done
SH
chmod +x "$TMP/bin/gh"

# 1. Two files (image-form + bare-url video form) -> one "basename<TAB>url" line each, exit 0.
OUT="$(PATH="$TMP/bin:$PATH" bash "$HOST" PC-1 "$TMP/a.gif" "$TMP/b.mp4" 2>/dev/null)"; RC=$?
assert_eq "success exits 0" "0" "$RC"
assert_ok "gif -> user-attachments url" \
  bash -c 'printf "%s\n" "$1" | grep -qE "^a\.gif[[:space:]]+https://github\.com/user-attachments/assets/uuid-1$"' _ "$OUT"
assert_ok "mp4 (bare-url form) -> user-attachments url" \
  bash -c 'printf "%s\n" "$1" | grep -qE "^b\.mp4[[:space:]]+https://github\.com/user-attachments/assets/uuid-2$"' _ "$OUT"

# 2. Upload error -> fail closed (non-zero, nothing on stdout).
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
[ "$1" = "image" ] || exit 0
shift
[ "$1" = "--help" ] && exit 0
echo "upload failed" >&2; exit 1
SH
chmod +x "$TMP/bin/gh"
OUT2="$(PATH="$TMP/bin:$PATH" bash "$HOST" PC-2 "$TMP/a.gif" 2>/dev/null)"; RC2=$?
assert_ok "upload-fail exits non-zero" is_nonzero "$RC2"
assert_eq "upload-fail prints nothing" "" "$OUT2"

# 3. Extension not installed (`gh image --help` fails) -> fail closed.
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
echo "unknown command \"image\" for \"gh\"" >&2; exit 1
SH
chmod +x "$TMP/bin/gh"
OUT3="$(PATH="$TMP/bin:$PATH" bash "$HOST" PC-3 "$TMP/a.gif" 2>/dev/null)"; RC3=$?
assert_ok "missing-extension exits non-zero" is_nonzero "$RC3"
assert_eq "missing-extension prints nothing" "" "$OUT3"

# 4. Missing input file -> fail closed (before invoking gh).
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
[ "$1" = "image" ] || exit 0
shift
[ "$1" = "--help" ] && exit 0
printf '![x](https://github.com/user-attachments/assets/uuid-x)\n'
SH
chmod +x "$TMP/bin/gh"
OUT4="$(PATH="$TMP/bin:$PATH" bash "$HOST" PC-4 "$TMP/nope.gif" 2>/dev/null)"; RC4=$?
assert_ok "missing-file exits non-zero" is_nonzero "$RC4"
assert_eq "missing-file prints nothing" "" "$OUT4"

# 5. Fewer URLs than files (partial upload) -> fail closed rather than a mismatched mapping.
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
[ "$1" = "image" ] || exit 0
shift
[ "$1" = "--help" ] && exit 0
printf '![only](https://github.com/user-attachments/assets/uuid-1)\n'
SH
chmod +x "$TMP/bin/gh"
OUT5="$(PATH="$TMP/bin:$PATH" bash "$HOST" PC-5 "$TMP/a.gif" "$TMP/b.mp4" 2>/dev/null)"; RC5=$?
assert_ok "count-mismatch exits non-zero" is_nonzero "$RC5"
assert_eq "count-mismatch prints nothing" "" "$OUT5"

finish_tests
