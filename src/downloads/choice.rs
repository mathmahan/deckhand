//! What can be downloaded for one game — a port of the Kotlin app's `GameVariants`, with discs that
//! bring their tracks along.
//!
//! A game in RomM is not one downloadable thing. It is two kinds of set at once:
//!
//! - **Builds** — a region, a revision, a hack, a translation. RomM keeps each as a rom of its own,
//!   linked as *siblings*, with its own files and its own saves; the library shows one tile for all
//!   of them.
//! - **Discs** — a multi-disc game is several files on *one* rom, fetched together and collapsed
//!   into a generated `.m3u` so the emulator can swap between them.
//!
//! So Download asks — when there is anything to ask. The rom's own files come first: the whole set
//! (what the queue has always fetched, and the only thing that yields a working `.m3u`), then each
//! disc on its own, for the "I only want disc 1" or "I deleted disc 3" case. Then the other builds,
//! each fetched whole. All pure: the choices are built from RomM's detail of the rom and what is
//! already on disk, so the interesting cases are unit tests.

use crate::library::Game;
use crate::romm::models::{RomDetail, RomFile};

use super::Installed;

/// One thing Download can fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The rom to fetch — the game's own, or a sibling build's.
    pub rom_id: i64,
    /// "All 3 discs", "Disc 2", "Europe · Rev 1".
    pub label: String,
    /// A line under it: the file it is, or what it weighs.
    pub detail: String,
    /// Which of the rom's files, by RomM's id — None for all of them.
    pub files: Option<Vec<i64>>,
    /// What it weighs, when RomM said.
    pub bytes: Option<u64>,
    /// Already on this device: marked rather than hidden, so the set stays legible.
    pub installed: bool,
}

/// One disc of a multi-disc game: its entry point and the files that travel with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Disc {
    pub label: String,
    /// The file an emulator is pointed at — `Final Fantasy VII (USA) (Disc 2).cue`.
    pub entry: String,
    pub files: Vec<i64>,
    pub bytes: u64,
}

/// Files an emulator is pointed at as one disc. A `.bin` track is not one: it belongs to its
/// `.cue`, and listing it would make a one-disc game look like two.
const DISC_IMAGES: [&str; 13] = ["cue", "chd", "iso", "pbp", "gdi", "img", "cso", "ciso", "mdf", "nrg", "rvz", "wbfs", "wia"];

fn extension(name: &str) -> String {
    super::layout::extension(name)
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// The discs of a rom's files, in disc order — empty unless there are at least two.
///
/// A disc is one entry point and everything filed with it: a `.cue`'s tracks, which Redump names
/// after the cue with a `(Track N)` tag, ride along. An image a `.cue` of the same name points at
/// (a CloneCD `.img`) is that cue's track, not a disc of its own.
pub fn discs(files: &[RomFile]) -> Vec<Disc> {
    let game: Vec<&RomFile> = files.iter().filter(|f| f.is_game()).collect();
    let cues: Vec<&str> = game.iter().filter(|f| extension(&f.file_name) == "cue").map(|f| stem(&f.file_name)).collect();
    let entries: Vec<&RomFile> = game
        .iter()
        .copied()
        .filter(|f| {
            let ext = extension(&f.file_name);
            DISC_IMAGES.contains(&ext.as_str()) && (ext == "cue" || !cues.contains(&stem(&f.file_name)))
        })
        .collect();
    if entries.len() < 2 {
        return Vec::new();
    }
    let mut discs: Vec<Disc> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let own = stem(&entry.file_name);
            let tracks = format!("{own} (");
            let members: Vec<&RomFile> = game
                .iter()
                .copied()
                .filter(|f| {
                    f.id == entry.id
                        || (!entries.iter().any(|e| e.id == f.id) && {
                            let other = stem(&f.file_name);
                            other == own || other.starts_with(&tracks)
                        })
                })
                .collect();
            Disc {
                label: disc_label(&entry.file_name).unwrap_or_else(|| format!("Disc {}", i + 1)),
                entry: entry.file_name.clone(),
                files: members.iter().map(|f| f.id).collect(),
                bytes: members.iter().map(|f| f.size()).sum(),
            }
        })
        .collect();
    discs.sort_by_key(|d| natural(&d.entry));
    discs
}

/// `(Disc 2)` or `(Disk 2)` in a name, as "Disc 2".
fn disc_label(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    for tag in ["(disc ", "(disk "] {
        if let Some(at) = lower.find(tag) {
            let rest = &name[at + tag.len()..];
            let number: String = rest.chars().take_while(|c| *c != ')').collect();
            if !number.trim().is_empty() {
                return Some(format!("Disc {}", number.trim()));
            }
        }
    }
    None
}

