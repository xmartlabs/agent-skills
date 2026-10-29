#!/usr/bin/env bash
# Tests the release-notes renderer (pure, via fixtures) and the orchestrator end-to-end with FAKE
# git+gh on PATH — no network.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
RENDER="$DIR/skills/release-notes/scripts/lib/render-notes.py"
RN="$DIR/skills/release-notes/scripts/release-notes.sh"
echo "release_notes_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
contains() { printf '%s' "$2" | grep -qF "$1"; }
lacks()    { ! printf '%s' "$2" | grep -qF "$1"; }
is_nonzero() { [ "$1" -ne 0 ]; }

# ---- renderer (pure) ----
J1='{"range":"v1..v2","prs":[
 {"number":483,"title":"[PC-998] - Fix modal","url":"https://gh/pr/483","type":"fix",
  "ticket":{"key":"PC-998","url":"https://jira/PC-998"},
  "evidence":{"commentUrl":"https://gh/pr/483#c1","images":["https://github.com/user-attachments/assets/aaa"],"links":["https://github.com/user-attachments/assets/bbb"]}},
 {"number":484,"title":"[PC-999] - Add thing","url":"https://gh/pr/484","type":"feature","ticket":{"key":"PC-999"},"evidence":null}
]}'
OUT1="$(printf '%s' "$J1" | python3 "$RENDER")"; RC1=$?
assert_eq "renderer exits 0" "0" "$RC1"
assert_ok "has Features heading"            contains "### 🚀 Features" "$OUT1"
assert_ok "has Fixes heading"               contains "### 🐛 Fixes" "$OUT1"
assert_ok "count summary"                   contains "_2 changes · 1 with evidence_" "$OUT1"
assert_ok "title from range"                contains "## v1..v2" "$OUT1"
assert_ok "ticket linked to jira"           contains "**[PC-998](https://jira/PC-998)**" "$OUT1"
assert_ok "ticketless key stays plain bold" contains "**PC-999**" "$OUT1"
assert_ok "embeds first evidence image"     contains "![](https://github.com/user-attachments/assets/aaa)" "$OUT1"
assert_ok "links evidence comment"          contains "[evidence](https://gh/pr/483#c1)" "$OUT1"
assert_ok "links video"                     contains "[video](https://github.com/user-attachments/assets/bbb)" "$OUT1"
# Features must render before Fixes.
fpos="$(printf '%s' "$OUT1" | grep -n 'Features' | head -1 | cut -d: -f1)"
xpos="$(printf '%s' "$OUT1" | grep -n 'Fixes' | head -1 | cut -d: -f1)"
assert_ok "Features before Fixes" bash -c '[ "$1" -lt "$2" ]' _ "$fpos" "$xpos"

# A raw private-repo URL must NOT be embedded as an image.
J2='{"prs":[{"number":1,"title":"x","type":"fix","evidence":{"commentUrl":"c","images":["https://github.com/org/repo/raw/sha/x.gif"],"links":[]}}]}'
OUT2="$(printf '%s' "$J2" | python3 "$RENDER")"
assert_ok "non-embeddable raw url not embedded" lacks "![](https://github.com/org/repo/raw" "$OUT2"

# Allowlisted markers in a foreign URL's path/query must NOT be embedded.
J2b='{"prs":[{"number":1,"title":"x","type":"fix","evidence":{"commentUrl":"c","images":["https://evil.example/?githubusercontent.com/","https://evil.example/https://github.com/user-attachments/assets/x","https://private-user-images.githubusercontent.com/ok.png"],"links":[]}}]}'
OUT2b="$(printf '%s' "$J2b" | python3 "$RENDER")"
assert_ok "foreign host with marker in query not embedded" lacks "evil.example" "$OUT2b"
assert_ok "githubusercontent subdomain embedded" contains "![](https://private-user-images.githubusercontent.com/ok.png)" "$OUT2b"

# Empty range renders a clear placeholder.
OUT3="$(printf '%s' '{"range":"v1..v2","prs":[]}' | python3 "$RENDER")"
assert_ok "empty range placeholder" contains "No merged pull requests" "$OUT3"

