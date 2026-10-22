//! The game page's cursor past its row of actions: the screenshots, one of them open over the whole
//! window, and the heading the achievements fold under — see [`crate::ui::detail`].

use super::*;
use crate::ui::detail::{self, Zone};
use crate::ui::reveal;

impl App {
    /// The game whose page is open, and which of its screenshots is open over it, if one is.
    pub(super) fn shot_open(&self) -> Option<(&Game, usize)> {
        let index = self.detail_focus.viewing?;
        let Screen::Browsing(Route::Detail { rom_id, .. }) = &self.screen else { return None };
        Some((self.library.game(*rom_id)?, index))
    }

    /// The game whose page is open.
    fn detail_game(&self) -> Option<&Game> {
        let Screen::Browsing(Route::Detail { rom_id, .. }) = &self.screen else { return None };
        self.library.game(*rom_id)
    }

    /// How many screenshots the open page shows — see [`detail::shown_shots`].
    fn page_shots(&self) -> usize {
        self.detail_game().map_or(0, |game| detail::shown_shots(game, &self.covers).len())
    }

    /// The rows of the open page the cursor stops on, top to bottom.
    pub(super) fn detail_zones(&self) -> Vec<Zone> {
        Zone::of(self.page_shots(), self.detail_game().map_or(0, |game| game.achievements))
    }

    /// A step of the cursor on the game page. Left and Right walk the row it is on; Up and Down go
    /// from row to row, and past the last one read on down the page.
    pub(super) fn navigate_detail(&mut self, direction: Direction) -> Task<Message> {
        use iced::widget::operation::scroll_by;
        use iced::widget::scrollable::AbsoluteOffset;
        let zones = self.detail_zones();
        // A row that is not there now — screenshots that never came — gives the cursor back to the
        // actions.
        if !zones.contains(&self.detail_focus.zone) {
            self.detail_focus.zone = Zone::Actions;
        }
        let at = zones.iter().position(|zone| *zone == self.detail_focus.zone).unwrap_or(0);
        let read = |step: f32| scroll_by(detail::detail_id(), AbsoluteOffset { x: 0.0, y: step });
        match (self.detail_focus.zone, direction) {
            (Zone::Actions, Direction::Left | Direction::Right) => {
                let last = self.detail_actions().len().saturating_sub(1);
                let action = self.detail_focus.action.min(last);
                self.detail_focus.action = if direction == Direction::Left { action.saturating_sub(1) } else { (action + 1).min(last) };
                Task::none()
            }
            (Zone::Shots, Direction::Left | Direction::Right) => {
                let last = self.page_shots().saturating_sub(1);
                let shot = self.detail_focus.shot.min(last);
                self.detail_focus.shot = if direction == Direction::Left { shot.saturating_sub(1) } else { (shot + 1).min(last) };
                self.scroll_cursor_into_view()
            }
            (Zone::Achievements, Direction::Left | Direction::Right) => Task::none(),
            (_, Direction::Down) => match zones.get(at + 1) {
                Some(next) => {
                    self.detail_focus.zone = *next;
                    self.scroll_cursor_into_view()
                }
                // Under the last row, Down reads on down the page: the description, an opened list
                // of achievements.
                None => read(detail::READ_STEP),
            },
            // At the page's top, Up goes on to the tab bar.
            (Zone::Actions, Direction::Up) if self.detail_offset <= 0.5 => {
                self.up_stopped = true;
                Task::none()
            }
            (Zone::Actions, Direction::Up) => read(-detail::READ_STEP),
            // Back up to the top of the row the cursor is on before leaving it, so a list read down
            // a step at a time is read back up the same way — and then on to the row above.
            (zone, Direction::Up) => reveal::climb(detail::detail_id(), detail::part_id(zone), detail::READ_STEP, detail::REVEAL_MARGIN)
                .map(Message::DetailClimbed),
        }
    }