/// A name with its numbers padded, so "Disc 10" sorts after "Disc 9".
fn natural(name: &str) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    for c in name.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            out.push_str(&format!("{digits:0>8}"));
            digits.clear();
        }
        out.push(c);
    }
    out.to_lowercase()
}

/// What tells one build of a game from another, from its file name: every parenthesised group —
/// `Zelda (Europe) (Rev 1)` is "Europe · Rev 1" — or "Original" for a name with none.
pub fn build_label(name: &str) -> String {
    let groups: Vec<&str> = name
        .split('(')
        .skip(1)
        .filter_map(|part| part.split_once(')').map(|(inside, _)| inside.trim()))
        .filter(|inside| !inside.is_empty())
        .collect();
    if groups.is_empty() { "Original".to_string() } else { groups.join(" · ") }
}

/// What Download offers for `game`, given RomM's detail of its own rom (`detail`, when fetched) and
/// what is on this device (`installed`, every build's record that exists).
pub fn choices(game: &Game, detail: Option<&RomDetail>, installed: &[&Installed]) -> Vec<Choice> {
    let record = |rom_id: i64| installed.iter().find(|i| i.rom_id == rom_id);
    let own_name = game.file_stems.iter().find(|(id, _)| *id == game.rom_id).map(|(_, s)| s.as_str()).unwrap_or(&game.title);
    let mut out = Vec::new();
    let discs = detail.map(|d| discs(&d.files)).unwrap_or_default();
    // What the whole game weighs: its files, its update and its DLC — not the extras it leaves on RomM.
    let total = detail
        .map(|d| d.files.iter().filter(|f| f.comes_with_game()).map(RomFile::size).sum::<u64>())
        .filter(|b| *b > 0)
        .or(game.size_bytes.and_then(|b| u64::try_from(b).ok()));
    let own = record(game.rom_id);
    if game.unavailable {
        // RomM has lost this build's file: only the others are offered.
    } else if discs.is_empty() {
        out.push(Choice {
            rom_id: game.rom_id,
            // RomM's own reading of the file name's tags, where it made one.
            label: game.build.clone().unwrap_or_else(|| build_label(own_name)),
            detail: own_name.to_string(),
            files: None,
            bytes: total,
            installed: own.is_some_and(|i| i.subset.is_none()),
        });
    } else {
        out.push(Choice {
            rom_id: game.rom_id,
            label: format!("All {} discs", discs.len()),
            detail: "keeps disc swapping working".into(),
            files: None,
            bytes: total,
            installed: own.is_some_and(|i| i.subset.is_none()),
        });
        for disc in &discs {
            let here = own.is_some_and(|i| i.subset.is_none() || disc.files.iter().all(|f| i.source_files.contains(f)));
            out.push(Choice {
                rom_id: game.rom_id,
                label: disc.label.clone(),
                detail: disc.entry.clone(),
                files: Some(disc.files.clone()),
                bytes: Some(disc.bytes),
                installed: here,
            });
        }
    }
    for (rom_id, name) in game.file_stems.iter().filter(|(id, _)| *id != game.rom_id) {
        out.push(Choice {
            rom_id: *rom_id,
            label: build_label(name),
            detail: name.clone(),
            files: None,
            bytes: None,
            installed: record(*rom_id).is_some(),
        });
    }
    // A sibling RomM names but gives no file name for still has a build to offer.
    for rom_id in game.sibling_ids.iter().filter(|id| !game.file_stems.iter().any(|(s, _)| s == *id)) {
        out.push(Choice {
            rom_id: *rom_id,
            label: format!("Build #{rom_id}"),
            detail: "another build of this game".into(),
            files: None,
            bytes: None,
            installed: record(*rom_id).is_some(),
        });
    }
    out
}

