# Attributions

Code and data bundled into Deckhand that come from elsewhere. Each notice below must be preserved.

## argosy-sigil

Save sync reads and writes emulator saves through argosy-sigil, RomM's save library, compiled into
the binary from the `third_party/argosy-sigil` submodule, unmodified.

- Source: **rommapp/argosy-sigil** — <https://github.com/rommapp/argosy-sigil>
- License: **Mozilla Public License 2.0 (MPL-2.0)** — `third_party/argosy-sigil/LICENSE`. Its source
  files carry no *Incompatible With Secondary Licenses* notice, so a GPL-3.0 build may include them;
  the MPL still covers those files, and their source stays available at the link above.
- Its Sega CD backup RAM reader takes a table from **superctr/buram** (MIT) —
  `third_party/argosy-sigil/licenses/buram-MIT.txt`.

It brings these, compiled in with it:

- **libchdr** — <https://github.com/rtissera/libchdr> — BSD 3-Clause,
  `third_party/argosy-sigil/third_party/libchdr/LICENSE.txt`.
- **zlib** 1.3.1 — zlib License, in `libchdr/deps/zlib-1.3.1`.
- **Zstandard** 1.5.6 — BSD, `libchdr/deps/zstd-1.5.6/LICENSE`.
- **LZMA SDK** 24.05 — public domain.
- **tiny-AES-c** — <https://github.com/kokke/tiny-AES-c> — The Unlicense (public domain).
