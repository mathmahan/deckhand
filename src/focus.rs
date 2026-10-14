//! Home's cursor: which band it is in, which shelf, and where on that shelf.
//!
//! A port of `HomeFocus`/`HomeState` from `HomeView.kt`. Deckhand is gamepad-first, which means
//! selection is a model the app owns rather than the toolkit's focus ring — so navigation stays
//! exact and predictable, and a background library refresh cannot move the cursor.

use std::collections::HashMap;

use crate::library::Shelf;

/// Home's vertical bands, top to bottom. Up and Down walk between them; everything else is
/// interpreted by the band that has it.
///
/// There is no hero of its own any more. The spotlight across the top of the page is whichever game
/// the shelf cursor rests on, so its buttons are a band of their own above the shelves: Up from the
/// first shelf reaches them, Down goes back to the card they are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    /// The spotlight's buttons, for the game the shelf cursor rests on.
    Actions,
    #[default]
    Shelves,
}

/// The Home cursor, kept outside the per-frame shelf list so it survives a library refresh — a
/// refresh rebuilds the shelves, and without a persistent holder the cursor snapped back to the
/// first card every time.
#[derive(Debug, Default)]
pub struct HomeFocus {
    zone: Zone,
    shelf_pos: usize,
    /// Where the cursor sits **on each shelf**, keyed by shelf id — not one index shared by all.
    ///
    /// A single shared index made the shelves move together: walking to the third card of Continue
    /// playing and pressing Down landed on the third card of Recently added, and coming back up had
    /// lost wherever you actually were. The shelves answer completely different questions, so a
    /// position on one means nothing on the next.
    ///
    /// Keyed by id rather than by row, because rows are not stable: a shelf empties out and stops
    /// being drawn, and every collection shelf below it shifts up. Entries for shelves that are not
    /// currently drawn are kept, not evicted — that is what restores your place when the shelf comes
    /// back, and the key space is bounded by the collection count.
    index_by_shelf: HashMap<String, usize>,
    /// Which of the spotlight's buttons the cursor is on, in [`Zone::Actions`].
    action: usize,
}

impl HomeFocus {
    /// Clamp the cursor into a freshly rebuilt shelf list. Call once per rebuild, before reading
    /// anything else.
    ///
    /// A preserved position may point past a shelf or a card that dropped out on refresh.
    pub fn reconcile(&mut self, shelves: &[Shelf]) {
        self.shelf_pos = self.shelf_pos.min(shelves.len().saturating_sub(1));

        // Every shelf's remembered position, not just the focused one: any of them can be shorter
        // than it was, and an off-screen shelf's cursor still has to be in bounds for the moment
        // you walk back down to it.
        for shelf in shelves {
            if let Some(remembered) = self.index_by_shelf.get_mut(&shelf.id) {
                *remembered = (*remembered).min(shelf.games.len().saturating_sub(1));
            }
        }
        // With no shelf there is no game for the buttons to be about.
        if shelves.is_empty() {
            self.zone = Zone::Shelves;
        }
    }

