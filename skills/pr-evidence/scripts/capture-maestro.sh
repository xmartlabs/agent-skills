#!/usr/bin/env bash
# capture-maestro.sh --flow FILE --out DIR/BASENAME [--platform ios|android|web] [--device UDID]
#
# The mobile capture driver: records a Maestro flow to a local video and converts it to the same
# GIF+MP4 evidence artifacts the web flow produces. Wraps the `maestro` CLI (not its MCP) so it has
# no "is the MCP connected" dependency — consistent with how the kit wraps playwright-cli / gh image.
#
# `maestro record --local` DRIVES the flow (running its assertions) while recording. The flow is
# expected to assert its post-state after each interaction (assertVisible/assertNotVisible) — a tap
# reporting success is NOT proof it worked, and a recording of a wrong-but-"successful" tap is
# misleading evidence, worse than none. If any assertion fails, `maestro record` fails and this
# script FAILS-CLOSED: nothing on stdout, non-zero exit, so the caller reports "evidence pending".
#
# Output: one "basename<TAB>path" line per artifact (gif, then mp4) — the same per-file mapping shape
# the user-attachments host consumes.
set -u
fail() { echo "capture-maestro: $1" >&2; exit 1; }

FLOW=""; OUT=""; PLATFORM=""; DEVICE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --flow) FLOW="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --platform) PLATFORM="$2"; shift 2 ;;
    --device) DEVICE="$2"; shift 2 ;;
    --) shift; break ;;
    --*) fail "unknown flag '$1'" ;;
    *) fail "unexpected arg '$1'" ;;
  esac
done

[ -n "$FLOW" ] || fail "--flow FILE is required"
[ -n "$OUT" ]  || fail "--out DIR/BASENAME is required"
[ -f "$FLOW" ] || fail "flow file not found: $FLOW"
command -v maestro >/dev/null 2>&1 || fail "maestro not on PATH (install: https://maestro.dev)"
command -v ffmpeg  >/dev/null 2>&1 || fail "ffmpeg not on PATH"

mkdir -p "$(dirname "$OUT")" 2>/dev/null || fail "cannot create output directory for '$OUT'"
MP4="$OUT.mp4"; GIF="$OUT.gif"

# Build the maestro invocation: global -p/--device come before the `record` subcommand.
# --local keeps rendering on this machine (no Maestro Cloud / login dependency).
set -- record --local "$FLOW" "$MP4"
[ -n "$DEVICE" ]   && set -- --device "$DEVICE" "$@"
[ -n "$PLATFORM" ] && set -- -p "$PLATFORM" "$@"
maestro "$@" >/dev/null 2>&1 || fail "maestro record failed (flow assertion failed or device unavailable)"
[ -s "$MP4" ] || fail "maestro produced no video"

# mp4 is the full-fidelity artifact; the GIF is the inline-embeddable one (same pipeline as web).
ffmpeg -y -i "$MP4" -vf "fps=12,scale=640:-1:flags=lanczos" -loop 0 "$GIF" >/dev/null 2>&1 \
  || fail "ffmpeg gif conversion failed"
[ -s "$GIF" ] || fail "gif not produced"

printf '%s\t%s\n' "$(basename "$GIF")" "$GIF"
printf '%s\t%s\n' "$(basename "$MP4")" "$MP4"
