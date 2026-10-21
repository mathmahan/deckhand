//! The pad's and the keyboard's cursor: moving it, keeping it in view, and pressing what it is on.

use super::*;

impl App {
    /// Bring the cursor's row, and its card, into view.
    ///
    /// Stepping down past the last visible row and stepping right past the last visible card are
    /// the same problem, and without this the cursor walks off the screen and navigation becomes
    /// guesswork. Home is the exception that proves it: its shelves slide by themselves — see
    /// [`crate::animation::Glide`] — so only the sideways half is asked for here.
    pub(super) fn scroll_cursor_into_view(&mut self) -> Task<Message> {
        use crate::ui::home;
        use iced::widget::operation::scroll_to;
        use iced::widget::scrollable::AbsoluteOffset;

        let Screen::Browsing(route) = &self.screen else { return Task::none() };
        match route {
            // Across the shelf with the cursor, only as far as it takes to show the card with a peek of
            // the next: the cursor walks over the cards already on screen before the row moves.
            // The column's slide to the shelf is the glide's, which `update` starts.
            Route::Home => match self.shelves.get(self.focus.shelf_pos()) {
                Some(shelf) => {
                    let (left, right) = home::card_span(shelf, self.focus.index_of(shelf), &self.covers, &self.library);
                    reveal::reveal_across(home::shelf_id(shelf), left, right, home::SHELF_PEEK).discard()
                }
                None => Task::none(),
            },
            Route::Collections => {
                // Pinned to the cursor's row, and mirrored by hand: a scroll by operation fires no
                // `on_scroll`, and the mirror is what the cover window is cut from.
                let y = crate::ui::collections::row_offset(self.collections_focus.cards.index());
                self.collections_scroll = y;
                scroll_to(crate::ui::collections::page_id(), AbsoluteOffset { x: 0.0, y })
            }
            Route::Browse { scope } => match self.browse_focus.zone() {
                crate::focus::BrowseZone::Grid => {
                    let (spec, packing) = self.layout(scope);
                    let top = crate::ui::browse::row_top(&spec, packing.row_of(self.browse_focus.index()));
                    let bottom = top + spec.tile_h + crate::grid::LABEL_H;
                    // Only as far as it takes to show the whole row, rather than pinning it to the
                    // top: a cursor walking inside what is already on screen leaves the page where
                    // it is. How much is on screen is measured, because the grid's viewport is
                    // whatever the chrome above it leaves — see `ui::reveal`.
                    //
                    // A scroll by operation never fires `on_scroll`, so the move is reported as a
                    // scroll by hand: that is what moves the mirror, and what fetches the art for
                    // the rows it lands on.
                    reveal::reveal(crate::ui::browse::grid_id(), top, bottom, 0.0)
                        .map(Message::GridScrolled)
                }
                crate::focus::BrowseZone::SystemList => reveal_system_row(self.browse_focus.system),
                // On the chips or the rail the cursor has left the grid, and the grid stays where
                // it was.
                crate::focus::BrowseZone::Chips | crate::focus::BrowseZone::Rail => Task::none(),
            },
            // The detail page's cursor is one row of buttons, always on screen.
            Route::Detail { .. } => Task::none(),
            Route::Saves { rom_id, .. } => {
                let row = self.saves_focus.row(self.saves_rows(*rom_id));
                // Under the heading and the tabs, the list has most of the page.
                let y = crate::ui::saves::scroll_offset(row, self.content_height * 0.7);
                scroll_to(crate::ui::saves::list_id(), AbsoluteOffset { x: 0.0, y })
            }
            Route::Search { .. } => {
                let results = self.search_results().len();
                // Under the keyboard, the list has roughly half the page.
                let y = crate::ui::search::scroll_offset(self.search_focus.result(results), self.content_height / 2.0);
                scroll_to(crate::ui::search::results_id(), AbsoluteOffset { x: 0.0, y })
            }
            Route::Downloads => {
                let rows = self.download_rows();
                let index = self.downloads_focus.index(rows.len());
                scroll_to(
                    crate::ui::downloads::downloads_id(),
                    AbsoluteOffset {
                        x: 0.0,
                        y: crate::ui::downloads::scroll_offset(&rows, index, self.content_height),
                    },
                )
            }
            Route::Settings => scroll_to(
                crate::ui::settings::settings_id(),
                AbsoluteOffset {
                    x: 0.0,
                    y: crate::ui::settings::scroll_offset(
                        self.settings_focus.index(),
                        self.content_height,
                    ),
                },
            ),
        }
    }