    /// Put the cursor back where the page opens: the first card of the first shelf — the game
    /// played last, under the cursor with the spotlight on it.
    pub fn reset_to_top(&mut self, shelves: &[Shelf]) {
        self.zone = Zone::Shelves;
        self.shelf_pos = 0;
        if let Some(first) = shelves.first() {
            self.index_by_shelf.insert(first.id.clone(), 0);
        }
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    pub fn shelf_pos(&self) -> usize {
        self.shelf_pos
    }

    /// Which of the spotlight's buttons the cursor is on — meaningful in [`Zone::Actions`].
    pub fn action(&self) -> usize {
        self.action
    }

    /// Where the cursor sits on `shelf` — its own remembered position, whether or not it is the
    /// shelf with focus. Each shelf's row reads this to stay scrolled to its own card, so walking
    /// back down to a shelf finds it showing where you left it.
    pub fn index_of(&self, shelf: &Shelf) -> usize {
        self.index_by_shelf
            .get(&shelf.id)
            .copied()
            .unwrap_or(0)
            .min(shelf.games.len().saturating_sub(1))
    }

    /// The card the spotlight is on: the shelf cursor's, in either band. On the buttons the cursor
    /// is still *about* that card, which stays lifted under them.
    pub fn is_spotlit(&self, shelves: &[Shelf], shelf_index: usize, card: usize) -> bool {
        self.shelf_pos == shelf_index && shelves.get(shelf_index).is_some_and(|s| self.index_of(s) == card)
    }

    /// The card under the cursor itself — the spotlit one, while the cursor is on the shelves.
    pub fn is_focused(&self, shelves: &[Shelf], shelf_index: usize, card: usize) -> bool {
        self.zone == Zone::Shelves && self.is_spotlit(shelves, shelf_index, card)
    }

    /// The game in the spotlight.
    pub fn spotlit<'a>(&self, shelves: &'a [Shelf]) -> Option<&'a crate::library::Game> {
        let shelf = shelves.get(self.shelf_pos)?;
        shelf.games.get(self.index_of(shelf))
    }

    /// Put the cursor on a card — where a click landed.
    pub fn focus_card(&mut self, shelves: &[Shelf], shelf_index: usize, card: usize) {
        let Some(shelf) = shelves.get(shelf_index) else { return };
        if card >= shelf.games.len() {
            return;
        }
        self.zone = Zone::Shelves;
        self.shelf_pos = shelf_index;
        self.index_by_shelf.insert(shelf.id.clone(), card);
    }

    /// Horizontal step: along the focused shelf, or across the spotlight's `actions` buttons. False
    /// = fell off the edge, and the caller keeps the cursor put.
    pub fn step(&mut self, shelves: &[Shelf], delta: isize, actions: usize) -> bool {
        if self.zone == Zone::Actions {
            let next = self.action as isize + delta;
            if next < 0 || next >= actions as isize {
                return false;
            }
            self.action = next as usize;
            return true;
        }
        let Some(shelf) = shelves.get(self.shelf_pos) else { return false };
        let next = self.index_of(shelf) as isize + delta;
        if next < 0 || next >= shelf.games.len() as isize {
            return false;
        }
        self.index_by_shelf.insert(shelf.id.clone(), next as usize);
        true
    }

    /// Vertical move: between bands at the edges, between shelves inside the shelf band.
    ///
    /// Deliberately does **not** carry the column across when changing shelf. Doing so made the
    /// shelves read as one grid: the third card of Continue playing sat above the third card of
    /// Recently added, and stepping back up landed on whatever the shortest shelf had clamped you
    /// to rather than where you were. Each shelf keeps its own place.
    ///
    /// Up into the buttons always lands on the first, the main action: that is the one the
    /// spotlight is built around, and the one most likely wanted on the way up.
    pub fn step_vertical(&mut self, shelves: &[Shelf], delta: isize) -> bool {
        match self.zone {
            Zone::Actions => {
                if delta > 0 && !shelves.is_empty() {
                    self.zone = Zone::Shelves;
                    return true;
                }
                false
            }
            Zone::Shelves => {
                let next = self.shelf_pos as isize + delta;
                if next < 0 {
                    // Off the top of the shelves: up to the spotlight's buttons.
                    if shelves.is_empty() {
                        return false;
                    }
                    self.zone = Zone::Actions;
                    self.action = 0;
                    return true;
                }
                if next >= shelves.len() as isize {
                    return false;
                }
                self.shelf_pos = next as usize;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Game;

    fn game(id: i64) -> Game {
        Game {
            rom_id: id,
            title: format!("Game {id}"),
            sort_title: format!("game {id}"),
            system_id: "snes".into(),
            ..Default::default()
        }
    }

    fn shelf(id: &str, n: i64) -> Shelf {
        Shelf { id: id.into(), title: id.into(), note: String::new(), games: (0..n).map(game).collect() }
    }

    #[test]
    fn opens_on_the_first_card_with_the_spotlight_on_it() {
        let shelves = vec![shelf("continue", 3)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);
        assert_eq!(focus.zone(), Zone::Shelves);
        assert_eq!(focus.spotlit(&shelves).map(|g| g.rom_id), Some(0), "the game played last");
        assert!(focus.is_focused(&shelves, 0, 0));
    }

    #[test]
    fn each_shelf_keeps_its_own_place() {
        let shelves = vec![shelf("continue", 5), shelf("recent", 5)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);

        assert!(focus.step(&shelves, 1, 4));
        assert!(focus.step(&shelves, 1, 4));
        assert_eq!(focus.index_of(&shelves[0]), 2);

        // Down to the next shelf: it starts at its own place, not the outgoing shelf's column.
        assert!(focus.step_vertical(&shelves, 1));
        assert_eq!(focus.index_of(&shelves[1]), 0, "the column is not carried across");

        // And coming back up finds the first shelf exactly where it was left.
        assert!(focus.step_vertical(&shelves, -1));
        assert_eq!(focus.index_of(&shelves[0]), 2);
    }

    #[test]
    fn steps_stop_at_the_edges_rather_than_wrapping() {
        let shelves = vec![shelf("only", 2)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);

        assert!(!focus.step(&shelves, -1, 4), "left of the first card goes nowhere");
        assert!(focus.step(&shelves, 1, 4));
        assert!(!focus.step(&shelves, 1, 4), "right of the last card goes nowhere");
        assert!(!focus.step_vertical(&shelves, 1), "below the last shelf goes nowhere");
    }

    #[test]
    fn up_from_the_first_shelf_reaches_the_spotlights_buttons_and_down_comes_back() {
        let shelves = vec![shelf("continue", 3), shelf("recent", 3)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);
        focus.step(&shelves, 1, 4);

        assert!(focus.step_vertical(&shelves, -1));
        assert_eq!(focus.zone(), Zone::Actions);
        assert_eq!(focus.action(), 0, "on the main action");
        assert_eq!(focus.spotlit(&shelves).map(|g| g.rom_id), Some(1), "still about the card it came from");
        assert!(focus.is_spotlit(&shelves, 0, 1) && !focus.is_focused(&shelves, 0, 1), "but not on it");

        // Across the buttons, stopping at both ends.
        assert!(!focus.step(&shelves, -1, 4));
        for _ in 0..3 {
            assert!(focus.step(&shelves, 1, 4));
        }
        assert!(!focus.step(&shelves, 1, 4), "past the last button goes nowhere");
        assert!(!focus.step_vertical(&shelves, -1), "above the buttons is the tab bar's business");

        assert!(focus.step_vertical(&shelves, 1));
        assert!(focus.is_focused(&shelves, 0, 1), "back on the card the buttons were for");
    }

    #[test]
    fn up_from_a_lower_shelf_is_the_shelf_above_not_the_buttons() {
        let shelves = vec![shelf("continue", 3), shelf("recent", 3)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);
        focus.step_vertical(&shelves, 1);
        assert!(focus.step_vertical(&shelves, -1));
        assert_eq!((focus.zone(), focus.shelf_pos()), (Zone::Shelves, 0));
    }

    #[test]
    fn a_click_puts_the_cursor_on_the_card_it_landed_on() {
        let shelves = vec![shelf("continue", 3), shelf("recent", 4)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);
        focus.step_vertical(&shelves, -1);
        assert_eq!(focus.zone(), Zone::Actions);
        focus.focus_card(&shelves, 1, 3);
        assert!(focus.is_focused(&shelves, 1, 3));
        focus.focus_card(&shelves, 1, 9);
        assert!(focus.is_focused(&shelves, 1, 3), "a card that is not there moves nothing");
    }

    #[test]
    fn a_refresh_that_shrinks_a_shelf_clamps_the_cursor_instead_of_pointing_past_it() {
        let big = vec![shelf("continue", 6), shelf("recent", 6)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&big);
        focus.step_vertical(&big, 1);
        for _ in 0..5 {
            focus.step(&big, 1, 4);
        }
        assert_eq!(focus.shelf_pos(), 1);
        assert_eq!(focus.index_of(&big[1]), 5);

        // The server drops a shelf and shortens the other.
        let smaller = vec![shelf("continue", 2)];
        focus.reconcile(&smaller);
        assert_eq!(focus.shelf_pos(), 0, "the cursor is pulled back onto a shelf that exists");
        assert!(focus.spotlit(&smaller).is_some(), "and lands on a real card");
    }

    #[test]
    fn a_shelf_that_comes_back_restores_where_you_were() {
        let both = vec![shelf("continue", 4), shelf("recent", 4)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&both);
        focus.step(&both, 1, 4);
        focus.step(&both, 1, 4);
        assert_eq!(focus.index_of(&both[0]), 2);

        // Continue playing empties out (offline), then comes back on the next refresh.
        let without = vec![shelf("recent", 4)];
        focus.reconcile(&without);
        focus.reconcile(&both);
        assert_eq!(focus.index_of(&both[0]), 2, "the remembered position was kept, not evicted");
    }

    #[test]
    fn the_top_of_home_is_the_game_played_last() {
        let shelves = vec![shelf("continue", 4), shelf("recent", 4)];
        let mut focus = HomeFocus::default();
        focus.reconcile(&shelves);
        focus.step(&shelves, 1, 4);
        focus.step_vertical(&shelves, 1);
        focus.reset_to_top(&shelves);
        assert!(focus.is_focused(&shelves, 0, 0));
    }
}

// -------------------------------------------------------------------------------------------
// Card grids
// -------------------------------------------------------------------------------------------

/// How many cards sit across a card page — the Collections page's.
///
/// Fixed, not derived from a cover ratio — these are uniform chrome cards, not box art, so the
/// per-system geometry in [`crate::grid`] does not apply to them.
pub const CARD_COLS: usize = 5;

/// A card page's cursor: a plain 2D walk over a uniform card grid.
///
/// Separate from [`BrowseFocus`] on purpose, and it is a duplication worth naming. The game grid's
/// movement is entangled with its neighbours — Left at column 0 and Up at row 0 are handoffs to
/// another zone, not clamps. A card grid's edges simply clamp, and the page around it decides what
/// lies past them. Sharing one type would have meant either teaching it a second edge policy or
/// letting a card page inherit handoffs to zones it does not have.
#[derive(Debug, Default, Clone)]
pub struct CardFocus {
    index: usize,
    /// Kept for the same reason the game grid keeps one: dropping into a short last row and coming
    /// back up should return to the column you left from, not the one you landed in.
    desired_column: usize,
}

impl CardFocus {
    pub fn index(&self) -> usize {
        self.index
    }

    /// Keep the cursor on one of `count` cards, the last if it was past them.
    pub fn clamp(&mut self, count: usize) {
        if self.index >= count {
            self.index = count.saturating_sub(1);
            self.desired_column = self.index % CARD_COLS;
        }
    }

    /// Move the cursor. Edges clamp — this page has nowhere to hand off to.
    pub fn step(&mut self, direction: crate::app::Direction, count: usize) -> bool {
        use crate::app::Direction::*;
        if count == 0 {
            return false;
        }
        let last = count - 1;
        let cols = CARD_COLS;
        let row = self.index / cols;
        let last_row = last / cols;
        let before = self.index;
        match direction {
            Right => {
                if self.index < last {
                    self.index += 1;
                    self.desired_column = self.index % cols;
                }
            }
            Left => {
                if !self.index.is_multiple_of(cols) {
                    self.index -= 1;
                    self.desired_column = self.index % cols;
                }
            }
            // desired_column is deliberately not updated by vertical movement.
            Down => {
                if row < last_row {
                    self.index = ((row + 1) * cols + self.desired_column).min(last);
                }
            }
            Up => {
                if row > 0 {
                    self.index = (row - 1) * cols + self.desired_column;
                }
            }
        }
        self.index != before
    }
}

// -------------------------------------------------------------------------------------------
// Browse
// -------------------------------------------------------------------------------------------

/// Browse's zones: the system list down the left, and beside it the chips above the grid, with the
/// A-Z rail down the grid's right edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseZone {
    /// The system list, reached by pressing Left at the grid's first column or the first chip.
    SystemList,
    /// The filter and sort chips.
    Chips,
    Grid,
    /// The A-Z rail down the grid's right edge, reached by pressing Right at the grid's last column.
    Rail,
}

/// The Browse page's cursor.
///
/// Unlike a card grid, this one has neighbours: Up at row 0 hands off to the chips, Left at a row's
/// first tile to the system list, and Right at its last to the alphabet rail. That is the whole
/// reason the two are separate types.
///
/// The grid's rows are packed from tiles of their own widths ([`crate::grid::Packing`]), so they
/// hold different numbers of tiles and a column number means nothing from one to the next. Up and
/// Down steer by horizontal position instead — to the tile in the next row whose centre is nearest
/// where the cursor was — and keep that position across a short row the way a column was kept.
#[derive(Debug)]
pub struct BrowseFocus {
    zone: BrowseZone,
    /// Index into the *filtered and sorted* view of the current system, not the raw library.
    index: usize,
    /// Where Up and Down aim, from the row's left edge: the centre of the tile last moved to
    /// sideways. Not updated by vertical moves, so a pass through a short row comes back to the same
    /// place.
    desired_x: f32,
    /// Which chip the cursor is on while in [`BrowseZone::Chips`].
    pub chip: usize,
    /// Which letter the cursor is on while in [`BrowseZone::Rail`].
    pub letter: usize,
    /// Which system the cursor is on while in [`BrowseZone::SystemList`], by library index. Rests on
    /// the open system the rest of the time, which is where stepping into the list lands.
    pub system: usize,
}

impl Default for BrowseFocus {
    fn default() -> Self {
        Self { zone: BrowseZone::Grid, index: 0, desired_x: 0.0, chip: 0, letter: 0, system: 0 }
    }
}

impl BrowseFocus {
    pub fn zone(&self) -> BrowseZone {
        self.zone
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn set_zone(&mut self, zone: BrowseZone) {
        self.zone = zone;
    }

    /// Land on a game, wherever it sits in the current view — used when Browse is entered from a
    /// system card, and when a filter change moves the game you were on.
    pub fn focus_index(&mut self, index: usize, packing: &crate::grid::Packing) {
        self.index = index;
        self.desired_x = packing.center(index);
        self.zone = BrowseZone::Grid;
    }

    /// Clamp into a view that may have shrunk under the cursor — a filter chip does exactly that.
    pub fn reconcile(&mut self, count: usize, chip_count: usize, letter_count: usize) {
        self.index = self.index.min(count.saturating_sub(1));
        self.chip = self.chip.min(chip_count.saturating_sub(1));
        self.letter = self.letter.min(letter_count.saturating_sub(1));
        // A rail with no letters, or an empty grid, is not somewhere the cursor can sit.
        if self.zone == BrowseZone::Rail && letter_count == 0 {
            self.zone = BrowseZone::Grid;
        }
        if self.zone == BrowseZone::Grid && count == 0 {
            self.zone = BrowseZone::Chips;
        }
    }

    /// Move the cursor over the grid as `packing` lays it out.
    pub fn step(
        &mut self,
        direction: crate::app::Direction,
        packing: &crate::grid::Packing,
        chip_count: usize,
        letter_count: usize,
        system_count: usize,
    ) -> bool {
        use crate::app::Direction::*;
        let count = packing.len();
        let before = (self.zone, self.index, self.chip, self.letter, self.system);
        match self.zone {
            BrowseZone::SystemList => match direction {
                Up => self.system = self.system.saturating_sub(1),
                Down => {
                    if self.system + 1 < system_count {
                        self.system += 1;
                    }
                }
                // Right leaves the list, which the caller handles: it may mean opening a system.
                Left | Right => {}
            },
            BrowseZone::Chips => match direction {
                // The chips are the top of the page; the system list is beside them, not above.
                Up => {}
                Down => {
                    if count > 0 {
                        self.zone = BrowseZone::Grid;
                    }
                }
                Left => {
                    if self.chip == 0 {
                        self.zone = BrowseZone::SystemList;
                    } else {
                        self.chip -= 1;
                    }
                }
                Right => {
                    if self.chip + 1 < chip_count {
                        self.chip += 1;
                    }
                }
            },
            BrowseZone::Grid => {
                if count == 0 {
                    return false;
                }
                self.index = self.index.min(count - 1);
                let row = packing.row_of(self.index);
                let tiles = packing.rows()[row].clone();
                match direction {
                    Right => {
                        if self.index + 1 == tiles.end {
                            // Off the row's right end: the alphabet rail, when there is one.
                            if letter_count > 0 {
                                self.zone = BrowseZone::Rail;
                            }
                        } else {
                            self.index += 1;
                            self.desired_x = packing.center(self.index);
                        }
                    }
                    Left => {
                        if self.index > tiles.start {
                            self.index -= 1;
                            self.desired_x = packing.center(self.index);
                        } else {
                            // Off the row's left end: the system list.
                            self.zone = BrowseZone::SystemList;
                        }
                    }
                    Down => {
                        if row + 1 < packing.rows().len() {
                            self.index = packing.nearest(row + 1, self.desired_x);
                        }
                    }
                    Up => {
                        if row > 0 {
                            self.index = packing.nearest(row - 1, self.desired_x);
                        } else {
                            // Off the top of the grid: back up to the chips.
                            self.zone = BrowseZone::Chips;
                        }
                    }
                }
            }
            BrowseZone::Rail => match direction {
                Left => self.zone = BrowseZone::Grid,
                Up => self.letter = self.letter.saturating_sub(1),
                Down => {
                    if self.letter + 1 < letter_count {
                        self.letter += 1;
                    }
                }
                Right => {}
            },
        }
        (self.zone, self.index, self.chip, self.letter, self.system) != before
    }
}

// -------------------------------------------------------------------------------------------
// Downloads
// -------------------------------------------------------------------------------------------

/// The Downloads page's cursor: one index down one flat list.
///
/// The page has two sections — what is coming, and what is on disk — but they are one list to walk,
/// because they read as one page and plain Up/Down is the whole of its navigation. The row count
/// changes under it constantly (a download finishes and moves from the top section to the bottom,
/// a removal takes a row away), so the cursor is clamped on every read rather than trusted.
#[derive(Debug, Default)]
pub struct DownloadsFocus {
    index: usize,
}

impl DownloadsFocus {
    /// Where the cursor is, in a list of `rows`. Clamped here rather than on the way in: the list
    /// is rebuilt from live state every frame, and a row can go while the cursor is below it.
    pub fn index(&self, rows: usize) -> usize {
        self.index.min(rows.saturating_sub(1))
    }

    /// Up/Down. The ends clamp — there is nothing above the first row or below the last to hand to.
    pub fn step(&mut self, delta: isize, rows: usize) -> bool {
        if rows == 0 {
            return false;
        }
        let before = self.index(rows);
        self.index = before.saturating_add_signed(delta).min(rows - 1);
        self.index != before
    }

    /// Put the cursor on a row — where a click landed.
    pub fn focus(&mut self, index: usize) {
        self.index = index;
    }
}

#[cfg(test)]
mod page_cursor_tests {
    use super::*;
    use crate::app::Direction::*;

    #[test]
    fn the_downloads_cursor_clamps_into_a_list_that_shrank_under_it() {
        let mut f = DownloadsFocus::default();
        assert!(!f.step(-1, 5), "nothing above the first row");
        for _ in 0..4 {
            f.step(1, 5);
        }
        assert_eq!(f.index(5), 4);
        assert!(!f.step(1, 5), "nothing below the last row");
        // Two downloads finish and are removed while the cursor sits at the bottom.
        assert_eq!(f.index(2), 1, "clamped on read, not left pointing past the end");
        assert!(!f.step(1, 2));
        assert_eq!(f.index(0), 0, "and an empty page has nowhere to be");
    }

    #[test]
    fn card_grid_edges_clamp_rather_than_wrapping_or_handing_off() {
        let mut f = CardFocus::default();
        assert!(!f.step(Left, 12), "left of the first card goes nowhere");
        assert!(!f.step(Up, 12), "above the first row goes nowhere");
        assert!(f.step(Right, 12));
        assert_eq!(f.index(), 1);
    }

    #[test]
    fn card_grid_vertical_movement_keeps_the_column_you_left_from() {
        // 12 cards over 5 columns: rows of 5, 5, 2.
        let mut f = CardFocus::default();
        for _ in 0..4 {
            f.step(Right, 12);
        }
        assert_eq!(f.index(), 4, "last column of the first row");

        f.step(Down, 12);
        assert_eq!(f.index(), 9, "same column, second row");

        // The last row is short, so this clamps to its end rather than overshooting.
        f.step(Down, 12);
        assert_eq!(f.index(), 11);

        // Coming back up returns to the column left from, not the one landed in.
        f.step(Up, 12);
        assert_eq!(f.index(), 9);
    }

    #[test]
    fn browse_grid_hands_off_to_the_chips_above_and_the_rail_to_the_right() {
        let mut f = BrowseFocus::default();
        assert_eq!(f.zone(), BrowseZone::Grid);

        f.step(Up, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::Chips, "up from the first row reaches the chips");
        assert!(!f.step(Up, &crate::grid::Packing::columns(40, 5), 4, 26, 30), "the chips are the top of the page");

        // Back down and out to the right edge.
        f.step(Down, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::Grid);
        for _ in 0..4 {
            f.step(Right, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        }
        assert_eq!(f.index(), 4, "the last column of the row");
        f.step(Right, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::Rail, "right off the last column reaches the rail");
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::Grid, "and left comes back");
    }

    #[test]
    fn the_system_list_is_left_of_both_the_grid_and_the_chips() {
        let mut f = BrowseFocus { system: 7, ..Default::default() };
        f.step(Right, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::Grid, "left inside a row is still a step along it");
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::SystemList, "left off the first column");
        assert_eq!(f.system, 7, "lands on the open system");

        // Up and Down walk the list and stop at its ends.
        f.step(Down, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.system, 8);
        for _ in 0..40 {
            f.step(Down, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        }
        assert_eq!(f.system, 29);

        // From the chips too, but only off the first one.
        let mut f = BrowseFocus::default();
        f.step(Up, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        f.chip = 2;
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!((f.zone(), f.chip), (BrowseZone::Chips, 1));
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        f.step(Left, &crate::grid::Packing::columns(40, 5), 4, 26, 30);
        assert_eq!(f.zone(), BrowseZone::SystemList);
    }

    #[test]
    fn browse_grid_keeps_its_column_down_into_a_short_last_row() {
        // 13 games over 5 columns: rows of 5, 5, 3.
        let mut f = BrowseFocus::default();
        for _ in 0..4 {
            f.step(Right, &crate::grid::Packing::columns(13, 5), 4, 0, 30);
        }
        assert_eq!(f.index(), 4);
        f.step(Down, &crate::grid::Packing::columns(13, 5), 4, 0, 30);
        assert_eq!(f.index(), 9);
        f.step(Down, &crate::grid::Packing::columns(13, 5), 4, 0, 30);
        assert_eq!(f.index(), 12, "clamped into the short row");
        f.step(Up, &crate::grid::Packing::columns(13, 5), 4, 0, 30);
        assert_eq!(f.index(), 9, "and back to the column left from");
    }

    #[test]
    fn a_filter_that_empties_the_grid_moves_the_cursor_off_it() {
        let mut f = BrowseFocus::default();
        f.focus_index(30, &crate::grid::Packing::columns(40, 5));
        f.reconcile(0, 4, 0);
        assert_eq!(f.zone(), BrowseZone::Chips, "there is nothing in the grid to sit on");
        assert_eq!(f.index(), 0);
    }

    #[test]
    fn a_filter_that_shrinks_the_grid_pulls_the_cursor_back_in() {
        let mut f = BrowseFocus::default();
        f.focus_index(300, &crate::grid::Packing::columns(400, 5));
        f.reconcile(12, 4, 26);
        assert_eq!(f.index(), 11, "clamped onto the last surviving tile");
        assert_eq!(f.zone(), BrowseZone::Grid);
    }

    #[test]
    fn the_rail_is_not_reachable_when_a_system_has_no_letters() {
        let mut f = BrowseFocus::default();
        for _ in 0..6 {
            f.step(Right, &crate::grid::Packing::columns(40, 5), 4, 0, 30);
        }
        assert_eq!(f.zone(), BrowseZone::Grid, "with no rail, the right edge just stops");
    }
}
