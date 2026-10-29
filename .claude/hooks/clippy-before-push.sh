#!/usr/bin/env bash
# PreToolUse on Bash: before a `git push` whose branch changes the tribunal Rust
# engine, run the two fast steps of CI's linux job (`cargo fmt --all --check` and
# `cargo clippy ... -D warnings`). A failure exits 2, which blocks the push and
# hands the error back to Claude. `cargo test --workspace` stays in CI: it takes
# minutes, too long to pay on every push.
#
# The repository is the one the push runs in: `git -C <dir>`, a leading
# `cd <dir> &&`, or else the hook's cwd. The range is the branch against
# origin/main, so a push that does not touch plugins/tribunal/rust costs nothing.
set -uo pipefail

INPUT=$(cat)
case "$INPUT" in
  *git*push*) ;;
  *) exit 0 ;;
esac

IFS=$'\t' read -r CMD_DIR CWD < <(printf '%s' "$INPUT" | node -e '
  let s = "";
  process.stdin.on("data", (d) => (s += d)).on("end", () => {
    try {
      const j = JSON.parse(s);
      const cmd = j.tool_input?.command ?? "";
      if (!/\bgit\b(?:\s+-C\s+\S+)?\s+push\b/.test(cmd)) return;
      const dir = cmd.match(/\bgit\s+-C\s+("?)([^"\s]+)\1/)?.[2]
        ?? cmd.match(/^\s*cd\s+("?)([^"\s]+)\1\s*&&/)?.[2] ?? "-";
      process.stdout.write(`${dir}\t${j.cwd ?? "."}\n`);
    } catch {}
  });
')
[ -n "${CWD:-}" ] || exit 0

DIR="$CWD"
if [ "$CMD_DIR" != "-" ]; then
  case "$CMD_DIR" in
    /*) DIR="$CMD_DIR" ;;
    "~"*) DIR="$HOME${CMD_DIR#\~}" ;;
    *) DIR="$CWD/$CMD_DIR" ;;
  esac
fi

TOP=$(git -C "$DIR" rev-parse --show-toplevel 2>/dev/null) || exit 0
ENGINE="$TOP/plugins/tribunal/rust"
[ -f "$ENGINE/rust-toolchain.toml" ] || exit 0
git -C "$TOP" rev-parse --verify -q origin/main >/dev/null || exit 0
[ -n "$(git -C "$TOP" diff --name-only origin/main...HEAD -- plugins/tribunal/rust)" ] || exit 0

cd "$ENGINE" || exit 0
if ! OUT=$(cargo fmt --all --check -- --color never 2>&1); then
  printf 'Push blocked: cargo fmt --all --check fails in %s. Run `cargo fmt --all` there and commit.\n%s\n' \
    "$ENGINE" "$(printf '%s\n' "$OUT" | head -40)" >&2
  exit 2
fi
if ! OUT=$(cargo clippy --color never --workspace --all-targets --locked -- -D warnings 2>&1); then
  printf 'Push blocked: cargo clippy -D warnings fails in %s.\n%s\n' \
    "$ENGINE" "$(printf '%s\n' "$OUT" | grep -E '^(warning|error)|-->' | head -40)" >&2
  exit 2
fi
exit 0