    /// Route a cursor move to whichever page owns the cursor.
    ///
    /// Each page keeps its own cursor rather than sharing one, because they are different shapes:
    /// Home is bands of horizontal shelves, Systems is a uniform card grid that clamps at its
    /// edges, and Browse is a derived grid whose edges hand off to the chips, the system list and
    /// the letter rail.
    pub(super) fn navigate(&mut self, direction: Direction) -> Task<Message> {
        let Screen::Browsing(route) = &self.screen else { return Task::none() };
        if let Some(chrome) = self.chrome {
            return self.navigate_chrome(chrome, direction);
        }
        let task = self.navigate_page(route.clone(), direction);
        // Up with nowhere left to go on the page is up into the tab bar — how a pad reaches every
        // destination, and the banner above it.
        if direction == Direction::Up && self.up_stopped {
            self.up_stopped = false;
            self.chrome = Some(Chrome::Tabs);
        }
        task
    }

    /// The pad's cursor in the chrome: Left/Right walk the destinations — each opening as the cursor
    /// reaches it — Up reaches the banner's Dismiss, Down goes back into the page.
    pub(super) fn navigate_chrome(&mut self, chrome: Chrome, direction: Direction) -> Task<Message> {
        match (chrome, direction) {
            (Chrome::Tabs, Direction::Left | Direction::Right) => {
                let task = self.update(Message::CycleTab(if direction == Direction::Left { -1 } else { 1 }));
                self.chrome = Some(Chrome::Tabs);
                task
            }
            (Chrome::Tabs, Direction::Up) if self.banner.is_some() => {
                self.chrome = Some(Chrome::Banner);
                Task::none()
            }
            (Chrome::Tabs, Direction::Down) => {
                self.chrome = None;
                Task::none()
            }
            (Chrome::Banner, Direction::Down) => {
                self.chrome = Some(Chrome::Tabs);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// The pad on the page itself. Sets [`App::up_stopped`] when Up had nowhere to go.
    pub(super) fn navigate_page(&mut self, route: Route, direction: Direction) -> Task<Message> {
        let up = direction == Direction::Up;
        match route {
            Route::Home => {
                let moved = match direction {
                    Direction::Left => self.focus.step(&self.shelves, -1),
                    Direction::Right => self.focus.step(&self.shelves, 1),
                    Direction::Up => self.focus.step_vertical(&self.shelves, -1),
                    Direction::Down => self.focus.step_vertical(&self.shelves, 1),
                };
                self.up_stopped = up && !moved;
                if moved { self.scroll_cursor_into_view() } else { Task::none() }
            }
            Route::Collections => {
                let kinds = self.all_collections.kinds();
                let cards = self.collection_cards().len();
                let kind = self.collections_focus.kind;
                if self.collections_focus.step(direction, kinds.len(), cards) {
                    // A new kind is a new set of covers.
                    let art = if self.collections_focus.kind != kind { self.fetch_art() } else { Task::none() };
                    Task::batch([self.scroll_cursor_into_view(), art])
                } else {
                    self.up_stopped = up;
                    Task::none()
                }
            }
            Route::Browse { scope } => {
                // Right out of the list is the way back to the grid, and whether that means opening
                // another system or collection depends on which one the list's cursor is on.
                if self.browse_focus.zone() == crate::focus::BrowseZone::SystemList
                    && direction == Direction::Right
                {
                    return self.leave_system_list(&scope);
                }

                let (view, spec) = self.scope_view(&scope);
                let letters = view.letters.len();
                let packing = self.packing(&view, &spec);
                let places = self.places(&scope).len();
                if self.browse_focus.step(direction, &packing, CHIP_COUNT, letters, places) {
                    self.scroll_cursor_into_view()
                } else {
                    self.up_stopped = up;
                    Task::none()
                }
            }
            Route::Detail { .. } => {
                // The detail page's cursor is its action row, which is one-dimensional.
                let last = self.detail_actions().len().saturating_sub(1);
                match direction {
                    Direction::Left => {
                        self.detail_focus.action = self.detail_focus.action.min(last).saturating_sub(1)
                    }
                    Direction::Right => {
                        self.detail_focus.action = (self.detail_focus.action + 1).min(last)
                    }
                    // At the page's top, Up goes on to the tab bar.
                    Direction::Up if self.detail_offset <= 0.5 => {
                        self.up_stopped = true;
                        return Task::none();
                    }
                    // Up and Down read the page — the description, the screenshots, the
                    // achievements below the action row — a few lines at a time.
                    Direction::Up | Direction::Down => {
                        let step = if direction == Direction::Down { 160.0 } else { -160.0 };
                        return iced::widget::operation::scroll_by(
                            crate::ui::detail::detail_id(),
                            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: step },
                        );
                    }
                }
                Task::none()
            }
            Route::Search { .. } => {
                let results = self.search_results().len();
                if self.search_focus.step(direction, results) {
                    self.scroll_cursor_into_view()
                } else {
                    self.up_stopped = up;
                    Task::none()
                }
            }
            Route::Saves { rom_id, .. } => {
                let rows = self.saves_rows(rom_id);
                if self.saves_focus.step(direction, rows) {
                    self.scroll_cursor_into_view()
                } else {
                    self.up_stopped = up;
                    Task::none()
                }
            }
            Route::Downloads => {
                // One flat list, so only Up and Down mean anything — the two sections are one
                // thing to walk. See [`crate::ui::downloads`].
                let rows = self.download_rows().len();
                let moved = match direction {
                    Direction::Up => self.downloads_focus.step(-1, rows),
                    Direction::Down => self.downloads_focus.step(1, rows),
                    Direction::Left | Direction::Right => false,
                };
                self.up_stopped = up && !moved;
                if moved { self.scroll_cursor_into_view() } else { Task::none() }
            }
            Route::Settings => match direction {
                Direction::Up | Direction::Down => {
                    let delta = if direction == Direction::Up { -1 } else { 1 };
                    if self.settings_focus.step(delta) {
                        self.scroll_cursor_into_view()
                    } else {
                        self.up_stopped = up;
                        Task::none()
                    }
                }
                // Left/Right adjust a stepped row in place. They stop at its ends rather than
                // wrapping: holding Right should settle on Huge, not cycle through Normal again.
                Direction::Left | Direction::Right => {
                    let setting = self.settings_focus.row();
                    let Some((options, selected)) = setting.stepped(&self.settings) else {
                        return Task::none();
                    };
                    let next = match direction {
                        Direction::Left => selected.saturating_sub(1),
                        _ => (selected + 1).min(options.len() - 1),
                    };
                    if next == selected {
                        Task::none()
                    } else {
                        self.pick_setting(setting, next)
                    }
                }
            },
        }
    }

    /// Confirm on whatever the cursor is on, which differs per page.
    pub(super) fn activate(&mut self) -> Task<Message> {
        let Screen::Browsing(route) = &self.screen else { return Task::none() };
        match self.chrome {
            // On the bar, the destination the cursor is on is already open: confirm goes into it.
            Some(Chrome::Tabs) => {
                self.chrome = None;
                return Task::none();
            }
            Some(Chrome::Banner) => return self.update(Message::DismissBanner),
            None => {}
        }
        match route.clone() {
            // A card's game opens its page — the spotlight's View — where it is played or fetched, its
            // saves are, and it is a favourite. One press for every game on every shelf, and never a
            // download or a launch nobody meant.
            Route::Home => match self.focus.spotlit(&self.shelves).map(|g| g.rom_id) {
                Some(rom_id) => self.update(Message::OpenDetail(rom_id)),
                None => Task::none(),
            },
            Route::Collections => match self.collections_focus.zone {
                crate::collections::Zone::Kinds => {
                    self.collections_focus.zone = crate::collections::Zone::Cards;
                    Task::none()
                }
                crate::collections::Zone::Cards => {
                    let open = self.collection_cards().get(self.collections_focus.cards.index()).map(|c| c.open.clone());
                    match open {
                        Some(message) => self.update(message),
                        None => Task::none(),
                    }
                }
            },
            Route::Browse { scope } => {
                use crate::focus::BrowseZone;
                match self.browse_focus.zone() {
                    BrowseZone::Grid => {
                        let (view, _) = self.scope_view(&scope);
                        match view.games.get(self.browse_focus.index()).map(|g| g.rom_id) {
                            Some(id) => self.update(Message::OpenDetail(id)),
                            None => Task::none(),
                        }
                    }
                    BrowseZone::Chips => {
                        let chip = self.browse_focus.chip;
                        if chip < crate::view_model::Filter::ALL.len() {
                            self.update(Message::FilterSelected(crate::view_model::Filter::ALL[chip]))
                        } else {
                            let sort = crate::view_model::Sort::ALL[chip - crate::view_model::Filter::ALL.len()];
                            self.update(Message::SortSelected(sort))
                        }
                    }
                    BrowseZone::Rail => {
                        let (view, _) = self.scope_view(&scope);
                        match view.letters.get(self.browse_focus.letter).map(|(_, i)| *i) {
                            Some(index) => self.update(Message::JumpToLetter(index)),
                            None => Task::none(),
                        }
                    }
                    BrowseZone::SystemList => self.leave_system_list(&scope),
                }
            }
            Route::Detail { .. } => {
                let actions = self.detail_actions();
                match actions.get(self.detail_focus.action.min(actions.len().saturating_sub(1))) {
                    Some(action) => self.update(Message::DetailAction(*action)),
                    None => Task::none(),
                }
            }
            // An in-game save offers to come back; a state says where it is.
            Route::Saves { rom_id, .. } => self.press_save(rom_id),
            Route::Search { .. } => {
                if !self.search_focus.in_results {
                    let key = self.search_focus.key();
                    return self.update(Message::SearchKey(key));
                }
                let results = self.search_results();
                match results.get(self.search_focus.result(results.len())).map(|g| g.rom_id) {
                    Some(id) => self.update(Message::OpenDetail(id)),
                    None => Task::none(),
                }
            }
            Route::Downloads => {
                use crate::downloads::SyncState;
                let rows = self.download_rows();
                let Some(row) = rows.get(self.downloads_focus.index(rows.len())) else {
                    return Task::none();
                };
                let (rom_id, retry) = (row.rom_id, matches!(row.state, SyncState::Failed(_)));
                // Confirm opens the game, as it does from every other list in the app — except on
                // a row that failed, where the thing you came here to press is "again". Removal is
                // Delete, deliberately not this: it is not what a thumb finds by accident.
                if retry {
                    self.update(Message::Download(rom_id))
                } else {
                    // Another build's row opens the game it is a build of.
                    let tile = self.game_of(rom_id).map_or(rom_id, |g| g.rom_id);
                    self.update(Message::OpenDetail(tile))
                }
            }
            Route::Settings => self.activate_setting(self.settings_focus.row()),
        }
    }

    /// Right or Enter in the system list: open the system under the list's cursor.
    ///
    /// Unless it is the one already open, in which case this goes back to the grid where its cursor
    /// was, rather than reopening the system at the top. A stray Left off the grid's first column is
    /// the ordinary way into the list, and Right has to undo it.
    pub(super) fn leave_system_list(&mut self, open: &Scope) -> Task<Message> {
        use crate::focus::BrowseZone;
        let Some(target) = self.places(open).get(self.browse_focus.system).map(|p| p.scope.clone()) else {
            return Task::none();
        };
        if target != *open {
            return self.open_scope(target);
        }
        // An empty grid — a filter that left nothing — has no tile to go back to.
        let zone = if self.scope_view(open).0.is_empty() { BrowseZone::Chips } else { BrowseZone::Grid };
        self.browse_focus.set_zone(zone);
        Task::none()
    }
}
