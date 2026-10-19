//! Synthetic library for `--fixtures=N` — the UI with no RomM server, ported from
//! `browse/model/Fixtures.kt`.
//!
//! It exists to make the *layout* testable: mixed cover ratios on one shelf, a hero with and
//! without art, long titles against short ones. Fixture games carry no cover URL, so this exercises
//! the designed art-less path rather than the loaded-art one — which is the state most likely to be
//! wrong, because it is the one you never see on a healthy server.

use crate::library::{Game, Library, System};
use crate::systems;

const SYSTEMS: &[(&str, &str)] = &[
    ("snes", "Super Nintendo"),
    ("psx", "PlayStation"),
    ("gba", "Game Boy Advance"),
    ("n64", "Nintendo 64"),
    ("psp", "PSP"),
    ("nds", "Nintendo DS"),
];

const WORDS: &[&str] = &[
    "Chrono", "Legend", "Star", "Shadow", "Crystal", "Dragon", "Phantom", "Iron", "Silent",
    "Golden", "Mega", "Super", "Final", "Secret", "Castle", "Sky", "Ocean", "Ember", "Frost",
    "Riven", "Lunar", "Solar", "Vagrant", "Wild", "Broken",
];

const NOUNS: &[&str] = &[
    "Trigger", "Quest", "Odyssey", "Saga", "Chronicles", "Legacy", "Frontier", "Requiem",
    "Warrior", "Kingdom", "Circuit", "Runner", "Tactics", "Story", "Blade", "Empire",
];

/// A deterministic library of `count` games. Seeded from the index rather than a random source, so
/// two runs render exactly the same page — which is what makes a visual difference between runs
/// mean something.
pub fn library(count: usize) -> Library {
    let systems: Vec<System> = SYSTEMS
        .iter()
        .map(|(slug, name)| System {
            id: (*slug).to_string(),
            name: (*name).to_string(),
            cover_ratio: systems::ratio_for(slug),
        })
        .collect();

    let games = (0..count)
        .map(|i| {
            let system = &SYSTEMS[i % SYSTEMS.len()];
            let title = format!(
                "{} {}{}",
                WORDS[(i * 7) % WORDS.len()],
                NOUNS[(i * 5) % NOUNS.len()],
                match i % 4 {
                    0 => "",
                    1 => " II",
                    2 => ": Director's Cut",
                    _ => " Advance",
                }
            );
            // Every third game has been played, most recently first — enough to fill the hero and
            // the continue shelf without every card carrying a date.
            let last_played = (i % 3 == 0).then(|| {
                format!("2026-{:02}-{:02}T20:15:00", 1 + (i % 9), 1 + (i % 27))
            });
            Game {
                rom_id: 1000 + i as i64,
                // Fixtures are single roms: nothing here stands for a group of regional builds.
                sibling_ids: Vec::new(),
                // No files behind a fixture, so no save on disk can be one of these games'.
                file_stems: Vec::new(),
                sort_title: title.to_lowercase(),
                title,
                system_id: system.0.to_string(),
                cover_url: None,
                last_played,
                created_at: Some(format!("2026-{:02}-{:02}T10:00:00", 1 + (i % 12), 1 + (i % 28))),
                rating: (i % 5 != 0).then_some(2.5 + (i % 5) as f32 * 0.5),
                summary: (i % 2 == 0).then(|| {
                    "A sprawling adventure across a fractured world, where every settlement \
                     remembers the war differently and none of them agree on who won it."
                        .to_string()
                }),
                logo_path: None,
                miximage_path: None,
                marquee_path: None,
                // Enough detail facts to exercise the Detail page's chip row and stats without a
                // server: a couple of genres, a studio, a plausible size and a year.
                size_bytes: Some(4_194_304 * (1 + i as i64 % 12)),
                screenshots: Vec::new(),
                genres: vec!["Action".into(), "Adventure".into()],
                companies: vec!["Fixture Interactive".into()],
                players: Some(if i % 2 == 0 { "1".into() } else { "1-2".into() }),
                year: Some(1990 + (i % 20) as i32),
                fingerprint: None,
                multi_file: false,
                // Most matched on HowLongToBeat, some not — the detail page says N/A for those.
                beat_secs: (i % 4 != 0).then_some(1800 + 5400 * (i as u64 % 9)),
                // Nothing marked, hidden or missing: a fixture library is what Home's layout and its
                // tests are built against, and a test that wants one of these sets it.
                unavailable: false,
                build: None,
                alt_names: Vec::new(),
                achievements: 0,
                ra_id: None,
                status: None,
                backlogged: false,
                now_playing: false,
            }
        })
        .collect();

    Library::new(systems, games)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_fill_the_shelves_the_home_view_needs() {
        let lib = library(60);
        assert_eq!(lib.total_games, 60);
        assert!(lib.hero().is_some(), "a hero to open on");
        assert!(!lib.continue_playing(10).is_empty(), "a continue shelf");
        assert!(!lib.recently_added(12).is_empty(), "a recently-added shelf");
        // Mixed ratios on one shelf is the layout case fixtures exist to exercise.
        let ratios: Vec<f32> = lib.systems.iter().map(|s| s.cover_ratio).collect();
        assert!(ratios.iter().any(|r| *r > 1.0) && ratios.iter().any(|r| *r < 1.0));
    }

    #[test]
    fn fixtures_are_deterministic() {
        let a = library(20);
        let b = library(20);
        let titles = |l: &Library| l.all_games().iter().map(|g| g.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&a), titles(&b));
    }
}
