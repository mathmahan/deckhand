//! Search across every system — a port of the Kotlin app's `Fuzzy` and `SearchState`.
//!
//! Scoring, not substring. A query is matched against a title's derived keys as well as the title
//! itself, which is what lets `ff7`, `mgs3` and `sotc` find what they mean:
//!
//! - the normalised title — lowercased, punctuation collapsed to spaces, and a second form with
//!   Roman numerals as digits ("final fantasy vii" also reads "final fantasy 7"). Both forms, never
//!   a destructive conversion: that would lose "Mega Man X" for `mmx` to win `ff7`;
//! - its initials — "metal gear solid 3" is `mgs3`.
//!
//! Then, best first: the initials exactly, the initials as a prefix, the title as a prefix, the
//! title containing it, and last an in-order subsequence that rewards word starts and runs — which
//! is capped below every direct hit, so a scattered match never outranks a real one.
//!
//! The page is an on-screen keyboard over a list of results, because a pad has no keys to type
//! with. A real keyboard types straight into the query as well.

use crate::app::Direction;
use crate::library::Library;
#[cfg(test)]
use crate::library::Game;

/// The on-screen keyboard: QWERTY, since familiar beats alphabetical. `' '` draws as a space key
/// and `'\u{8}'` as backspace.
pub const ROWS: [&str; 4] = ["qwertyuiop", "asdfghjkl", "zxcvbnm", "0123456789 \u{8}"];

/// Backspace, as the keyboard carries it.
pub const BACKSPACE: char = '\u{8}';

/// How many results a query keeps. Generous, so "final fantasy" shows its whole tail, but a floor
/// all the same: ranked with none, real matches sink under fuzzy near-misses.
const LIMIT: usize = 200;

/// How well `query` matches `title`: 0 is no match, higher is better, comparable across titles.
#[cfg(test)]
pub fn score(query: &str, title: &str) -> u32 {
    let q = query_key(query);
    if q.is_empty() {
        return 0;
    }
    forms(title).iter().map(|form| Form::of(form).score(&q)).max().unwrap_or(0)
}

/// Every game in `library` matching `query`, best first — [`Index`] and [`Index::rank`] in one.
#[cfg(test)]
pub fn search<'a>(library: &'a Library, query: &str) -> Vec<&'a Game> {
    Index::of(library).rank(query).into_iter().filter_map(|id| library.game(id)).collect()
}

/// A query as it is matched: lowercase letters and digits only.
fn query_key(query: &str) -> String {
    query.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// One normalised form of a name, with what scoring it needs worked out up front — they depend on
/// the name alone, and a search is every name against one query.
struct Form {
    normalized: String,
    initials: String,
    /// The form without its spaces.
    flat: String,
}

impl Form {
    fn of(normalized: &str) -> Self {
        Self {
            initials: initials(normalized),
            flat: normalized.chars().filter(|c| *c != ' ').collect(),
            normalized: normalized.to_string(),
        }
    }

    fn score(&self, q: &str) -> u32 {
        let mut best = 0;
        if self.initials == q {
            best = 1000;
        } else if self.initials.starts_with(q) {
            best = 900 - (self.initials.len() - q.len()).min(100) as u32;
        }
        if self.flat.starts_with(q) {
            best = best.max(800);
        } else if let Some(at) = self.flat.find(q) {
            best = best.max(700 - at.min(90) as u32);
        }
        best.max(subsequence(q, &self.normalized))
    }
}

/// Every game's names, prepared for matching — built once per library rather than on every
/// search, which once re-derived each of some 3,000 titles' forms on every frame of the page.
pub struct Index {
    /// The library it was built from — see [`Library::generation`].
    pub generation: u64,
    /// Each game's rom id, and the forms of its title and its other names, in library order.
    games: Vec<(i64, Vec<Form>)>,
}

impl Index {
    pub fn of(library: &Library) -> Self {
        // A title's other names count as much as the title itself: `ct` finds Chrono Trigger through
        // RomM's "CT", and a Japanese release's name finds the game filed under its English one.
        let games = library
            .all_games().iter()
            .map(|g| {
                let forms = std::iter::once(&g.title).chain(&g.alt_names).flat_map(|name| forms(name)).map(|f| Form::of(&f)).collect();
                (g.rom_id, forms)
            })
            .collect();
        Self { generation: library.generation(), games }
    }

    /// The rom ids matching `query`, best first, capped at [`LIMIT`]. Ties keep library order,
    /// which is system then title.
    pub fn rank(&self, query: &str) -> Vec<i64> {
        let q = query_key(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u32, i64)> = self
            .games
            .iter()
            .map(|(id, forms)| (forms.iter().map(|f| f.score(&q)).max().unwrap_or(0), *id))
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.into_iter().take(LIMIT).map(|(_, id)| id).collect()
    }
}

/// The title lowercased with punctuation as single spaces, and again with Roman numerals as digits
/// when that changes anything.
fn forms(title: &str) -> Vec<String> {
    let plain: String = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let roman = plain.split(' ').map(|w| roman(w).unwrap_or(w)).collect::<Vec<_>>().join(" ");
    if roman == plain { vec![plain] } else { vec![plain, roman] }
}

fn roman(word: &str) -> Option<&'static str> {
    const NUMERALS: [(&str, &str); 16] = [
        ("i", "1"), ("ii", "2"), ("iii", "3"), ("iv", "4"), ("v", "5"), ("vi", "6"), ("vii", "7"), ("viii", "8"),
        ("ix", "9"), ("x", "10"), ("xi", "11"), ("xii", "12"), ("xiii", "13"), ("xiv", "14"), ("xv", "15"), ("xvi", "16"),
    ];
    NUMERALS.iter().find(|(r, _)| *r == word).map(|(_, d)| *d)
}

