//! The Browse page's view of a system: which games it shows, in what order, and the A-Z index down
//! its right edge.
//!
//! Kept out of the widget code because it is all decisions rather than layout: a filter that empties
//! the grid and a sort that moves the game under the cursor are both things the cursor has to be
//! reconciled against, and that has to be testable without a window.

use crate::library::Game;

/// The filter chips, in the order Browse walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    /// On disk — the set [`crate::downloads`] keeps. Shown with its count even when that is zero:
    /// "installed 0" is a fact about this system, where a hidden chip would read as a missing
    /// feature.
    Installed,
    Unplayed,
    Favourites,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::Installed, Filter::Unplayed, Filter::Favourites];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Installed => "installed",
            Filter::Unplayed => "unplayed",
            Filter::Favourites => "favourites",
        }
    }
}

/// The sort chips, in the order Browse walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Alpha,
    Rating,
    Added,
    Played,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Alpha, Sort::Rating, Sort::Added, Sort::Played];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Alpha => "A-Z",
            Sort::Rating => "rating",
            Sort::Added => "added",
            Sort::Played => "played",
        }
    }
}

/// One system's games, filtered and sorted, plus the letter index for the rail.
pub struct SystemView<'a> {
    pub games: Vec<&'a Game>,
    /// Letter → the first index in [`SystemView::games`] that starts with it. Built from what is
    /// actually on screen, so it never jumps to a game a filter has removed.
    pub letters: Vec<(char, usize)>,
}

impl<'a> SystemView<'a> {
    /// A view over a set of games — a system's, or a collection's, which spans systems.
    pub fn of_games(
        games: Vec<&'a Game>,
        filter: Filter,
        sort: Sort,
        favourites: &std::collections::HashSet<i64>,
        installed: &std::collections::HashSet<i64>,
    ) -> Self {
        let mut games: Vec<&Game> = games
            .into_iter()
            .filter(|g| match filter {
                Filter::All => true,
                Filter::Installed => g.builds().any(|id| installed.contains(&id)),
                Filter::Unplayed => g.last_played.as_deref().is_none_or(|s| s.trim().is_empty()),
                // Any build of it: RomM's web app can favourite a regional build the tile stands for.
                Filter::Favourites => g.builds().any(|id| favourites.contains(&id)),
            })
            .collect();

        match sort {
            Sort::Alpha => games.sort_by(|a, b| a.sort_title.cmp(&b.sort_title)),
            // Unrated games sort last rather than as zero, so a rating sort is a list of rated
            // games followed by the rest, not a list led by everything nobody has scored.
            Sort::Rating => games.sort_by(|a, b| {
                b.rating
                    .partial_cmp(&a.rating)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.sort_title.cmp(&b.sort_title))
            }),
            Sort::Added => games.sort_by(|a, b| {
                b.created_at.cmp(&a.created_at).then_with(|| a.sort_title.cmp(&b.sort_title))
            }),
            Sort::Played => games.sort_by(|a, b| {
                b.last_played.cmp(&a.last_played).then_with(|| a.sort_title.cmp(&b.sort_title))
            }),
        }

        // The rail only means anything against an alphabetical order — against "recently added" the
        // letters are scattered, so jumping to one would land somewhere arbitrary.
        let letters = if sort == Sort::Alpha { letter_index(&games) } else { Vec::new() };
        Self { games, letters }
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    /// Where the game with this id sits in the current view, if it is still in it.
    pub fn position_of(&self, rom_id: i64) -> Option<usize> {
        self.games.iter().position(|g| g.rom_id == rom_id)
    }
}

