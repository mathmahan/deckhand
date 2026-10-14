# Attributions

Data files bundled into Deckhand that come from elsewhere. Each notice below must be preserved.

## PlayStation 2 serial catalogue

`data/ps2-serials.json` maps PlayStation 2 disc serials to titles, which is how a PCSX2 folder
memory card's save folders — named by serial — are matched to a RomM game.

- Source: **niemasd/GameDB-PS2** — <https://github.com/niemasd/GameDB-PS2>
- License: **GNU General Public License v3.0 (GPL-3.0)** —
  <https://github.com/niemasd/GameDB-PS2/blob/main/LICENSE>
- Modifications: reduced to `serial → title`, by way of the Kotlin Deckhand's copy.

## PlayStation Portable serial catalogue

`data/psp-serials.json` is the PSP counterpart, matching PPSSPP `SAVEDATA` folders to RomM games.

- Source: **niemasd/GameDB-PSP** — <https://github.com/niemasd/GameDB-PSP>
- License: **GNU General Public License v3.0 (GPL-3.0)** —
  <https://github.com/niemasd/GameDB-PSP/blob/main/LICENSE>
- Modifications: reduced to `serial → title`, by way of the Kotlin Deckhand's copy.

> **Copyleft note:** both catalogues are GPL-3.0 and compiled into the binary, so a distributed build
> carries their obligations. Deckhand is itself GPL-3.0, which is what satisfies them. Relicensing
> the project to anything permissive would mean unbundling these first.
