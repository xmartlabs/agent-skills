#!/usr/bin/env bash
# PostToolUse on Write|Edit: after Claude edits a file of the tribunal Rust
# engine, run `cargo fmt --all` there, so the change never reaches CI's
# `cargo fmt --all --check` unformatted.
#
# The engine root is derived from the edited path, not from the project dir, so
# the hook also covers edits made in a sibling worktree. The toolchain pinned in
# rust-toolchain.toml is the one CI uses, and `cargo fmt` picks it up from there.
# Never blocks: a file that does not parse yet is left as it is.
set -uo pipefail

FILE=$(node -e '
  let s = "";
  process.stdin.on("data", (d) => (s += d)).on("end", () => {
    try {
      const j = JSON.parse(s);
      process.stdout.write(j.tool_input?.file_path ?? j.tool_response?.filePath ?? "");
    } catch {}
  });
')

case "$FILE" in
  */plugins/tribunal/rust/*.rs) ;;
  *) exit 0 ;;
esac

ROOT="${FILE%%/plugins/tribunal/rust/*}/plugins/tribunal/rust"
[ -f "$ROOT/rust-toolchain.toml" ] || exit 0

if ! (cd "$ROOT" && cargo fmt --all) >/dev/null 2>&1; then
  echo "rustfmt-on-edit: cargo fmt failed in $ROOT, the file is left unformatted" >&2
fi
exit 0