/// The first index for each starting letter, with everything non-alphabetic gathered under `#`.
fn letter_index(games: &[&Game]) -> Vec<(char, usize)> {
    let mut index: Vec<(char, usize)> = Vec::new();
    for (i, game) in games.iter().enumerate() {
        let first = game
            .sort_title
            .chars()
            .next()
            .map(|c| if c.is_ascii_alphabetic() { c.to_ascii_uppercase() } else { '#' })
            .unwrap_or('#');
        if index.last().map(|(c, _)| *c) != Some(first) {
            // A letter already seen cannot start a second run: the list is sorted, so a repeat
            // would mean the sort and this index disagree.
            if !index.iter().any(|(c, _)| *c == first) {
                index.push((first, i));
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Game, Library, System};
    use std::collections::HashSet;

    fn game(id: i64, title: &str, played: Option<&str>, rating: Option<f32>, created: Option<&str>) -> Game {
        Game {
            rom_id: id,
            title: title.into(),
            sort_title: crate::library::test_sort_title(title),
            system_id: "snes".into(),
            last_played: played.map(str::to_string),
            created_at: created.map(str::to_string),
            rating,
            ..Default::default()
        }
    }

    fn library() -> Library {
        Library::new(
            vec![System { id: "snes".into(), name: "SNES".into(), cover_ratio: 1.37 }],
            vec![
                game(1, "Chrono Trigger", Some("2026-08-01"), Some(4.8), Some("2026-01-01")),
                game(2, "The Lost City", None, Some(3.0), Some("2026-05-01")),
                game(3, "Aladdin", Some("2026-02-01"), None, Some("2026-03-01")),
                game(4, "007 Goldeneye", None, Some(4.0), Some("2026-06-01")),
            ],
        )
    }

    fn view(filter: Filter, sort: Sort) -> Vec<i64> {
        let lib = library();
        let favs = HashSet::from([2]);
        let installed = HashSet::new();
        SystemView::of_games(lib.games_for("snes").iter().collect(), filter, sort, &favs, &installed)
            .games
            .iter()
            .map(|g| g.rom_id)
            .collect()
    }

    #[test]
    fn alpha_sort_uses_the_article_stripped_title() {
        // 007 is non-alphabetic and leads; "The Lost City" sorts under L, after Chrono.
        assert_eq!(view(Filter::All, Sort::Alpha), vec![4, 3, 1, 2]);
    }

    #[test]
    fn unrated_games_sort_last_rather_than_as_zero() {
        let order = view(Filter::All, Sort::Rating);
        assert_eq!(order, vec![1, 4, 2, 3], "Aladdin, unrated, is last");
    }

    #[test]
    fn filters_narrow_to_what_they_name() {
        assert_eq!(view(Filter::Unplayed, Sort::Alpha), vec![4, 2]);
        assert_eq!(view(Filter::Favourites, Sort::Alpha), vec![2]);
        // Nothing on disk is a real answer rather than a missing one.
        assert!(view(Filter::Installed, Sort::Alpha).is_empty());
    }

    #[test]
    fn the_letter_rail_indexes_what_is_on_screen_not_the_whole_system() {
        let lib = library();
        let favs = HashSet::new();
        let installed = HashSet::new();
        let all = SystemView::of_games(lib.games_for("snes").iter().collect(), Filter::All, Sort::Alpha, &favs, &installed);
        assert_eq!(all.letters, vec![('#', 0), ('A', 1), ('C', 2), ('L', 3)]);

        // Filtered down, the rail follows: only the surviving games have letters.
        let unplayed = SystemView::of_games(lib.games_for("snes").iter().collect(), Filter::Unplayed, Sort::Alpha, &favs, &installed);
        assert_eq!(unplayed.letters, vec![('#', 0), ('L', 1)]);
    }

    #[test]
    fn the_rail_is_empty_under_a_non_alphabetical_sort() {
        let lib = library();
        let (favs, installed) = (HashSet::new(), HashSet::new());
        let by_added = SystemView::of_games(lib.games_for("snes").iter().collect(), Filter::All, Sort::Added, &favs, &installed);
        assert!(
            by_added.letters.is_empty(),
            "against 'recently added' the letters are scattered, so a jump would land arbitrarily"
        );
    }

    #[test]
    fn a_game_can_be_found_again_after_a_sort_change() {
        let lib = library();
        let (favs, installed) = (HashSet::new(), HashSet::new());
        let alpha = SystemView::of_games(lib.games_for("snes").iter().collect(), Filter::All, Sort::Alpha, &favs, &installed);
        let added = SystemView::of_games(lib.games_for("snes").iter().collect(), Filter::All, Sort::Added, &favs, &installed);
        let chrono = alpha.games[2].rom_id;
        assert_eq!(alpha.position_of(chrono), Some(2));
        assert_eq!(added.position_of(chrono), Some(3), "same game, new place");
    }
}
