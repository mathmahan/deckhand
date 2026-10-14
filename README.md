> [!IMPORTANT]
> **This entire project was vibe-coded with AI assistance.** The code was built iteratively through prompting, testing, debugging and refinement rather than through a traditional from-scratch coding workflow. Please backup your saves before using this app.


<p align="center"><img src="packaging/deckhand.png" width="128" alt="Deckhand logo"></p>

# Deckhand

A gamepad-first companion for the [RomM](https://romm.app) ROM manager, built for the Steam Deck.
Browse your library from the couch, download games onto the device, play them through RetroDECK,
and keep saves and save states in sync with your server.

![Home](screenshots/home.jpg)

## Features

- **Pairing without a keyboard.** Scan a QR code with your phone and approve Deckhand in RomM, or
  type a pairing code.
- **Home.** A spotlight on the game the cursor is on, over its own screenshot, with A to play it and
  X for its saves. Below it are shelves for *Continue playing*, *On this device*, your RomM *Now
  playing*, *Backlog* and *Finished* lists, and RomM's collections.
- **Library and Collections.** Every system or collection as a cover grid, with filters, sorting and
  an A–Z jump. Your favourites and your own collections are a chip each, and RomM's genres,
  franchises and series are browsable across systems.
- **Search.** Ranked search across every system, by title and by other names, so `ff7` finds
  *Final Fantasy VII*. An on-screen keyboard makes it work with a pad.
- **Game pages.** Art, rating, How Long to Beat, regions and builds, and the game's
  RetroAchievements, with the ones you have earned marked. *Favourite* marks it in RomM.
- **Downloads.** A queue that survives restarts, unpacks archives, writes an `.m3u` for multi-disc
  games and fetches the BIOS. Games can go to any drive (an SD card, for instance), with an optional
  space budget. Games already in the folder are found and count as downloaded. With RomM 5.4 and a
  QR pairing, a game can be sent to the Deck from RomM's web app with *Install on device*.
- **Playing.** Games start through RetroDECK, a command of your own, or an emulator picked per
  system, such as a PCSX2 you installed yourself. They can resume from their newest save state.
  RomM 5.4's web app shows the game as being played on the Deck.
- **Save sync.** Battery saves and save states sync both ways through RomM: around every launch,
  and while the window is open, as soon as a save changes. An older save can be restored from a
  game's Saves page.
- **Steam and ES-DE.** Downloaded games can be added to Steam with their art, and still sync when
  started there. Games started from ES-DE sync too, through hooks Deckhand installs.
- **Made for a pad.** The d-pad reaches every page and button, and L1/R1 move between tabs.
- **Updates itself** from this repository's releases.

![Browse](screenshots/browse.jpg)

## Getting started

Download the AppImage (Linux, Steam Deck) from
[Releases](https://github.com/mathmahan/deckhand/releases), then run it:

```sh
chmod +x Deckhand-*-x86_64.AppImage
./Deckhand-*-x86_64.AppImage
```

You need a [RomM](https://docs.romm.app) server, 5.3 or later for QR pairing. To see which version
yours runs:

```sh
curl -s https://romm.example.com/api/heartbeat | jq -r .SYSTEM.VERSION
```

Games play through RetroDECK. Install it from Flathub, then open it once to set it up:

```sh
flatpak install flathub net.retrodeck.retrodeck
```

Without RetroDECK, choose *Your own command…* under Settings ▸ Playing ▸ *Default emulator*. That
command starts every game. `{rom}` is replaced by the game's file (or added at the end if you
leave it out), and `{system}` by its ES-DE system name, such as `snes`, `psx` or `gc`. For example:

```sh
# RetroArch with one core. The -L also tells save sync which core runs the game.
retroarch -L /usr/lib/libretro/snes9x_libretro.so {rom}

# The RetroArch Flatpak
flatpak run org.libretro.RetroArch -L /home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores/mgba_libretro.so {rom}
```

To give one system its own emulator, such as a PCSX2 you installed yourself, use Settings ▸ Playing ▸
*Emulator for each system*. Choose the emulator (a program, an AppImage, or a Flatpak) and
Deckhand fills in the usual options for it, for you to check:

```sh
# PlayStation 2, with the PCSX2 AppImage
/home/deck/Applications/pcsx2-v2.2.0-linux-appimage-x64-Qt.AppImage -batch {rom}

# GameCube, with the Dolphin Flatpak
/var/lib/flatpak/exports/bin/org.DolphinEmu.dolphin-emu -b -e {rom}
```

Saves sync for RetroArch, and on Linux for PCSX2, Dolphin, PPSSPP, RPCS3, Vita3K, Azahar and Cemu.

## Building

```sh
cargo run                      # the app
cargo run -- --fixtures=60     # 60 made-up games, no server needed
cargo test
packaging/build-appimage.sh    # AppImage, built in Docker
packaging/build-windows.sh     # Windows .exe, cross-compiled in Docker
```

## License

GPL-3.0, like the app it ports. DM Sans and Space Grotesk are under the SIL OFL 1.1
([DM Sans](fonts/OFL.txt), [Space Grotesk](fonts/OFL-SpaceGrotesk.txt)) and the Lucide icons under
ISC. Bundled game data is credited in [ATTRIBUTIONS.md](ATTRIBUTIONS.md).
