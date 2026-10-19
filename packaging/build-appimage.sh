#!/bin/sh
# Build target/appimage/Deckhand-<version>-x86_64.AppImage.
#
# The binary is built inside Debian bookworm (glibc 2.36) rather than on the host. A binary links
# against the glibc it was built with, and a rolling-release host's is newer than SteamOS's or most
# distributions' — built on one, the AppImage would refuse to start almost everywhere else. Nothing
# else needs bundling: the windowing, GPU and font libraries are opened at run time, and libudev
# (for gamepads) is on every systemd distribution and SteamOS.
#
# Requirements: docker, and `file`, which appimagetool calls. appimagetool is fetched on first use,
# and run extracted, so FUSE 2 is not needed on the build machine either.
#
# `--here` builds with the cargo already on the PATH instead of starting a container, for a caller
# that is a bookworm machine with libudev-dev, pkg-config and cmake installed — the release workflow's
# job. Either way the argosy-sigil submodule has to be checked out (`git submodule update --init
# --recursive`): build.rs compiles it.
# It builds into CARGO_TARGET_DIR when that is set, so a target/ kept between builds is used.
#
# Beside the binary go Deckhand's LICENSE and the notices of everything built into it, in
# usr/share/doc/deckhand/, which packaging/licences.sh writes from the same Cargo.lock — and refuses
# to write, failing the build, for a crate under a licence about.toml does not accept.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
OUT="$ROOT/target/appimage"
BUILD="$ROOT/target/bookworm"
mkdir -p "$OUT" "$BUILD"

echo "==> Building deckhand $VERSION against glibc 2.36"
if [ "${1:-}" = --here ]; then
    BUILD="${CARGO_TARGET_DIR:-$BUILD}"
    (cd "$ROOT" && CARGO_TARGET_DIR="$BUILD" cargo build --release --locked)
    CARGO_TARGET_DIR="$BUILD" "$ROOT/packaging/licences.sh" "$BUILD/THIRD-PARTY-LICENSES.md"
else
    mkdir -p "${CARGO_HOME:-$HOME/.cargo}/registry"
    # As root, because gilrs links libudev and its headers have to be installed first; everything it
    # wrote is handed back to the calling user afterwards.
    docker run --rm \
        -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/build \
        -v "$ROOT:/src:ro" -v "$BUILD:/build" \
        -v "${CARGO_HOME:-$HOME/.cargo}/registry:/tmp/cargo/registry" \
        -w /src rust:1-bookworm \
        sh -c "apt-get update -qq && apt-get install -y -qq libudev-dev pkg-config cmake >/dev/null \
            && cargo build --release --locked && packaging/licences.sh /build/THIRD-PARTY-LICENSES.md; status=\$?; \
            chown -R $(id -u):$(id -g) /build /tmp/cargo/registry; exit \$status"
fi

APPDIR="$OUT/Deckhand.AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/512x512/apps" "$APPDIR/usr/share/doc/deckhand"
install -m755 "$BUILD/release/deckhand" "$APPDIR/usr/bin/deckhand"
install -m644 "$ROOT/LICENSE" "$APPDIR/usr/share/doc/deckhand/LICENSE"
install -m644 "$BUILD/THIRD-PARTY-LICENSES.md" "$APPDIR/usr/share/doc/deckhand/THIRD-PARTY-LICENSES.md"
install -m755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"
install -m644 "$ROOT/packaging/deckhand.desktop" "$APPDIR/deckhand.desktop"
install -m644 "$ROOT/packaging/deckhand.desktop" "$APPDIR/usr/share/applications/deckhand.desktop"
install -m644 "$ROOT/packaging/deckhand.png" "$APPDIR/deckhand.png"
install -m644 "$ROOT/packaging/deckhand.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/deckhand.png"
ln -sf deckhand.png "$APPDIR/.DirIcon"

TOOL="$OUT/appimagetool-x86_64.AppImage"
if [ ! -x "$TOOL" ]; then
    echo "==> Fetching appimagetool"
    curl -fL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi

IMAGE="$OUT/Deckhand-$VERSION-x86_64.AppImage"
echo "==> Packing $IMAGE"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$IMAGE"
echo "==> $IMAGE"
