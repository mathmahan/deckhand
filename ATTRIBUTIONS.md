# Attributions

Code and data bundled into Deckhand that come from elsewhere. Each notice below must be preserved.

The Rust crates Deckhand is built from are not listed here: [packaging/licences.sh](packaging/licences.sh)
collects their notices from `Cargo.lock`, adds the licence files of everything below, and the release
build puts the result in the AppImage as `usr/share/doc/deckhand/THIRD-PARTY-LICENSES.md`. Something
added here is added to that script too.

## argosy-sigil

Save sync reads and writes emulator saves through argosy-sigil, RomM's save library, compiled into
the binary from the `third_party/argosy-sigil` submodule, unmodified.

- Source: **rommapp/argosy-sigil** — <https://github.com/rommapp/argosy-sigil>
- License: **Mozilla Public License 2.0 (MPL-2.0)** — `third_party/argosy-sigil/LICENSE`. The MPL
  applies file by file, so an MIT-licensed build may include them: the MPL still covers those files,
  and their source stays available at the link above.
- Its Sega CD backup RAM reader takes a table from **superctr/buram** (MIT) —
  `third_party/argosy-sigil/licenses/buram-MIT.txt`.

It brings these, compiled in with it:

- **libchdr** — <https://github.com/rtissera/libchdr> — BSD 3-Clause,
  `third_party/argosy-sigil/third_party/libchdr/LICENSE.txt`.
- **zlib** 1.3.1 — zlib License, in `libchdr/deps/zlib-1.3.1`.
- **Zstandard** 1.5.6 — BSD, `libchdr/deps/zstd-1.5.6/LICENSE`.
- **LZMA SDK** 24.05 — public domain.
- **tiny-AES-c** — <https://github.com/kokke/tiny-AES-c> — The Unlicense (public domain).

## Fonts and icons

Built into the binary, and drawn on every screen:

- **DM Sans** — SIL Open Font License 1.1, `fonts/OFL.txt`.
- **Space Grotesk** — SIL Open Font License 1.1, `fonts/OFL-SpaceGrotesk.txt`.
- **Lucide** icons, through the `iced_fonts` crate — ISC, with the icons Lucide takes from Feather
  under MIT: `fonts/ISC-Lucide.txt`, from <https://github.com/lucide-icons/lucide>.
