#!/usr/bin/env bash
# The full suite on macOS, run by the `macos` job in
# .github/workflows/tribunal-rust.yml. The tests ignored off macOS compare
# against its BSD userland, so this is where they run. The job fetches the
# whole history: the evidence-line tests replay this repository's commits.
set -euo pipefail

# shellcheck source=mac-setup.sh
. "$(dirname "$0")/mac-setup.sh"
ensure_rust
ensure_node

cd "$rust_dir"
cargo test --workspace --locked
