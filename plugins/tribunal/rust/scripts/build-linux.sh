#!/usr/bin/env bash
# Builds the Linux release archive, a static musl binary that runs on any
# distribution whatever its libc. The `build` job in
# .github/workflows/tribunal-rust.yml runs it with OUT_DIR set and uploads
# whatever lands there; the `linux-release-build` job runs the same on every
# pull request, so a release cannot fail on a build no pull request tried.
# Needs rustup and musl-gcc (Debian and Ubuntu: apt-get install musl-tools).
set -euo pipefail

rust_dir=$(cd "$(dirname "$0")/.." && pwd)
repo_root=$(cd "$rust_dir/../../.." && pwd)
# rust/.cargo/config.toml puts target/ at the repo root; name it outright.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo_root/target}
out=${OUT_DIR:?set OUT_DIR}
mkdir -p "$out"
out=$(cd "$out" && pwd)
target=x86_64-unknown-linux-musl

if [ "$(uname -s)-$(uname -m)" != Linux-x86_64 ]; then
  echo "build-linux.sh builds $target on an x86_64 Linux machine, not $(uname -s)-$(uname -m)" >&2
  exit 1
fi
# A crate builds C through cc, which looks for musl-gcc for this target.
command -v musl-gcc >/dev/null || { echo "musl-gcc is missing: apt-get install musl-tools" >&2; exit 1; }

cd "$rust_dir"
rustup toolchain install
rustup target add "$target"
cargo build --release --locked --target "$target" -p tribunal-cli

# tribunal-v<version>-<target>: the release job refuses a tag that is not the
# CLI version, and rust/scripts/pin-release.mjs and the launcher
# (bin/tribunal-engine) read this shape.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/tribunal-cli/Cargo.toml | head -1)
name="tribunal-v$version-$target"
stage=$(mktemp -d)
mkdir -p "$stage/$name"
cp "$CARGO_TARGET_DIR/$target/release/tribunal" "$stage/$name/"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")

# The binary starts on the machine it was built on before it ships, and has
# to be static: a dynamic one would need the build machine's libc.
"$stage/$name/tribunal" --version
kind=$(file -b "$stage/$name/tribunal")
case "$kind" in
  *"statically linked"* | *"static-pie linked"*) ;;
  *) echo "$name/tribunal is not static: $kind" >&2; exit 1 ;;
esac
