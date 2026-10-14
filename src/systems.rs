//! Per-system cover ratios — the shelf-geometry input, ported from `browse/data/SystemCatalog.kt`.
//!
//! Data, not code, in the Kotlin app: a user-editable `systems.json` whose other half is the
//! launcher table (emulator command, args, core). This build launches nothing, so only the ratio
//! survives — and with nothing user-editable left worth a file, the table is compiled in. It comes
//! back as a file the moment launching does.
//!
//! The ratio is width/height of that system's box art as RomM serves it. It matters because a shelf
//! mixes systems: a PSP box is 0.58 and a SNES one is 1.37, and cards only share a bottom baseline if
//! each one's width follows its own ratio at the shelf's shared height. It matters on the Browse grid
//! too, where a cover more than a few percent off its tile's shape is drawn whole inside it rather
//! than filling it — so a wrong figure here shows as a box around every cover of the system.
//!
//! The figures were measured: the median of RomM's covers for each system on a 3,000-game library
//! (October 2026), where they differed from the physical box or were missing. RomM's own slugs are
//! listed alongside the longer ones (`dc` beside `dreamcast`) because they are what arrives.

pub struct SystemConfig;

impl SystemConfig {
    /// The ratio for a system with no row below — a portrait box, which most are.
    pub const DEFAULT_RATIO: f32 = 0.71;
}

/// Slug → the name Deckhand shows, where it differs from RomM's.
///
/// RomM reports a platform's full formal name — "Nintendo Entertainment System", "Sony PlayStation
/// Portable" — which is right for a database and wrong for a card 190 units wide: it wraps to two
/// lines and pushes the game count out of the card. These are the names the Kotlin app's system
/// catalogue uses. Anything not listed keeps whatever RomM called it.
const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("nes", "NES"),
    ("snes", "Super Nintendo"),
    ("n64", "Nintendo 64"),
    ("nds", "Nintendo DS"),
    ("ngc", "GameCube"),
    ("gc", "GameCube"),
    ("gb", "Game Boy"),
    ("gbc", "Game Boy Color"),
    ("gba", "Game Boy Advance"),
    ("genesis-slash-megadrive", "MD/Genesis"),
    ("genesis", "MD/Genesis"),
    ("sms", "Master System"),
    ("mastersystem", "Master System"),
    ("gamegear", "Game Gear"),
    ("psx", "PlayStation"),
    ("ps", "PlayStation"),
    ("ps2", "PlayStation 2"),
    ("ps3", "PlayStation 3"),
    ("psp", "PSP"),
    ("psvita", "PS Vita"),
    ("wiiu", "Wii U"),
    ("3ds", "Nintendo 3DS"),
    ("saturn", "Saturn"),
    ("segacd", "Sega CD"),
    ("atari2600", "Atari 2600"),
    ("atari5200", "Atari 5200"),
    ("atari7800", "Atari 7800"),
    ("jaguar", "Atari Jaguar"),
];

/// The name to show for a platform: Deckhand's short one where there is one, else RomM's own.
pub fn display_name(slug: &str, romm_name: &str) -> String {
    let key = slug.trim().to_lowercase();
    DISPLAY_NAMES
        .iter()
        .find(|(s, _)| *s == key)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| romm_name.to_string())
}

/// Slug → cover ratio. Keyed by RomM's platform slug.
const RATIOS: &[(&str, f32)] = &[
    ("nes", 0.73),
    ("snes", 1.37),
    ("gb", 1.00),
    ("gbc", 1.00),
    ("gba", 1.00),
    ("n64", 1.37),
    ("genesis", 0.71),
    ("genesis-slash-megadrive", 0.71),
    ("mastersystem", 0.71),
    ("sms", 0.71),
    ("gamegear", 0.71),
    ("dreamcast", 1.00),
    ("dc", 1.00),
    ("gc", 0.71),
    ("ngc", 0.71),
    ("wii", 0.71),
    ("psx", 1.16),
    ("ps", 1.16),
    ("ps2", 0.71),
    ("ps3", 0.80),
    ("psp", 0.58),
    ("psvita", 0.72),
    ("nds", 1.11),
    ("3ds", 0.71),
    ("switch", 0.71),
    ("arcade", 0.75),
    ("scummvm", 0.75),
    ("neogeo", 0.75),
    ("saturn", 0.65),
    ("segacd", 0.71),
    ("32x", 0.71),
    ("sega32", 0.71),
    ("tg16", 0.85),
    ("turbografx-cd", 1.00),
    ("atari5200", 0.73),
    ("atari7800", 0.73),
    ("jaguar", 0.73),
    ("atari2600", 0.75),
    ("c64", 0.75),
    ("amiga", 0.75),
    ("dos", 0.78),
];

/// The cover ratio for a RomM platform slug, falling back to a portrait box.
///
/// Matched case-insensitively, and with RomM's `-slash-` compound slugs
/// (`genesis-slash-megadrive`) also tried by their leading segment — the same platform reaches this
/// under either spelling depending on the server's metadata source.
pub fn ratio_for(slug: &str) -> f32 {
    let key = slug.trim().to_lowercase();
    if let Some((_, r)) = RATIOS.iter().find(|(s, _)| *s == key) {
        return *r;
    }
    let head = key.split("-slash-").next().unwrap_or(&key);
    RATIOS
        .iter()
        .find(|(s, _)| *s == head)
        .map(|(_, r)| *r)
        .unwrap_or(SystemConfig::DEFAULT_RATIO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_systems_keep_their_box_shape() {
        assert_eq!(ratio_for("snes"), 1.37, "a SNES box is landscape");
        assert_eq!(ratio_for("psp"), 0.58, "a PSP box is tall and narrow");
        assert_eq!(ratio_for("dc"), 1.00, "RomM's own slug for the Dreamcast");
        assert_eq!(ratio_for("SNES"), 1.37, "slugs match case-insensitively");
    }

    #[test]
    fn romm_compound_slugs_resolve_to_the_same_shape() {
        assert_eq!(ratio_for("genesis-slash-megadrive"), 0.71);
    }

    #[test]
    fn long_romm_names_are_shortened_but_unknown_ones_pass_through() {
        assert_eq!(display_name("nes", "Nintendo Entertainment System"), "NES");
        assert_eq!(display_name("ngc", "Nintendo GameCube"), "GameCube");
        // A platform with no override keeps whatever the server called it, rather than falling
        // back to a slug the user has never seen.
        assert_eq!(display_name("pico8", "PICO-8"), "PICO-8");
    }

    #[test]
    fn an_unknown_system_gets_a_portrait_box() {
        assert_eq!(ratio_for("some-new-console"), SystemConfig::DEFAULT_RATIO);
    }
}