# ---- orchestrator end-to-end (mock git + gh) ----
mkdir -p "$TMP/bin"
cat > "$TMP/bin/git" <<'SH'
#!/usr/bin/env bash
[ "$1" = "tag" ] && { printf 'v2\nv1\n'; exit 0; }
if [ "$1" = "log" ]; then
  # first-parent subjects: merge-commit form, a dup, a squash-(#N) form, and an unreadable PR.
  printf '%s\n' \
    "Merge pull request #483 from org/a" \
    "chore: direct commit" \
    "Merge pull request #484 from org/b" \
    "squash change (#483)" \
    "Merge pull request #999 from org/gone"
  exit 0
fi
exit 0
SH
cat > "$TMP/bin/gh" <<'SH'
#!/usr/bin/env bash
case "$1" in
  auth) exit 0 ;;
  pr)   # gh pr view N --json ...
    case "$3" in
      483) cat <<'J'
{"number":483,"title":"[PC-998] - Fix modal","url":"https://gh/pr/483","labels":[{"name":"bug"}],"body":"b","comments":[{"body":"## PR evidence\n![](https://github.com/user-attachments/assets/aaa)","url":"https://gh/pr/483#c1"}]}
J
      ;;
      484) cat <<'J'
{"number":484,"title":"[PC-999] - Add feature","url":"https://gh/pr/484","labels":[{"name":"feature"}],"body":"b","comments":[]}
J
      ;;
      *) echo "not found" >&2; exit 1 ;;
    esac
  ;;
  release)
    case "$2" in
      view)  # existence probe -> 0; `--json body --jq .body` -> print the existing body
        for a in "$@"; do [ "$a" = "--json" ] && { echo "HAND-WRITTEN BODY"; exit 0; }; done
        exit 0 ;;
      edit)  # capture the notes-file content so the test can assert what got written
        prev=""; for a in "$@"; do [ "$prev" = "--notes-file" ] && cat "$a" > "${RN_CAP:-/dev/null}"; prev="$a"; done
        exit 0 ;;
      *) exit 0 ;;
    esac
  ;;
  *) exit 0 ;;
esac
SH
chmod +x "$TMP/bin/git" "$TMP/bin/gh"

OUT="$(PATH="$TMP/bin:$PATH" bash "$RN" v1..v2 2>/dev/null)"; RC=$?
assert_eq "orchestrator exits 0" "0" "$RC"
assert_ok "PC-998 present (fix)"      contains "PC-998" "$OUT"
assert_ok "PC-999 present (feature)"  contains "PC-999" "$OUT"
assert_ok "evidence image embedded"   contains "![](https://github.com/user-attachments/assets/aaa)" "$OUT"
assert_ok "unreadable PR skipped, still 2 with a fix+feature" contains "_2 changes" "$OUT"
# dedupe: #483 must appear exactly once as a PR line.
n483="$(printf '%s\n' "$OUT" | grep -cE '\(\[#483\]|#483\)')"
assert_ok "PR #483 deduped to one line" bash -c '[ "$1" -ge 1 ] && [ "$1" -le 2 ]' _ "$n483"

# default range (no arg) resolves <lastTag>..HEAD and still works.
OUT_DEF="$(PATH="$TMP/bin:$PATH" bash "$RN" 2>/dev/null)"; RCD=$?
assert_eq "default-range exits 0" "0" "$RCD"
assert_ok "default-range has content" contains "PC-998" "$OUT_DEF"

# publish to an existing release calls gh release edit and reports it.
ERRP="$(PATH="$TMP/bin:$PATH" bash "$RN" --publish v2 v1..v2 2>&1 >/dev/null)"; RCP=$?
assert_eq "publish exits 0" "0" "$RCP"
assert_ok "publish reports update" contains "updated release v2" "$ERRP"

# --append preserves the existing body and adds the generated notes below it.
CAP="$TMP/captured.md"
ERRA="$(PATH="$TMP/bin:$PATH" RN_CAP="$CAP" bash "$RN" --publish v2 --append v1..v2 2>&1 >/dev/null)"; RCA=$?
assert_eq "append exits 0" "0" "$RCA"
assert_ok "append reports appended" contains "appended to release v2" "$ERRA"
CAPPED="$(cat "$CAP" 2>/dev/null)"
assert_ok "append keeps hand-written body"   contains "HAND-WRITTEN BODY" "$CAPPED"
assert_ok "append adds generated notes"      contains "PC-998" "$CAPPED"

# --append without --publish is rejected.
PATH="$TMP/bin:$PATH" bash "$RN" --append v1..v2 >/dev/null 2>&1
assert_ok "append without publish fails" is_nonzero "$?"

finish_tests
