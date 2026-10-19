#!/bin/sh
# Write the licence notices of everything built into Deckhand to the file named by $1.
#
# The Rust crates come from Cargo.lock through cargo-about, for the Linux target the AppImage is
# built for: each crate's licence text as the crate ships it. about.toml lists the licences Deckhand
# may ship under its own, and cargo-about refuses — this script fails — on a crate under any other,
# or one whose licence it cannot make out. That makes this the check, too: CI runs it on every pull
# request. Then what cargo does not see is added from the licence files in this repository: the C
# libraries build.rs compiles from argosy-sigil, the fonts and the icons. ATTRIBUTIONS.md names the
# same things, for people.
#
# cargo-about is fetched on first use, pinned to a version and its checksum, into CARGO_TARGET_DIR's
# tools/ (target/tools/ when that is not set). Needs cargo on the PATH, curl and sha256sum.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?usage: licences.sh <file to write>}"
VERSION=0.9.2
SHA256=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af

TOOLS="${CARGO_TARGET_DIR:-$ROOT/target}/tools"
ABOUT="$TOOLS/cargo-about-$VERSION"
if [ ! -x "$ABOUT" ]; then
    echo "==> Fetching cargo-about $VERSION"
    mkdir -p "$TOOLS"
    NAME="cargo-about-$VERSION-x86_64-unknown-linux-musl"
    curl -fsSL -o "$TOOLS/$NAME.tar.gz" "https://github.com/EmbarkStudios/cargo-about/releases/download/$VERSION/$NAME.tar.gz"
    echo "$SHA256  $TOOLS/$NAME.tar.gz" | sha256sum -c - >/dev/null
    tar -xzf "$TOOLS/$NAME.tar.gz" -C "$TOOLS" "$NAME/cargo-about"
    mv "$TOOLS/$NAME/cargo-about" "$ABOUT"
    rm -r "$TOOLS/$NAME" "$TOOLS/$NAME.tar.gz"
fi

echo "==> Writing the licence notices to $OUT"
mkdir -p "$(dirname "$OUT")"
(cd "$ROOT" && "$ABOUT" -L error generate --locked --fail packaging/licences.hbs -o "$OUT")

# One notice: a heading naming the work and its licence, then the licence file as it is.
notice() {
    printf '### %s\n\n%s\n\n```text\n' "$1" "$2"
    cat "$ROOT/$3"
    printf '\n```\n\n'
}

SIGIL=third_party/argosy-sigil
CHDR=$SIGIL/third_party/libchdr
{
    printf '## C libraries, compiled in with argosy-sigil\n\n'
    notice "argosy-sigil" "Mozilla Public License 2.0. Its source: https://github.com/rommapp/argosy-sigil" "$SIGIL/LICENSE"
    notice "buram (a table in argosy-sigil's Sega CD reader)" "MIT. https://github.com/superctr/buram" "$SIGIL/licenses/buram-MIT.txt"
    notice "libchdr" "BSD 3-Clause. https://github.com/rtissera/libchdr" "$CHDR/LICENSE.txt"
    notice "zlib 1.3.1" "zlib License." "$CHDR/deps/zlib-1.3.1/LICENSE"
    notice "Zstandard 1.5.6" "BSD 3-Clause, taken from its BSD-or-GPLv2 choice." "$CHDR/deps/zstd-1.5.6/LICENSE"
    notice "LZMA SDK 24.05" "Public domain." "$CHDR/deps/lzma-24.05/LICENSE"
    notice "tiny-AES-c" "The Unlicense. https://github.com/kokke/tiny-AES-c" "$SIGIL/third_party/tiny-AES-c/unlicense.txt"
    printf '## Fonts and icons\n\n'
    notice "DM Sans" "SIL Open Font License 1.1." fonts/OFL.txt
    notice "Space Grotesk" "SIL Open Font License 1.1." fonts/OFL-SpaceGrotesk.txt
    notice "Lucide icons (through the iced_fonts crate)" "ISC, with the icons from Feather under MIT." fonts/ISC-Lucide.txt
} >> "$OUT"
