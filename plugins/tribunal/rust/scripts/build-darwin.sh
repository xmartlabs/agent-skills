#!/usr/bin/env bash
# Builds the two macOS release archives on a Mac. The `build-darwin` job in
# .github/workflows/tribunal-rust.yml runs it with OUT_DIR set and uploads
# whatever lands there.
set -euo pipefail

# shellcheck source=mac-setup.sh
. "$(dirname "$0")/mac-setup.sh"
out=${OUT_DIR:?set OUT_DIR}
mkdir -p "$out"
out=$(cd "$out" && pwd)
ensure_rust
cd "$rust_dir"

# tribunal-v<version>-<target>: the job refuses a tag that is not the CLI
# version, so this is the name the tag gives the Linux and Windows archives.
# rust/scripts/pin-release.mjs and the launcher
# (skills/review-pr/scripts/rust-engine.mjs) read this shape.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/tribunal-cli/Cargo.toml | head -1)
stage=$(mktemp -d)
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target"
  cargo build --release --locked --target "$target" -p tribunal-cli
  name="tribunal-v$version-$target"
  mkdir -p "$stage/$name"
  cp "$CARGO_TARGET_DIR/$target/release/tribunal" "$stage/$name/"
  # No AppleDouble (._*) entries for extended attributes.
  COPYFILE_DISABLE=1 tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
  (cd "$out" && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
done

# The arm64 binary starts on the machine it was built for before it ships.
# The x86_64 one is not started, as on the GitHub macOS runner before.
"$CARGO_TARGET_DIR/aarch64-apple-darwin/release/tribunal" --version