/// Whether pressing Download has a question to ask before fetching anything: another build to
/// choose, or files that may be discs — which needs RomM's detail of the rom to answer.
pub fn may_ask(game: &Game) -> bool {
    !game.sibling_ids.is_empty() || game.multi_file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: i64, name: &str, bytes: i64) -> RomFile {
        RomFile { id, file_name: name.into(), file_size_bytes: bytes, file_path: "roms/psx".into(), ..Default::default() }
    }

    #[test]
    fn a_disc_brings_its_tracks_and_the_set_sorts_in_disc_order() {
        let files = vec![
            file(1, "FF7 (USA) (Disc 2).cue", 1),
            file(2, "FF7 (USA) (Disc 2).bin", 100),
            file(3, "FF7 (USA) (Disc 1).cue", 1),
            file(4, "FF7 (USA) (Disc 1) (Track 1).bin", 90),
            file(5, "FF7 (USA) (Disc 1) (Track 2).bin", 10),
            file(6, "FF7 (USA) (Disc 10).chd", 50),
        ];
        let discs = discs(&files);
        let labels: Vec<&str> = discs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, ["Disc 1", "Disc 2", "Disc 10"]);
        assert_eq!(discs[0].files, [3, 4, 5], "the cue and both its tracks");
        assert_eq!(discs[0].bytes, 101);
        assert_eq!(discs[1].files, [1, 2]);
        assert_eq!(discs[2].files, [6], "disc 10's files are not disc 1's");
    }

    #[test]
    fn one_disc_or_a_cue_with_its_image_is_not_a_choice() {
        assert!(discs(&[file(1, "Game (USA).chd", 1)]).is_empty());
        // A CloneCD set: the cue points at the image, which is its track.
        assert!(discs(&[file(1, "Game (USA).cue", 1), file(2, "Game (USA).img", 9)]).is_empty());
        // A manual beside two discs does not ride with either.
        let mut manual = file(9, "FF7 (USA) (Disc 1) (Manual).pdf", 5);
        manual.category = Some("manual".into());
        let set = discs(&[file(1, "FF7 (USA) (Disc 1).chd", 1), file(2, "FF7 (USA) (Disc 2).chd", 1), manual]);
        assert!(set.iter().all(|d| !d.files.contains(&9)));
    }

    #[test]
    fn a_build_is_named_by_what_its_file_name_says_sets_it_apart() {
        assert_eq!(build_label("Zelda (Europe) (Rev 1)"), "Europe · Rev 1");
        assert_eq!(build_label("7th Saga, The (USA) (patched-7thSagaRedux)"), "USA · patched-7thSagaRedux");
        assert_eq!(build_label("Tetris"), "Original");
    }

    fn game() -> Game {
        let mut game = crate::fixtures::library(1).all_games()[0].clone();
        game.rom_id = 10;
        game.sibling_ids = vec![11];
        game.file_stems = vec![(10, "Zelda (USA)".into()), (11, "Zelda (Europe)".into())];
        game
    }

    #[test]
    fn the_game_s_own_build_comes_first_then_its_discs_then_the_other_builds() {
        let detail = RomDetail {
            id: 10,
            files: vec![file(1, "Zelda (USA) (Disc 1).chd", 10), file(2, "Zelda (USA) (Disc 2).chd", 20)],
            ..Default::default()
        };
        let all = choices(&game(), Some(&detail), &[]);
        let labels: Vec<&str> = all.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["All 2 discs", "Disc 1", "Disc 2", "Europe"]);
        assert_eq!((all[0].files.clone(), all[0].bytes), (None, Some(30)));
        assert_eq!(all[2].files, Some(vec![2]));
        assert_eq!(all[3].rom_id, 11);
    }

    #[test]
    fn the_whole_game_weighs_its_update_and_dlc_but_not_the_extras_left_on_romm() {
        let filed = |id: i64, category: &str, bytes: i64| RomFile { category: Some(category.into()), ..file(id, &format!("{id}.nsp"), bytes) };
        let detail = RomDetail {
            id: 10,
            files: vec![filed(1, "game", 100), filed(2, "update", 20), filed(3, "dlc", 5), filed(4, "manual", 1000)],
            ..Default::default()
        };
        let mut solo = game();
        solo.sibling_ids.clear();
        solo.file_stems.truncate(1);
        let all = choices(&solo, Some(&detail), &[]);
        assert_eq!(all.len(), 1, "an update is no disc, so there is nothing to ask");
        assert_eq!((all[0].files.clone(), all[0].bytes), (None, Some(125)));
    }

    #[test]
    fn romm_s_own_name_for_a_build_wins_and_a_build_it_lost_is_not_offered() {
        let mut named = game();
        named.build = Some("USA · Rev 1".into());
        let labels: Vec<String> = choices(&named, None, &[]).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["USA · Rev 1", "Europe"]);

        named.unavailable = true;
        let left: Vec<i64> = choices(&named, None, &[]).into_iter().map(|c| c.rom_id).collect();
        assert_eq!(left, [11], "only the build RomM still has a file for");
        assert!(!named.missing(), "there is still something to download");
        named.sibling_ids.clear();
        named.file_stems.truncate(1);
        assert!(named.missing());
    }
}
