//! What the window draws.

use super::*;
use iced::widget::{column, stack};

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<Message> = match &self.screen {
            Screen::Login(state) => login_view(state),
            Screen::Loading { status } => loading_view(status),
            Screen::Browsing(route) => self.browse_body(route),
        };

        // The bar is chrome for the browse screens only. On login it would be a frame around a
        // form, naming destinations that do not exist yet for a server you are not on.
        let chrome: Element<Message> = if let Screen::Browsing(route) = &self.screen {
            // With the cursor up in the bar or on the banner the page is dimmed under a scrim, so
            // its own cursor — still drawn, as where Down goes back to — is not taken for a second
            // one. The scrim takes no input: a pointer reaches the page through it.
            //
            // Always there, and clear while the cursor is in the page, so the widget tree keeps its
            // shape. Adding it reset every card's hover and every row's scroll beneath it: a card
            // under a resting pointer then reported the pointer entering it, all over again.
            let dim = self.chrome.is_some();
            let scrim = container(Space::new()).width(Length::Fill).height(Length::Fill).style(move |_| container::Style {
                background: dim.then_some(Background::Color(theme::alpha(theme::BACKGROUND, 0.6))),
                ..Default::default()
            });
            let body: Element<Message> = stack![body, scrim].into();
            let page = column![tab_bar(route.tab(), self.chrome == Some(Chrome::Tabs), self.header_status()), body];
            // Home's picture runs to the top of the window, under the header: the bar floats on it.
            if *route == Route::Home {
                stack![crate::ui::home::page_backdrop(&self.home_context()), page].into()
            } else {
                page.into()
            }
        } else {
            body
        };

        if let Some(pending) = &self.conflict {
            return conflict_view(pending, &self.title_of(pending.rom_id));
        }
        if let Some(handover) = &self.handover {
            return self.handover_view(handover);
        }
        if let Some(pending) = &self.entry {
            return crate::ui::entry::view(&pending.entry);
        }
        if let Some(pending) = &self.pick {
            return crate::ui::picker::view(&pending.picker);
        }
        // The banner's place is always there, empty without one, so the page keeps its place in the
        // widget tree. Pushed in only when a banner came, it moved the page down a slot, and every
        // scrollable in it started over at the top — Settings jumped up as an update was found and
        // again as its banner went.
        let banner: Element<Message> = match &self.banner {
            Some(banner) => banner_view(banner, self.chrome == Some(Chrome::Banner)),
            None => Space::new().height(Length::Fixed(0.0)).into(),
        };
        let page = column![banner, chrome];

        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(theme::BACKGROUND)),
                ..Default::default()
            })
            .into()
    }

    /// The whole window while a game is handed to its emulator or back — see [`Handover`]. Its
    /// cover when that is on hand, what it is waiting on, and on the way out that Deckhand has to
    /// stay open for it.
    fn handover_view(&self, handover: &Handover) -> Element<'_, Message> {
        const COVER_H: f32 = 220.0;
        let title = self.title_of(handover.rom_id);
        let (headline, status) = match (handover.closing, self.settings.connection.is_some()) {
            (true, _) => (format!("Saving {title}"), "Syncing its saves to RomM…"),
            (false, true) => (format!("Opening {title}"), "Syncing BIOS and saves with RomM…"),
            (false, false) => (format!("Opening {title}"), "Starting…"),
        };
        let mut card = column![].spacing(14).align_x(Alignment::Center).max_width(640);
        let url = self.library.game_of_build(handover.rom_id).and_then(|g| g.cover_url.as_deref());
        if let Some((handle, meta)) = url.and_then(|u| self.covers.feature(u)) {
            card = card.push(
                iced::widget::image(handle)
                    .content_fit(iced::ContentFit::Cover)
                    .width(Length::Fixed(COVER_H * meta.ratio))
                    .height(Length::Fixed(COVER_H))
                    .border_radius(8.0),
            );
            card = card.push(Space::new().height(Length::Fixed(10.0)));
        }
        card = card
            .push(
                text(headline)
                    .size(theme::text_size::HEADLINE_SMALL)
                    .font(theme::BOLD)
                    .color(theme::TEXT_PRIMARY)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .push(text(status).size(theme::text_size::BODY_LARGE).color(theme::TEXT_SECONDARY))
            .push(sweep(handover.since, std::time::Instant::now()));
        if handover.closing {
            card = card.push(
                text("Don't close Deckhand yet")
                    .size(theme::text_size::LABEL_LARGE)
                    .font(theme::MEDIUM)
                    .color(theme::ACCENT_SOFT),
            );
        }
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(24)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style { background: Some(Background::Color(theme::BACKGROUND)), ..Default::default() })
            .into()
    }

    /// The rom under the Browse cursor, if that is where the cursor is.
    ///
    /// Used as an anchor across a filter or sort change: the view is rebuilt from scratch, so
    /// without this the cursor keeps its *index* and lands on whatever game moved into that slot.
    pub(super) fn focused_rom_id(&self) -> Option<i64> {
        let Screen::Browsing(Route::Browse { scope }) = &self.screen else { return None };
        let (view, _) = self.scope_view(scope);
        view.games.get(self.browse_focus.index()).map(|g| g.rom_id)
    }

    /// Re-seat the Browse cursor after the view was rebuilt, keeping `anchor` under it when the new
    /// view still contains that game, and clamping into the new bounds when it does not.
    pub(super) fn refocus_browse(&mut self, anchor: Option<i64>) -> Task<Message> {
        let Screen::Browsing(Route::Browse { scope }) = &self.screen else { return Task::none() };
        let scope = scope.clone();
        let (view, spec) = self.scope_view(&scope);
        let (len, letters) = (view.len(), view.letters.len());
        let packing = self.packing(&view, &spec);
        match anchor.and_then(|id| view.position_of(id)) {
            Some(index) => self.browse_focus.focus_index(index, &packing),
            None => self.browse_focus.reconcile(len, CHIP_COUNT, letters),
        }
        self.scroll_cursor_into_view()
    }

    /// Put a freshly-opened page at its top. A page entered from elsewhere should start at its
    /// beginning, not wherever the previous page's scrollable happened to sit.
    pub(super) fn scroll_page_to_top(&mut self) -> Task<Message> {
        use iced::widget::operation::scroll_to;
        use iced::widget::scrollable::AbsoluteOffset;
        let zero = AbsoluteOffset { x: 0.0, y: 0.0 };
        // `scroll_to` never fires `on_scroll`, so the mirror has to be moved by hand or the row
        // window would still be describing the page we just left.
        self.grid_scroll = 0.0;
        self.collections_scroll = 0.0;
        Task::batch([
            scroll_to(crate::ui::browse::grid_id(), zero),
            scroll_to(crate::ui::detail::detail_id(), zero),
            scroll_to(crate::ui::collections::page_id(), zero),
            scroll_to(crate::ui::downloads::downloads_id(), zero),
            scroll_to(crate::ui::settings::settings_id(), zero),
        ])
    }

    /// Everything Home draws from — its page and the picture under it.
    pub(super) fn home_context(&self) -> HomeContext<'_> {
        let spotlit = self.focus.spotlit(&self.shelves);
        let (title, summary) = self.spotlight_words();
        // The game's own page has the freshest count, once it has been opened; otherwise what the
        // user's record said at launch.
        let earned = spotlit.and_then(|game| match self.achievements.get(&game.rom_id) {
            Some(Ok(list)) if list.linked == Some(true) => Some(list.earned_count()),
            _ => self.awarded.as_ref().map(|awarded| game.ra_id.and_then(|id| awarded.get(&id).copied()).unwrap_or(0)),
        });
        HomeContext {
            library: &self.library,
            shelves: &self.shelves,
            focus: &self.focus,
            covers: &self.covers,
            downloads: &self.downloads,
            host: self.host(),
            fade: &self.fade,
            glide: &self.glide,
            backdrop: &self.backdrop,
            now: std::time::Instant::now(),
            hovered: self.hovered,
            favourites: &self.favourites,
            earned,
            title,
            summary,
            this_year: crate::library::this_year(),
            compact: self.content_height - crate::ui::tab_bar::TAB_BAR_HEIGHT < crate::ui::home::COMPACT_BELOW,
        }
    }

    /// The spotlit game's name and description, cut to the lines the spotlight gives them —
    /// measured the first time a game is spotlit, and kept.
    fn spotlight_words(&self) -> (String, String) {
        use crate::ui::home;
        let Some(game) = self.focus.spotlit(&self.shelves) else { return Default::default() };
        if let Some(words) = self.spotlight_text.borrow().get(&game.rom_id) {
            return words.clone();
        }
        let size = home::title_size(&game.title);
        let title = home::clamp_lines(&game.title, home::SPOTLIGHT_W, 2, size, size * 1.05, theme::DISPLAY);
        let summary = game
            .summary
            .as_deref()
            .map(|summary| home::clamp_lines(summary, home::SPOTLIGHT_W - 40.0, 2, home::SUMMARY_SIZE, home::SUMMARY_LINE, theme::REGULAR))
            .unwrap_or_default();
        let mut kept = self.spotlight_text.borrow_mut();
        // Bounded by what a session walks past; past that it starts again rather than growing.
        if kept.len() > 512 {
            kept.clear();
        }
        kept.insert(game.rom_id, (title.clone(), summary.clone()));
        (title, summary)
    }

    /// What the header's chips say: the downloads coming and how far they have got, and where the
    /// saves stand.
    pub(super) fn header_status(&self) -> crate::ui::tab_bar::Status {
        use crate::ui::tab_bar::{Saves, Status};
        let (count, done) = self.downloads.in_progress().fold((0usize, 0.0f32), |(count, done), (_, progress)| (count + 1, done + progress));
        let saves = match &self.save_sync {
            SaveSync::Idle => Saves::Quiet,
            SaveSync::Running => Saves::Syncing,
            SaveSync::Done(report) if !report.failed.is_empty() => Saves::Attention,
            SaveSync::Done(_) => Saves::InSync,
            SaveSync::Failed(_) => Saves::Failed,
        };
        Status { downloads: (count > 0).then(|| (count, done / count as f32)), saves }
    }

    pub(super) fn host(&self) -> &str {
        self.settings.connection.as_ref().map(|c| c.host.as_str()).unwrap_or_default()
    }

    /// The system a route is showing, falling back to the first in the library.
    pub(super) fn current_system(&self, route: &Route) -> Option<String> {
        match route {
            Route::Browse { scope: Scope::System(system) } => Some(system.clone()),
            Route::Detail { rom_id, .. } => self.library.game(*rom_id).map(|g| g.system_id.clone()),
            _ => None,
        }
    }

    /// The games a scope holds: a system's, or a collection's across every system it touches.
    pub(super) fn scope_games(&self, scope: &Scope) -> Vec<&Game> {
        match scope {
            Scope::System(id) => self.library.games_for(id).iter().collect(),
            Scope::Collection(key) => self.all_collections.games(key, &self.library),
        }
    }

    /// The filtered, sorted view of a scope, plus the grid geometry it is laid out with.
    pub(super) fn scope_view(&self, scope: &Scope) -> (crate::view_model::SystemView<'_>, crate::grid::GridSpec) {
        let (games, letters, ratio) = {
            let entry = self.browse_entry(scope);
            (entry.games.clone(), entry.letters.clone(), entry.ratio)
        };
        let view = crate::view_model::SystemView { games: games.into_iter().map(|at| self.library.at(at)).collect(), letters };
        (view, self.grid_spec_for(ratio))
    }

    /// What Browse shows of a scope under the current filter and sort — worked out when one of them,
    /// the library, the downloaded games or the favourites change, and read back on every other
    /// frame and key press. Filtering and sorting a scope, and counting its chips, once happened
    /// several times a frame.
    pub(super) fn browse_entry(&self, scope: &Scope) -> std::cell::Ref<'_, BrowseEntry> {
        let key = BrowseKey {
            scope: scope.clone(),
            filter: self.filter,
            sort: self.sort,
            library: self.library.generation(),
            installed: self.downloads.installed_fingerprint(),
            favourites: self.favourites.iter().fold(self.favourites.len() as u64, |h, id| h ^ (*id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
        };
        if self.browse_cache.borrow().as_ref().is_none_or(|entry| entry.key != key) {
            let games = self.scope_games(scope);
            let installed = self.downloads.installed_ids();
            let counts = crate::ui::browse::ChipCounts {
                all: games.len(),
                installed: games.iter().filter(|g| g.builds().any(|id| installed.contains(&id))).count(),
                unplayed: games.iter().filter(|g| g.last_played.as_deref().is_none_or(|s| s.trim().is_empty())).count(),
                favourites: games.iter().filter(|g| g.builds().any(|id| self.favourites.contains(&id))).count(),
            };
            let ratio = match scope {
                Scope::System(id) => self.library.cover_ratio(id),
                // A collection's grid takes the shape most of its games share; its tiles crop the
                // others to fit, as a system's do its strays.
                Scope::Collection(_) => {
                    let mut shapes: HashMap<&str, usize> = HashMap::new();
                    for game in &games {
                        *shapes.entry(game.system_id.as_str()).or_default() += 1;
                    }
                    shapes
                        .into_iter()
                        .max_by_key(|(id, n)| (*n, std::cmp::Reverse(*id)))
                        .map_or(crate::systems::SystemConfig::DEFAULT_RATIO, |(id, _)| self.library.cover_ratio(id))
                }
            };
            let view = crate::view_model::SystemView::of_games(games, self.filter, self.sort, &self.favourites, &installed);
            let entry = BrowseEntry {
                games: view.games.iter().filter_map(|g| self.library.index_of(g.rom_id)).collect(),
                letters: view.letters,
                counts,
                ratio,
                key,
            };
            *self.browse_cache.borrow_mut() = Some(entry);
        }
        std::cell::Ref::map(self.browse_cache.borrow(), |entry| entry.as_ref().expect("just filled"))
    }

    /// The box-art ratio a scope's grid is laid out in: its system's, or for a collection the one
    /// most of its games share.
    pub(super) fn scope_ratio(&self, scope: &Scope) -> f32 {
        self.browse_entry(scope).ratio
    }

    /// A scope's grid geometry on the current window.
    pub(super) fn grid_spec(&self, scope: &Scope) -> crate::grid::GridSpec {
        self.grid_spec_for(self.scope_ratio(scope))
    }

    pub(super) fn grid_spec_for(&self, ratio: f32) -> crate::grid::GridSpec {
        crate::grid::grid_for(
            ratio,
            crate::grid::usable_width_for(self.content_width),
            self.settings.ui_scale,
            self.settings.grid_density,
        )
    }

    /// Which grid row a scroll offset puts at the top of the viewport, or `None` off Browse.
    ///
    /// The row window is a function of this and the row count, and scrolling cannot change the row
    /// count — so two offsets that agree here ask for exactly the same rows.
    pub(super) fn browse_row_at(&self, offset: f32) -> Option<usize> {
        let Screen::Browsing(Route::Browse { scope }) = &self.screen else { return None };
        let pitch = self.grid_spec(scope).pitch.max(1.0);
        Some((offset / pitch).floor().max(0.0) as usize)
    }

    /// The rows of the Browse grid that are worth building and worth holding art for.
    ///
    /// The window height passed in is the *window's*, not the grid's: the grid sits below the tab
    /// bar, the header, the system pills and the chips, so this always over-states its viewport.
    /// That is the safe direction — a window a little too tall builds a couple of spare rows, where
    /// one too short leaves a band of the page empty.
    pub(super) fn grid_rows(&self, spec: &crate::grid::GridSpec, row_count: usize) -> std::ops::Range<usize> {
        crate::grid::rows_in_view(
            spec,
            row_count,
            self.grid_scroll,
            self.content_height,
            GRID_MARGIN_ROWS,
        )
    }

    /// The same window as [`App::grid_rows`], as game indices — what the art fetch works in.
    pub(super) fn grid_window(&self, spec: &crate::grid::GridSpec, packing: &crate::grid::Packing) -> std::ops::Range<usize> {
        let rows = self.grid_rows(spec, packing.rows().len());
        match (packing.rows().get(rows.start), rows.end.checked_sub(1).and_then(|last| packing.rows().get(last))) {
            (Some(first), Some(last)) if !rows.is_empty() => first.start..last.end,
            _ => 0..0,
        }
    }

    /// Each game's tile width at the grid's one height: its system's cover shape, so a collection
    /// mixing systems shows every cover whole at its own width — see [`crate::grid::Packing`]. The
    /// system's shape rather than each scan's measured one, because that is known before any art
    /// arrives: sized by the art, rows would re-pack under the cursor as the covers loaded.
    pub(super) fn packing(&self, view: &crate::view_model::SystemView<'_>, spec: &crate::grid::GridSpec) -> crate::grid::Packing {
        let widths = view.games.iter().map(|g| spec.tile_h * self.library.cover_ratio(&g.system_id)).collect();
        crate::grid::Packing::pack(widths, crate::grid::usable_width_for(self.content_width))
    }

    /// A scope's grid geometry and packing, for a caller that needs no more of the view.
    pub(super) fn layout(&self, scope: &Scope) -> (crate::grid::GridSpec, crate::grid::Packing) {
        let (view, spec) = self.scope_view(scope);
        let packing = self.packing(&view, &spec);
        (spec, packing)
    }

    pub(super) fn chip_counts(&self, scope: &Scope) -> crate::ui::browse::ChipCounts {
        self.browse_entry(scope).counts
    }

    /// The page body for a route.
    ///
    /// Both borrows carry the same lifetime deliberately: the route lives inside `self.screen`, and
    /// the returned element holds `&Game`s out of the library, so tying them together is what lets
    /// a tile borrow its game rather than cloning one per frame.
    pub(super) fn browse_body<'a>(&'a self, route: &'a Route) -> Element<'a, Message> {
        match route {
            Route::Home => home_view(&self.home_context()),
            Route::Collections => crate::ui::collections::collections_view(&crate::ui::collections::CollectionsContext {
                kinds: self.all_collections.kinds(),
                cards: self.collection_cards(),
                focus: &self.collections_focus,
                covers: &self.covers,
                note: self.collections_note(),
            }),
            Route::Browse { scope } => {
                let (view, spec) = self.scope_view(scope);
                let packing = self.packing(&view, &spec);
                let rows = self.grid_rows(&spec, packing.rows().len());
                let ctx = crate::ui::browse::BrowseContext {
                    packing,
                    places: self.places(scope),
                    scope,
                    view,
                    rows,
                    focus: &self.browse_focus,
                    covers: &self.covers,
                    downloads: &self.downloads,
                    spec,
                    filter: self.filter,
                    sort: self.sort,
                    counts: self.chip_counts(scope),
                    fade: &self.fade,
                    now: std::time::Instant::now(),
                    hovered: self.hovered,
                };
                crate::ui::browse::browse_view(ctx)
            }
            Route::Detail { rom_id, from } => {
                let Some(game) = self.library.game(*rom_id) else {
                    return loading_view("That game is no longer in the library.");
                };
                // Taken off the game itself, not off a local clone: the returned element borrows
                // this name, so it has to live as long as the library does.
                let system_id: &str = &game.system_id;
                let system_name = self
                    .library
                    .systems
                    .iter()
                    .find(|s| s.id == system_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or(system_id);
                // Opened from a collection's grid, the page says where it is in that collection; from
                // a system's, where it is in the system.
                let (system_name, system_total, position) = match from.as_ref() {
                    Route::Browse { scope: scope @ Scope::Collection(key) } => match self.all_collections.get(key) {
                        Some(c) => (c.name.as_str(), c.members.len(), self.scope_view(scope).0.position_of(*rom_id)),
                        None => (system_name, self.library.games_for(system_id).len(), None),
                    },
                    Route::Browse { scope: scope @ Scope::System(system) } if system == system_id => {
                        (system_name, self.library.games_for(system_id).len(), self.scope_view(scope).0.position_of(*rom_id))
                    }
                    _ => (system_name, self.library.games_for(system_id).len(), None),
                };
                let ctx = crate::ui::detail::DetailContext {
                    game,
                    system_name,
                    system_total,
                    covers: &self.covers,
                    downloads: &self.downloads,
                    focus: &self.detail_focus,
                    host: self.host(),
                    position,
                    saves: self.game_saves.get(rom_id).and_then(|r| r.as_ref().ok()).map(Arc::as_ref),
                    achievements: self.achievements.get(rom_id),
                    in_steam: game.builds().any(|id| self.steam_bindings.get(self.host(), id).is_some()),
                    favourite: game.builds().any(|id| self.favourites.contains(&id)),
                };
                crate::ui::detail::detail_view(&ctx)
            }
            Route::Saves { rom_id, .. } => {
                let Some(game) = self.library.game(*rom_id) else {
                    return loading_view("That game is no longer in the library.");
                };
                crate::ui::saves::saves_view(&crate::ui::saves::SavesContext {
                    game,
                    saves: self.game_saves.get(rom_id),
                    focus: &self.saves_focus,
                    covers: &self.covers,
                    host: self.host(),
                })
            }
            Route::Search { .. } => crate::ui::search::search_view(
                &self.search_focus,
                &self.search_results(),
                &self.library,
                &self.downloads,
            ),
            Route::Downloads => crate::ui::downloads::downloads_view(
                self.download_rows(),
                &self.downloads_focus,
                self.downloads.total_bytes(),
                &self.settings.roms_dir(),
            ),
            Route::Settings => {
                crate::ui::settings::settings_view(&crate::ui::settings::SettingsContext {
                    focus: &self.settings_focus,
                    settings: &self.settings,
                    check: &self.connection_check,
                    save_folder: self.save_folder.as_ref(),
                    retroarch_found: self.retroarch_found.as_ref(),
                    save_sync: &self.save_sync,
                    release: &self.release,
                    appimage: self.appimage.is_some(),
                    retrodeck: self.retrodeck,
                    steam: self.steam_found.then(|| {
                        let here = self.downloads.installed_ids();
                        let host = self.host();
                        (here.iter().filter(|id| self.steam_bindings.get(host, **id).is_some()).count(), here.len())
                    }),
                    hooks: self.hooks.as_ref(),
                    image_cache: self.image_cache_bytes,
                    downloads: &self.downloads,
                    games: self.library.total_games,
                    systems: self.library.systems.len(),
                    system_emulators: self.settings.system_commands.keys().map(|system| self.system_label(system)).collect(),
                    installs: self.installs,
                })
            }
        }
    }
}

/// A bar sweeping back and forth along a track, for work with no measure of how far it has got:
/// what says the window is waiting rather than stuck. Its place is worked out from the time, so any
/// frame draws it where it should be.
fn sweep<'a>(since: std::time::Instant, now: std::time::Instant) -> Element<'a, Message> {
    const PERIOD: f32 = 1.6;
    const BAR: f32 = 0.3;
    let t = now.saturating_duration_since(since).as_secs_f32() / PERIOD;
    // Eased in and out at each end, rather than bouncing off them.
    let at = (1.0 - BAR) * (0.5 - 0.5 * (std::f32::consts::TAU * t).cos());
    let portion = |f: f32| Length::FillPortion((f * 1000.0) as u16 + 1);
    container(
        row![
            Space::new().width(portion(at)),
            container(Space::new().height(Length::Fixed(4.0))).width(portion(BAR)).style(|_| container::Style {
                background: Some(Background::Color(theme::ACCENT)),
                border: Border { radius: 2.0.into(), ..Default::default() },
                ..Default::default()
            }),
            Space::new().width(portion(1.0 - BAR - at)),
        ]
        .height(Length::Fixed(4.0)),
    )
    .width(Length::Fixed(240.0))
    .height(Length::Fixed(4.0))
    .style(|_| container::Style {
        background: Some(Background::Color(theme::alpha(theme::NEUTRAL_100, 0.12))),
        border: Border { radius: 2.0.into(), ..Default::default() },
        ..Default::default()
    })
    .into()
}