fn initials(normalized: &str) -> String {
    normalized.split(' ').filter_map(|w| w.chars().next()).collect()
}

/// Every query character in order through the title; word starts and consecutive runs count.
fn subsequence(q: &str, normalized: &str) -> u32 {
    let chars: Vec<char> = normalized.chars().collect();
    let mut wanted = q.chars().peekable();
    let mut score = 0;
    let mut previous: Option<usize> = None;
    for (i, c) in chars.iter().enumerate() {
        let Some(&next) = wanted.peek() else { break };
        if *c == next {
            let word_start = i == 0 || chars[i - 1] == ' ';
            score += 4 + if word_start { 10 } else { 0 } + if previous == Some(i.wrapping_sub(1)) { 6 } else { 0 };
            previous = Some(i);
            wanted.next();
        }
    }
    if wanted.peek().is_some() { 0 } else { score.min(600) }
}

/// The Search page's state: the query, and a cursor that is either on the keyboard or in the
/// results. The results are not kept — they are recomputed from the query and the library, so a
/// library refresh under an open search can never leave it pointing at a game that is gone.
#[derive(Debug, Default)]
pub struct SearchFocus {
    pub query: String,
    /// False while the keyboard has the cursor.
    pub in_results: bool,
    pub key_row: usize,
    pub key_col: usize,
    result: usize,
}

impl SearchFocus {
    /// Type `c` — a character, or [`BACKSPACE`]. Any edit sends the cursor back to the top.
    pub fn type_char(&mut self, c: char) {
        if c == BACKSPACE {
            self.query.pop();
        } else {
            self.query.push(c);
        }
        self.result = 0;
    }

    /// The key under the cursor.
    pub fn key(&self) -> char {
        ROWS[self.key_row].chars().nth(self.key_col).unwrap_or(' ')
    }

    /// The result the cursor is on, among `results` many — clamped, since the list shrinks as the
    /// query grows.
    pub fn result(&self, results: usize) -> usize {
        self.result.min(results.saturating_sub(1))
    }

