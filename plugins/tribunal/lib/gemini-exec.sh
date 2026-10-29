#!/usr/bin/env bash
# gemini-exec.sh — run the Gemini CLI non-interactively, read-only, and print the
# model's answer to stdout. Shared by the cerberus head-3 fallback and the
# peer-review Gemini refuter so the CLI flags/auth/trust/timeout live in ONE place.
# Nothing skill-specific lives here — the caller owns the prompt and the parsing.
#
# Usage: gemini-exec.sh <model> <timeout-seconds> <prompt>
#   stdout = model output (nothing on failure)
#   exit 0 with non-empty output on success; non-zero otherwise (caller falls back)
#
# Notes:
#   - Free-tier keys can use gemini-2.5-flash; gemini-2.5-pro is limit:0 without billing.
#   - --approval-mode plan = read-only (reads files, never edits/executes).
#   - GEMINI_CLI_TRUST_WORKSPACE=true is required for headless runs in untrusted dirs,
#     otherwise the CLI downgrades to interactive approval and refuses to run.
#   - stderr (retry/backoff stack traces on rate-limit blips) is dropped; the clean
#     answer comes on stdout with -o text.
set -euo pipefail

MODEL="${1:?usage: gemini-exec.sh <model> <timeout-seconds> <prompt>}"
TIMEOUT="${2:?missing timeout seconds}"
PROMPT="${3:?missing prompt}"

# Resolve the API key: prefer the environment, fall back to ~/.zshrc so the helper
# works whether or not the caller's shell exported it (mirrors how the codex head
# reads its model back from ~/.codex/config.toml).
if [ -z "${GEMINI_API_KEY:-}" ]; then
  GEMINI_API_KEY=$(grep -m1 '^export GEMINI_API_KEY=' "$HOME/.zshrc" 2>/dev/null \
    | sed -E 's/^export GEMINI_API_KEY="?([^"]*)"?.*/\1/')
fi
[ -n "${GEMINI_API_KEY:-}" ] || { echo "gemini-exec: no GEMINI_API_KEY" >&2; exit 3; }
export GEMINI_API_KEY GEMINI_CLI_TRUST_WORKSPACE=true

command -v gemini >/dev/null 2>&1 || { echo "gemini-exec: gemini CLI not installed" >&2; exit 4; }
TO=$(command -v timeout || command -v gtimeout || true)

OUT=$($TO ${TO:+"$TIMEOUT"} gemini -m "$MODEL" --approval-mode plan -o text -p "$PROMPT" 2>/dev/null) || true
if [ -z "$(printf '%s' "$OUT" | tr -d '[:space:]')" ]; then
  echo "gemini-exec: empty/failed response" >&2
  exit 1
fi
printf '%s\n' "$OUT"
