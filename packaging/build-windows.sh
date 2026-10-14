#!/bin/sh
# Build target/windows/Deckhand-<version>-x86_64.exe.
#
# Cross-compiled for x86_64-pc-windows-gnu inside Debian bookworm with MinGW-w64, so no Windows
# machine or MSVC toolchain is needed. The result is one self-contained .exe: Rust's own runtime and
# MinGW's are linked in statically, and the graphics (DirectX 12 or Vulkan) and gamepad (XInput)
# libraries it opens are part of Windows.
#
# Requirements: docker, and the argosy-sigil submodule checked out (`git submodule update --init
# --recursive`), which build.rs cross-compiles with the same MinGW.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
OUT="$ROOT/target/windows"
BUILD="$ROOT/target/windows-release"
mkdir -p "$OUT" "$BUILD" "${CARGO_HOME:-$HOME/.cargo}/registry"

echo "==> Building deckhand $VERSION for Windows"
docker run --rm \
    -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/build \
    -v "$ROOT:/src:ro" -v "$BUILD:/build" \
    -v "${CARGO_HOME:-$HOME/.cargo}/registry:/tmp/cargo/registry" \
    -w /src rust:1-bookworm \
    sh -c "apt-get update -qq && apt-get install -y -qq gcc-mingw-w64-x86-64 cmake >/dev/null \
        && rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 \
        && cargo build --release --locked --target x86_64-pc-windows-gnu; status=\$?; \
        chown -R $(id -u):$(id -g) /build /tmp/cargo/registry; exit \$status"

install -m755 "$BUILD/x86_64-pc-windows-gnu/release/deckhand.exe" "$OUT/Deckhand-$VERSION-x86_64.exe"
echo "==> $OUT/Deckhand-$VERSION-x86_64.exe"