    /// Where a step up the page went: on up the page, or — the row's top in view — to the row above.
    pub(super) fn detail_climbed(&mut self, to: Option<f32>) -> Task<Message> {
        if let Some(offset) = to {
            self.detail_offset = offset;
            return Task::none();
        }
        let zones = self.detail_zones();
        match zones.iter().position(|zone| *zone == self.detail_focus.zone) {
            Some(at) if at > 0 => {
                self.detail_focus.zone = zones[at - 1];
                self.scroll_cursor_into_view()
            }
            _ => Task::none(),
        }
    }

    /// Bring the row the cursor is on into view, and on the screenshots the one it is on.
    pub(super) fn reveal_detail_cursor(&self) -> Task<Message> {
        let zone = self.detail_focus.zone;
        let page = reveal::reveal_part(detail::detail_id(), detail::part_id(zone), detail::REVEAL_MARGIN).map(Message::DetailScrolled);
        match (zone, self.detail_game()) {
            (Zone::Shots, Some(game)) => {
                let (left, right) = detail::shot_span(game, &self.covers, self.detail_focus.shot);
                // With a peek of the next, so the row says it goes on.
                Task::batch([page, reveal::reveal_across(detail::shots_strip_id(), left, right, 48.0).discard()])
            }
            _ => page,
        }
    }

    /// Confirm on the game page: the action under the cursor, the screenshot opened over the
    /// window, or the achievements opened or folded away.
    pub(super) fn activate_detail(&mut self) -> Task<Message> {
        match self.detail_focus.zone {
            Zone::Actions => {
                let actions = self.detail_actions();
                match actions.get(self.detail_focus.action.min(actions.len().saturating_sub(1))) {
                    Some(action) => self.update(Message::DetailAction(*action)),
                    None => Task::none(),
                }
            }
            Zone::Shots => self.view_shot(self.detail_focus.shot),
            Zone::Achievements => self.toggle_achievements(),
        }
    }

    /// Open screenshot `index` over the window, or step the open one to it. The row's cursor goes
    /// with it, so closing leaves the cursor on the last one looked at.
    pub(super) fn view_shot(&mut self, index: usize) -> Task<Message> {
        let shots = self.page_shots();
        if shots == 0 {
            return Task::none();
        }
        let index = index.min(shots - 1);
        self.detail_focus.viewing = Some(index);
        self.detail_focus.zone = Zone::Shots;
        self.detail_focus.shot = index;
        // Its full-size copy, and its neighbours' ahead of a step.
        self.fetch_art()
    }

    /// Close the screenshot open over the window, with the row scrolled to it: a few steps along,
    /// it may be past the ones the row showed.
    pub(super) fn close_shot(&mut self) -> Task<Message> {
        self.detail_focus.viewing = None;
        self.scroll_cursor_into_view()
    }

    /// Open the achievements' list, or fold it away — and put the cursor on its heading, where
    /// Confirm does the same.
    pub(super) fn toggle_achievements(&mut self) -> Task<Message> {
        self.achievements_open = !self.achievements_open;
        self.detail_focus.zone = Zone::Achievements;
        // The badges are asked for only while the list shows them.
        Task::batch([self.fetch_art(), self.scroll_cursor_into_view()])
    }

    /// The keys a screenshot open over the window answers: Left and Right step through the page's,
    /// and Confirm and Back close it. The rest wait until it is closed, as the pointer does: a
    /// key that walked the page under it, or left it, would act on what cannot be seen.
    pub(super) fn answer_viewer(&mut self, message: &Message) -> Option<Task<Message>> {
        let index = self.detail_focus.viewing?;
        Some(match message {
            Message::Nav(Direction::Left) => match index.checked_sub(1) {
                Some(before) => self.view_shot(before),
                None => Task::none(),
            },
            Message::Nav(Direction::Right) => self.view_shot(index + 1),
            Message::Activate | Message::Dismiss | Message::CloseShot => self.close_shot(),
            Message::Nav(_)
            | Message::KeyChar(..)
            | Message::CycleTab(_)
            | Message::CycleSystem(_)
            | Message::TabSelected(_)
            | Message::OpenSearch
            | Message::RemoveFocusedDownload
            | Message::DetailAction(_)
            | Message::ToggleAchievements => Task::none(),
            _ => return None,
        })
    }
}
