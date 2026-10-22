//! The save sync page's cursor and what it opens — see [`crate::ui::save_sync`].

use super::*;
use crate::ui::save_sync::{self as page, Filter, Group};

impl App {
    /// Open the page over the one the cursor is on — the header's cloud, or the sync card's Details.
    pub(super) fn open_sync(&mut self) -> Task<Message> {
        let from = match &self.screen {
            // Pressed again on the page itself: back to its top, where it came from stays.
            Screen::Browsing(Route::Sync { .. }) => {
                self.sync_focus.card = 0;
                return self.scroll_page_to_top();
            }
            Screen::Browsing(route) => route.clone(),
            _ => Route::Home,
        };
        self.sync_focus = Default::default();
        self.screen = Screen::Browsing(Route::Sync { from: Box::new(from) });
        Task::batch([self.scroll_page_to_top(), self.fetch_art()])
    }

    /// The report the page lists: the last one, and while a sync runs the one before it.
    fn shown_report(&self) -> Option<&crate::sync::Report> {
        let shown = if matches!(self.save_sync, SaveSync::Running) { &self.sync_before } else { &self.save_sync };
        match shown {
            SaveSync::Done(report) => Some(report),
            _ => None,
        }
    }

    /// The page's cards, as its filter shows them.
    pub(super) fn sync_groups(&self) -> Vec<Group<'_>> {
        let events = self.shown_report().map_or(&[][..], |r| r.events.as_slice());
        page::groups(events, self.sync_focus.filter, &self.library)
    }

    /// The games whose covers the page draws, nearest the cursor first.
    pub(super) fn sync_covers(&self) -> Vec<&Game> {
        self.sync_groups().into_iter().filter_map(|g| g.game).take(60).collect()
    }

    /// Up and Down from card to card; Up off the first is Up into the bar.
    pub(super) fn navigate_sync(&mut self, direction: Direction) -> Task<Message> {
        let cards = self.sync_groups().len();
        let card = self.sync_focus.card.min(cards.saturating_sub(1));
        match direction {
            Direction::Up if card == 0 => {
                self.up_stopped = true;
                Task::none()
            }
            Direction::Up => {
                self.sync_focus.card = card - 1;
                self.scroll_cursor_into_view()
            }
            Direction::Down if card + 1 < cards => {
                self.sync_focus.card = card + 1;
                self.scroll_cursor_into_view()
            }
            _ => Task::none(),
        }
    }

    /// The triggers: the filter before or after, of those the run has anything for.
    pub(super) fn cycle_sync_filter(&mut self, delta: isize) -> Task<Message> {
        let events = self.shown_report().map_or(&[][..], |r| r.events.as_slice());
        let shown = Filter::shown(events);
        let at = shown.iter().position(|f| *f == self.sync_focus.filter).unwrap_or(0);
        let to = at.saturating_add_signed(delta).min(shown.len().saturating_sub(1));
        match shown.get(to) {
            Some(filter) if to != at => self.update(Message::SyncFilter(*filter)),
            _ => Task::none(),
        }
    }

    /// Show one kind of what the run did, from its first card.
    pub(super) fn show_sync_filter(&mut self, filter: Filter) -> Task<Message> {
        self.sync_focus = page::SyncFocus { filter, card: 0 };
        Task::batch([self.scroll_page_to_top(), self.fetch_art()])
    }

    /// Confirm: the game the cursor's card is about, on its own page.
    pub(super) fn activate_sync(&mut self) -> Task<Message> {
        let card = self.sync_focus.card;
        match self.sync_groups().get(card).and_then(|g| g.game).map(|g| g.rom_id) {
            Some(rom_id) => self.update(Message::OpenDetail(rom_id)),
            None => Task::none(),
        }
    }

    /// The cursor's card in view: the first with the run's heading over it, any other by as little
    /// as brings it in.
    pub(super) fn reveal_sync_cursor(&self) -> Task<Message> {
        use iced::widget::operation::scroll_to;
        use iced::widget::scrollable::AbsoluteOffset;
        if self.sync_focus.card == 0 {
            return scroll_to(page::page_id(), AbsoluteOffset { x: 0.0, y: 0.0 });
        }
        reveal::reveal_part(page::page_id(), page::cursor_id(), 24.0).discard()
    }

    /// Name the game of each save the run only knew by name, by the library's file stems — once, as
    /// the report comes in.
    pub(super) fn name_games(&self, report: &mut crate::sync::Report) {
        if report.events.iter().all(|e| e.rom_id.is_some()) {
            return;
        }
        let builds = crate::sync::builds(&self.library);
        for event in report.events.iter_mut().filter(|e| e.rom_id.is_none()) {
            event.rom_id = crate::sync::game_of(&builds, &event.name);
        }
    }

    pub(super) fn save_sync_context(&self) -> page::SaveSyncContext<'_> {
        let back_to = match &self.screen {
            Screen::Browsing(Route::Sync { from }) => from.tab().label(),
            _ => Tab::Home.label(),
        };
        page::SaveSyncContext {
            focus: &self.sync_focus,
            library: &self.library,
            covers: &self.covers,
            save_sync: &self.save_sync,
            sync_before: &self.sync_before,
            synced_ago: self.save_synced_at.map(|at| at.elapsed()),
            progress: crate::sync::progress::now(),
            watch_saves: self.settings.watch_saves,
            back_to,
        }
    }
}
