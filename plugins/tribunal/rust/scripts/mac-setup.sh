#!/usr/bin/env bash
# Sourced by the scripts that run on a Mac (build-darwin.sh, test-macos.sh).
# A Mac may have no Rust and may have another node, so these install what is
# missing. Each download
# is checked against the sha256 its publisher serves next to it.

rust_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo_root=$(cd "$rust_dir/../../.." && pwd)
# rust/.cargo/config.toml puts target/ at the repo root; name it outright.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo_root/target}

# Fails unless file $1 hashes to the first field of the text at URL $2.
verify_sha256() {
  local want got
  want=$(curl --proto '=https' --tlsv1.2 -fsSL "$2" | cut -d' ' -f1)
  got=$(shasum -a 256 "$1" | cut -d' ' -f1)
  if [ -z "$want" ] || [ "$want" != "$got" ]; then
    echo "checksum mismatch for $1 (want $want, got $got)" >&2
    return 1
  fi
}

# rustup, then the toolchain rust-toolchain.toml names.
ensure_rust() {
  if ! command -v rustup >/dev/null; then
    local base tmp
    base="https://static.rust-lang.org/rustup/dist/$(uname -m | sed 's/arm64/aarch64/')-apple-darwin"
    tmp=$(mktemp -d)
    curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp/rustup-init" "$base/rustup-init"
    verify_sha256 "$tmp/rustup-init" "$base/rustup-init.sha256"
    chmod +x "$tmp/rustup-init"
    "$tmp/rustup-init" -y --profile minimal --default-toolchain none
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  fi
  (cd "$rust_dir" && rustup toolchain install)
}

# Node 24, the version the macOS job ran under setup-node. Node 23 and later
# compile regexp modifiers such as (?-i:...), which the profile checks are
# measured against.
ensure_node() {
  # A node installed through asdf is a shim, which picks the version from
  # $HOME; tests that run node with HOME set to a scratch dir then get "No
  # version is set for command node". Put the real binary first instead.
  if [ "$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null)" = 24 ]; then
    PATH="$(dirname "$(node -p process.execPath)"):$PATH"
    export PATH
    node --version
    return
  fi
  local base sums line tmp dir="$HOME/.node24"
  base="https://nodejs.org/dist/latest-v24.x"
  sums=$(curl --proto '=https' --tlsv1.2 -fsSL "$base/SHASUMS256.txt")
  # "<sha256>  node-v24.x.y-darwin-<arch>.tar.gz"
  line=$(grep -E "  node-v24\.[0-9.]+-darwin-$(uname -m | sed 's/x86_64/x64/')\.tar\.gz$" <<<"$sums")
  tmp=$(mktemp -d)
  curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp/node.tar.gz" "$base/${line##* }"
  if [ "$(shasum -a 256 "$tmp/node.tar.gz" | cut -d' ' -f1)" != "${line%% *}" ]; then
    echo "checksum mismatch for ${line##* }" >&2
    return 1
  fi
  mkdir -p "$dir"
  tar -xzf "$tmp/node.tar.gz" -C "$dir" --strip-components 1
  export PATH="$dir/bin:$PATH"
  node --version
}