    /// Move. The results stand beside the keyboard and under its last row both: Right off a row's
    /// end and Down off the last row go into them, Left and Up off the first result come back.
    /// Answers whether anything moved.
    pub fn step(&mut self, direction: Direction, results: usize) -> bool {
        let before = (self.in_results, self.key_row, self.key_col, self.result(results));
        let width = |row: usize| ROWS[row].chars().count();
        match (self.in_results, direction) {
            (false, Direction::Left) => self.key_col = self.key_col.saturating_sub(1),
            (false, Direction::Right) if self.key_col + 1 >= width(self.key_row) && results > 0 => {
                self.in_results = true;
                self.result = 0;
            }
            (false, Direction::Right) => self.key_col = (self.key_col + 1).min(width(self.key_row) - 1),
            (true, Direction::Left) => self.in_results = false,
            (false, Direction::Up) => {
                self.key_row = self.key_row.saturating_sub(1);
                self.key_col = self.key_col.min(width(self.key_row) - 1);
            }
            (false, Direction::Down) if self.key_row + 1 < ROWS.len() => {
                self.key_row += 1;
                self.key_col = self.key_col.min(width(self.key_row) - 1);
            }
            (false, Direction::Down) if results > 0 => {
                self.in_results = true;
                self.result = 0;
            }
            (true, Direction::Down) => self.result = (self.result(results) + 1).min(results.saturating_sub(1)),
            (true, Direction::Up) if self.result(results) > 0 => self.result = self.result(results) - 1,
            (true, Direction::Up) => self.in_results = false,
            _ => {}
        }
        before != (self.in_results, self.key_row, self.key_col, self.result(results))
    }

    /// Put the cursor on a result — where a click landed.
    pub fn focus_result(&mut self, index: usize) {
        self.in_results = true;
        self.result = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_is_found_by_its_other_names_too() {
        let base = crate::fixtures::library(3);
        let mut games: Vec<Game> = base.all_games().to_vec();
        games[0].title = "Chrono Trigger".into();
        games[1].title = "Seiken Densetsu 3".into();
        games[1].alt_names = vec!["Trials of Mana".into()];
        let library = Library::new(base.systems, games);
        assert_eq!(search(&library, "trials").first().map(|g| g.title.as_str()), Some("Seiken Densetsu 3"));
        assert_eq!(search(&library, "tom").first().map(|g| g.title.as_str()), Some("Seiken Densetsu 3"), "its initials too");
    }

    #[test]
    fn acronyms_and_numerals_find_what_they_mean() {
        assert_eq!(score("ff7", "Final Fantasy VII"), 1000);
        assert_eq!(score("mgs3", "Metal Gear Solid 3: Snake Eater"), 900 - 2);
        assert_eq!(score("sotc", "Shadow of the Colossus"), 1000);
        assert_eq!(score("mmx", "Mega Man X"), 1000, "the plain form survives the numeral one");
        assert!(score("chrono", "Chrono Trigger (USA)") >= 800);
        assert_eq!(score("zzz", "Chrono Trigger"), 0);
        assert_eq!(score("  ", "anything"), 0);
    }

    #[test]
    fn a_direct_hit_always_outranks_a_scattered_one() {
        let direct = score("trig", "Chrono Trigger");
        let scattered = score("trig", "Teenage Robot Ninja Gang");
        assert!(direct > scattered && scattered > 0, "{direct} vs {scattered}");
    }

    #[test]
    fn the_cursor_walks_the_keyboard_and_drops_into_the_results() {
        let mut f = SearchFocus::default();
        assert!(!f.step(Direction::Up, 5), "nothing above the top row");
        f.step(Direction::Right, 5);
        assert_eq!(f.key(), 'w');
        for _ in 0..3 {
            f.step(Direction::Down, 5);
        }
        assert_eq!(f.key(), '1');
        assert!(!f.step(Direction::Down, 0) || f.in_results, "no results, nowhere to go");
        assert!(f.step(Direction::Down, 5) && f.in_results);
        f.step(Direction::Down, 5);
        assert_eq!(f.result(5), 1);
        assert_eq!(f.result(1), 0, "clamped into a list that shrank");
        f.step(Direction::Up, 5);
        f.step(Direction::Up, 5);
        assert!(!f.in_results, "up off the first result is the keyboard again");

        f.type_char('f');
        f.type_char('f');
        f.type_char(BACKSPACE);
        assert_eq!(f.query, "f");
    }

    #[test]
    fn the_results_beside_the_keyboard_are_right_of_every_row_and_left_goes_back() {
        let mut f = SearchFocus::default();
        for _ in 0..9 {
            f.step(Direction::Right, 5);
        }
        assert_eq!(f.key(), 'p');
        assert!(!f.step(Direction::Right, 0), "no results, the row's end is the end");
        assert!(f.step(Direction::Right, 5) && f.in_results, "right off the row's end");
        f.step(Direction::Down, 5);
        assert!(f.step(Direction::Left, 5) && !f.in_results, "left from any result is the keyboard");
        assert_eq!(f.key(), 'p', "where it left");
    }
}
