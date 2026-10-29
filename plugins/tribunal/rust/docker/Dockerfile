# Release builds of `tribunal` for Linux and Windows, and a smoke test of
# each on a machine that has nothing else installed. Run from
# plugins/tribunal (the build reads ../lib through include_str!):
#
#   docker buildx build -f rust/docker/Dockerfile --target linux-smoke .
#   docker buildx build -f rust/docker/Dockerfile --target windows-smoke .
#
# or rust/docker/check.sh for both. macOS builds run on a macOS runner
# (rust/scripts/build-darwin.sh); a container cannot host one. Windows here is a cross build checked under
# Wine: it proves the binary links and starts, not that process-tree
# cleanup or PATHEXT resolution behave as on real Windows.

ARG RUST_IMAGE=rust:1.99.1-trixie

FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS toolchain
RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools gcc-mingw-w64-x86-64 \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add "$(uname -m)-unknown-linux-musl" x86_64-pc-windows-gnu \
 && rustup component add clippy
WORKDIR /src
COPY lib ./lib
COPY rust ./rust
WORKDIR /src/rust
# Overrides rust/.cargo/config.toml, whose target-dir is relative to the repo.
ENV CARGO_TARGET_DIR=/build

# A static musl binary runs on any distribution, whatever its libc.
FROM toolchain AS linux-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build \
    target="$(uname -m)-unknown-linux-musl" \
 && cargo build --release --locked --target "$target" -p tribunal-cli \
 && mkdir -p /out && cp "/build/$target/release/tribunal" /out/

FROM toolchain AS windows-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build \
    cargo build --release --locked --target x86_64-pc-windows-gnu -p tribunal-cli \
 && mkdir -p /out && cp /build/x86_64-pc-windows-gnu/release/tribunal.exe /out/

# `--output type=local` targets: the binaries alone, not the toolchain.
FROM scratch AS linux-bin
COPY --from=linux-build /out/ /

FROM scratch AS windows-bin
COPY --from=windows-build /out/ /

FROM ubuntu:24.04 AS linux-smoke
COPY --from=linux-build /out/tribunal /usr/local/bin/tribunal
RUN tribunal --version

# Wine runs x86_64 Windows binaries only on an x86_64 host.
FROM --platform=linux/amd64 debian:trixie-slim AS windows-smoke
RUN apt-get update \
 && apt-get install -y --no-install-recommends wine wine64 \
 && rm -rf /var/lib/apt/lists/*
ENV WINEDEBUG=-all WINEPREFIX=/wine
COPY --from=windows-build /out/tribunal.exe /opt/tribunal.exe
RUN wine /opt/tribunal.exe --version

# The target the release ships, x86_64-pc-windows-msvc, cross-built on Linux:
# cargo-xwin fetches Microsoft's CRT and Windows SDK, which accepts their
# license (https://go.microsoft.com/fwlink/?LinkId=2086102), and links with
# clang and lld. The `windows` job in .github/workflows/tribunal-rust.yml
# builds `windows-msvc-check`; `windows-msvc-bin` holds the release binary.
# Wine runs the unit tests: like the gnu smoke above, it shows the code
# builds, links and passes its tests, not that process-tree cleanup or
# PATHEXT resolution behave as on real Windows.
FROM --platform=linux/amd64 ${RUST_IMAGE} AS msvc
# Not XWIN_VERSION: cargo-xwin reads that variable as its --xwin-version.
ARG CARGO_XWIN_RELEASE=0.23.1
RUN apt-get update \
 && apt-get install -y --no-install-recommends clang lld llvm wine wine64 \
 && rm -rf /var/lib/apt/lists/*
RUN base="https://github.com/rust-cross/cargo-xwin/releases/download/v$CARGO_XWIN_RELEASE" \
 && name="cargo-xwin-v$CARGO_XWIN_RELEASE.x86_64-unknown-linux-musl.tar.gz" \
 && curl -fsSL -o "/tmp/$name" "$base/$name" \
 && curl -fsSL "$base/$name.sha256" | awk -v f="/tmp/$name" '{ print $1 "  " f }' | sha256sum -c - \
 && tar -xzf "/tmp/$name" -C /usr/local/bin cargo-xwin \
 && rm "/tmp/$name"
RUN rustup target add x86_64-pc-windows-msvc && rustup component add clippy
ENV XWIN_ACCEPT_LICENSE=1 XWIN_CACHE_DIR=/xwin \
    WINEDEBUG=-all WINEPREFIX=/wine \
    CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine
WORKDIR /src
# The unit tests read the agent definitions next to the crates.
COPY agents ./agents
COPY lib ./lib
COPY rust ./rust
WORKDIR /src/rust
ENV CARGO_TARGET_DIR=/build

# What the `windows` job checked on a GitHub Windows runner: clippy, the unit
# tests, the release build, and the binary starting.
FROM msvc AS windows-msvc-check
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/xwin \
    --mount=type=cache,target=/build \
    target=x86_64-pc-windows-msvc \
 && cargo xwin clippy --target "$target" --workspace --lib --bins --locked -- -D warnings \
 && cargo xwin test --target "$target" --workspace --lib --bins --locked \
 && cargo xwin build --release --locked --target "$target" -p tribunal-cli \
 && wine "/build/$target/release/tribunal.exe" --version \
 && mkdir -p /out && cp "/build/$target/release/tribunal.exe" /out/

FROM scratch AS windows-msvc-bin
COPY --from=windows-msvc-check /out/ /
