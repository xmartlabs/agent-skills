#!/usr/bin/env bash
# codex-model.sh — print the model the Codex external heads (refuter slots and the
# clean-verdict sweep) run on. Shared by both so the lookup lives in ONE place;
# the Rust port is `codex_model` in rust/crates/tribunal-context/src/heads.rs.
#
# Usage: codex-model.sh            stdout = one model slug, always non-empty
#
# Resolution order:
#   1. the top-level `model` in ~/.codex/review.config.toml
#   2. the top-level `model` in ~/.codex/config.toml
#   3. DEFAULT below, when neither is set
#
# WHY THE REVIEW PROFILE. The Codex app and TUI rewrite the top-level `model` of
# config.toml when you pick a model in their selector, so reading only that line
# meant that switching to a cheap model for an unrelated chat silently moved every
# later review onto it. review.config.toml is the file `codex --profile review`
# overlays (Codex 0.134+, which no longer reads `[profiles.review]` tables), and
# the selector does not write to it. The heads run with --ignore-user-config, so
# passing `--profile review` would not apply it: the file is read here instead and
# its model passed as -m.
#
# Step 2 keeps existing setups on the model they already chose; creating
# review.config.toml is what decouples the review from the app.
set -uo pipefail

DEFAULT="gpt-6.1-sol"

top_level_model() {
  awk '
    /^[[:space:]]*\[/ { exit }
    /^model = / && match($0, /".*"/) && RLENGTH > 2 {
      print substr($0, RSTART + 1, RLENGTH - 2); exit
    }
  ' "$1" 2>/dev/null
}

MODEL=$(top_level_model "$HOME/.codex/review.config.toml")
[ -n "$MODEL" ] || MODEL=$(top_level_model "$HOME/.codex/config.toml")
printf '%s\n' "${MODEL:-$DEFAULT}"
