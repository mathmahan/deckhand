//! Every message, to the method that handles it — the body of [`App::update`].

use super::*;

impl App {
    pub(super) fn dispatch(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UrlChanged(url) => {
                if let Screen::Login(state) = &mut self.screen {
                    state.url = url;
                }
                Task::none()
            }
            Message::CodeChanged(code) => {
                if let Screen::Login(state) = &mut self.screen {
                    // Cap what can be typed at the grouped form's length, so a pasted code with
                    // separators fits but a runaway paste does not.
                    if normalize_code(&code).len() <= crate::ui::login::CODE_LEN {
                        state.code = code.to_uppercase();
                    }
                }
                Task::none()
            }
            Message::InsecureToggled(on) => {
                if let Screen::Login(state) = &mut self.screen {
                    state.insecure = on;
                }
                Task::none()
            }
            Message::Connect => self.connect(),
            Message::StartQr => self.start_qr(),
            Message::QrStarted(generation, result) => {
                let Screen::Login(state) = &mut self.screen else { return Task::none() };
                if generation != state.qr_generation {
                    return Task::none();
                }
                match result {
                    Ok(pairing) => {
                        state.status = None;
                        state.qr = Some(Box::new(pairing));
                        self.poll_qr()
                    }
                    Err(why) => {
                        state.status = Some(why);
                        Task::none()
                    }
                }
            }
            Message::QrPolled(generation, poll) => {
                use crate::qr_pairing::Poll;
                let Screen::Login(state) = &mut self.screen else { return Task::none() };
                if generation != state.qr_generation || state.qr.is_none() {
                    return Task::none();
                }
                match poll {
                    Poll::Waiting => self.poll_qr(),
                    Poll::SlowDown => {
                        if let Some(qr) = &mut state.qr {
                            qr.interval += std::time::Duration::from_secs(5);
                        }
                        self.poll_qr()
                    }
                    Poll::Approved(connection) => self.update(Message::Paired(Ok(connection))),
                    Poll::Failed(why) => {
                        state.qr = None;
                        state.status = Some(why);
                        Task::none()
                    }
                }
            }
            Message::CancelQr => {
                if let Screen::Login(state) = &mut self.screen {
                    state.qr = None;
                    state.qr_generation += 1;
                    state.status = None;
                }
                Task::none()
            }
            Message::Paired(Ok(connection)) => {
                self.downloads.set_host(&connection.host);
                self.settings.connection = Some(*connection);
                if let Err(e) = self.save_settings() {
                    // Pairing worked; only the *remembering* failed. Say so rather than dropping
                    // back to the login screen — the session is live either way, and re-pairing
                    // would burn a code that is already spent.
                    self.say(format!("Paired, but the settings file could not be written: {e}"));
                }
                self.screen = Screen::Loading { status: "Loading your library…".into() };
                Task::batch([Task::done(Message::LoadLibrary), self.ensure_device()])
            }
            Message::DeviceReady(id) => {
                // Stored only for the server it was registered with: a sign-out in between would
                // otherwise hand the id to the next pairing.
                if let Some(connection) = self.settings.connection.as_mut()
                    && connection.device_id.as_deref() != Some(&id)
                {
                    connection.device_id = Some(id);
                    let _ = self.save_settings();
                }
                // A device RomM did not know until now: whatever was asked of it is waiting.
                self.installs = Installs::Unknown;
                self.claim_installs()
            }
            Message::Paired(Err(error)) => {
                if let Screen::Login(state) = &mut self.screen {
                    state.connecting = false;
                    state.status = Some(error);
                }
                Task::none()
            }
            Message::LoadLibrary => self.load_library(),
            Message::SyncLibrary => self.sync_library(),
            Message::LibraryProgress(progress) => self.on_progress(progress),
            Message::CoverLoaded(generation, url, art) => {
                if generation != self.art_generation {
                    return Task::none();
                }
                self.covers.insert(
                    url,
                    match art {
                        Some(art) => {
                            let (handle, meta) = (*art).clone();
                            Cover::Ready(handle, meta)
                        }
                        None => Cover::Failed(std::time::Instant::now()),
                    },
                );
                // The spotlight's picture it was waiting for — up it comes.
                if let Some(waiting) = self.backdrop.waiting()
                    && self.spotlight_ready(waiting)
                {
                    self.backdrop.show(waiting, std::time::Instant::now());
                }
                Task::none()
            }
            Message::OpenSystem(system) => self.open_scope(Scope::System(system)),
            Message::OpenCollection(key) => self.open_scope(Scope::Collection(key)),
            Message::CollectionsLoaded(result) => {
                match result {
                    Ok(sources) => self.collection_sources = Some(Ok(sources)),
                    // A refresh that fails leaves what is on screen, which was right until a moment
                    // ago — the copy kept from the last launch, or the last answer. With nothing to
                    // show, the page says why.
                    Err(why) if !matches!(self.collection_sources, Some(Ok(_))) => self.collection_sources = Some(Err(why)),
                    Err(_) => {}
                }
                self.rebuild_collections();
                // The cards' covers are known now.
                self.fetch_art()
            }
            Message::OwnCollectionsLoaded { edits, result } => {
                // A failure leaves the ones on screen, which the library load read a while ago.
                let Ok((own, smart)) = result else { return Task::none() };
                // A favourite pressed since the question went out, or still on its way, is ahead of
                // this answer: the favourites held here stay, and only the rest is taken.
                let behind = edits != self.favourite_edits || !self.favouriting.is_empty();
                let own: Vec<Collection> = if behind {
                    let held = self.collections.iter().filter(|c| c.is_favorite).cloned();
                    own.into_iter().filter(|c| !c.is_favorite).chain(held).collect()
                } else {
                    own
                };
                let mut changed = false;
                if own != self.collections {
                    if !behind {
                        self.favourites = favourites_of(&own);
                    }
                    self.collections = own;
                    // Home has a shelf per collection of the player's.
                    self.rebuild_shelves();
                    changed = true;
                }
                if let (Some(smart), Some(Ok((held, _)))) = (smart, &mut self.collection_sources)
                    && smart != *held
                {
                    *held = smart;
                    changed = true;
                }
                if !changed {
                    return Task::none();
                }
                self.rebuild_collections();
                self.fetch_art()
            }
            Message::CollectionsScrolled(offset) => {
                self.scrolls = self.scrolls.wrapping_add(1);
                let offset = offset.max(0.0);
                // Only a new row of cards is news for the art, as on the game grid.
                let row = |y: f32| crate::ui::collections::row_at(y);
                let same_row = row(self.collections_scroll) == row(offset);
                self.collections_scroll = offset;
                if same_row { Task::none() } else { self.fetch_art() }
            }
            Message::CollectionKindPicked(index) => {
                self.collections_focus.show(index);
                self.collections_focus.zone = crate::collections::Zone::Cards;
                Task::batch([self.scroll_page_to_top(), self.fetch_art()])
            }
            Message::OpenDetail(rom_id) => {
                let from = match &self.screen {
                    Screen::Browsing(route) => route.clone(),
                    _ => Route::Home,
                };
                // Detail opens on its primary action, which is the one you came here to press.
                self.detail_focus = Default::default();
                self.detail_offset = 0.0;
                self.screen = Screen::Browsing(Route::Detail { rom_id, from: Box::new(from) });
                Task::batch([self.scroll_page_to_top(), self.load_saves(rom_id), self.load_achievements(rom_id)])
            }
            Message::Back => {
                if let Screen::Browsing(Route::Detail { from, .. } | Route::Search { from } | Route::Saves { from, .. }) = &self.screen {
                    self.screen = Screen::Browsing((**from).clone());
                }
                Task::none()
            }
            Message::Pad(pad) => {
                use crate::gamepad::Pad;
                // A game has the pad while it runs. Without this every button pressed in the game
                // would also walk the grid behind it.
                if self.playing.is_some() {
                    return Task::none();
                }
                let message = match pad {
                    Pad::Nav(direction) => Message::Nav(direction),
                    Pad::Confirm => Message::Activate,
                    Pad::Back => Message::Dismiss,
                    Pad::Search => Message::OpenSearch,
                    Pad::Erase if matches!(self.screen, Screen::Browsing(Route::Search { .. })) => {
                        Message::SearchKey(crate::search::BACKSPACE)
                    }
                    // On Home, the spotlit game's saves and states: the X the spotlight shows beside
                    // the button.
                    Pad::Erase if matches!(self.screen, Screen::Browsing(Route::Home)) && self.chrome.is_none() => {
                        match self.focus.spotlit(&self.shelves) {
                            Some(game) => Message::OpenSaves(game.rom_id),
                            None => return Task::none(),
                        }
                    }
                    Pad::Erase => return Task::none(),
                    Pad::PrevTab => Message::CycleTab(-1),
                    Pad::NextTab => Message::CycleTab(1),
                    Pad::PrevSystem => Message::CycleSystem(-1),
                    Pad::NextSystem => Message::CycleSystem(1),
                    // Settings and Downloads in one press, wherever the cursor is — the shoulders
                    // reach them too, a few tabs along.
                    Pad::Start => Message::TabSelected(Tab::Settings),
                    Pad::Select => Message::TabSelected(Tab::Downloads),
                };
                self.update(message)
            }
            Message::OpenSaves(rom_id) => {
                let from = match &self.screen {
                    Screen::Browsing(route) => route.clone(),
                    _ => return Task::none(),
                };
                let cached = self.game_saves.get(&rom_id).and_then(|r| r.as_ref().ok()).map(Arc::as_ref);
                self.saves_focus = crate::game_saves::SavesFocus::opening(cached);
                self.screen = Screen::Browsing(Route::Saves { rom_id, from: Box::new(from) });
                // Asked again on the way in: a launch since the detail page opened may have moved it.
                self.load_saves(rom_id)
            }
            Message::AchievementsLoaded(rom_id, result) => {
                self.achievements.insert(rom_id, result);
                // The badges are known now, so the page can ask for them.
                if matches!(&self.screen, Screen::Browsing(Route::Detail { rom_id: open, .. }) if *open == rom_id) {
                    self.fetch_art()
                } else {
                    Task::none()
                }
            }
            Message::SavesLoaded(rom_id, result) => {
                let on_page = matches!(&self.screen, Screen::Browsing(Route::Saves { rom_id: open, .. }) if *open == rom_id);
                let first = !self.game_saves.contains_key(&rom_id);
                if first && on_page && let Ok(saves) = &result {
                    self.saves_focus = crate::game_saves::SavesFocus::opening(Some(saves));
                }
                self.game_saves.insert(rom_id, result);
                // The screenshots are known now, so the page can ask for them.
                if on_page { self.fetch_art() } else { Task::none() }
            }
            Message::SavesTab(tab) => {
                self.saves_focus.show(tab);
                self.scroll_cursor_into_view()
            }
            Message::SavesRowClicked(row) => {
                self.saves_focus.focus(row);
                Task::none()
            }
            Message::SteamChanged { add, games, restarted, result } => {
                match result {
                    Ok(gaps) => {
                        let host = self.host().to_string();
                        for (rom_id, appid) in &games {
                            if add {
                                self.steam_bindings.bind(&host, *rom_id, *appid);
                            } else {
                                self.steam_bindings.unbind(&host, *rom_id);
                            }
                        }
                        if self.writes_settings {
                            self.steam_bindings.save();
                        }
                        let what = match games.as_slice() {
                            [(rom_id, _)] => format!("\"{}\" is", self.title_of(*rom_id)),
                            many => format!("{} games are", many.len()),
                        };
                        let restart = if restarted { " Steam is starting again." } else { "" };
                        self.say(if add {
                            let gaps = gaps.sentence(games.len()).map(|s| format!(" {s}")).unwrap_or_default();
                            format!("{what} in Steam's library, with RomM's art, and start through Deckhand.{gaps}{restart}")
                        } else {
                            format!("{what} out of Steam's library.{restart}")
                        });
                    }
                    Err(why) => self.say(format!("Steam's library was not changed: {why}")),
                }
                Task::none()
            }
            Message::SaveRestored(tile, result) => {
                match &result {
                    Ok(report) => self.watcher.wrote(&report.written),
                    Err(_) => self.watcher.rebase(),
                }
                self.say(match &result {
                    Ok(_) => "Restored. It's the save on this device now, and RomM's newest, so your other devices take it \
                              too. What was here is in save-backups."
                        .into(),
                    Err(why) => format!("Couldn't restore it: {why}"),
                });
                // The page reads RomM again: the restored version is a new record, the newest.
                self.load_saves(tile)
            }
            Message::OpenSearch => {
                let from = match &self.screen {
                    Screen::Browsing(Route::Search { .. }) => return Task::none(),
                    Screen::Browsing(route) => route.clone(),
                    _ => return Task::none(),
                };
                // A new search each time: the last one's query is rarely the next one's.
                self.search_focus = Default::default();
                self.screen = Screen::Browsing(Route::Search { from: Box::new(from) });
                Task::none()
            }
            Message::KeyChar(key, control) => {
                if matches!(self.screen, Screen::Browsing(Route::Search { .. })) && !control {
                    for c in key.chars().flat_map(char::to_lowercase) {
                        if c.is_alphanumeric() || c == ' ' || c == crate::search::BACKSPACE {
                            self.search_focus.type_char(c);
                        }
                    }
                    return self.scroll_cursor_into_view();
                }
                match (key.as_str(), control) {
                    // Deckhand's own keyboard convenience: bracket keys step system without
                    // leaving the page, so you can walk a library system by system.
                    ("[", false) => self.update(Message::CycleSystem(-1)),
                    ("]", false) => self.update(Message::CycleSystem(1)),
                    ("/", false) | ("f", true) => self.update(Message::OpenSearch),
                    _ => Task::none(),
                }
            }
            Message::SearchKey(key) => {
                self.search_focus.type_char(key);
                Task::none()
            }
            Message::SearchResultClicked(index) => {
                self.search_focus.focus_result(index);
                self.activate()
            }
            Message::FilterSelected(filter) => {
                // Keep the game under the cursor if the new filter still contains it, so narrowing
                // to "unplayed" from a game you were looking at does not also lose your place.
                let anchor = self.focused_rom_id();
                self.filter = filter;
                self.refocus_browse(anchor)
            }
            Message::SortSelected(sort) => {
                let anchor = self.focused_rom_id();
                self.sort = sort;
                self.refocus_browse(anchor)
            }
            Message::JumpToLetter(index) => {
                let Screen::Browsing(Route::Browse { scope }) = &self.screen else {
                    return Task::none();
                };
                let (spec, packing) = self.layout(scope);
                self.browse_focus.focus_index(index, &packing);
                // A letter is a heading, so its first row goes to the top and the games under it
                // fill the page. A step of the cursor, by contrast, moves the page only as far as
                // it has to — see `scroll_cursor_into_view`.
                let y = crate::ui::browse::row_top(&spec, packing.row_of(self.browse_focus.index()));
                self.grid_scroll = y;
                Task::batch([
                    iced::widget::operation::scroll_to(
                        crate::ui::browse::grid_id(),
                        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
                    ),
                    self.fetch_art(),
                ])
            }
            Message::DetailAction(action) => {
                use crate::ui::detail::Action;
                let Screen::Browsing(Route::Detail { rom_id, .. }) = &self.screen else {
                    return Task::none();
                };
                let rom_id = *rom_id;
                let state = self.game_state(rom_id);
                let missing = self.game_of(rom_id).is_some_and(Game::missing);
                // What the button says it will do is what it does — including saying why it
                // cannot, which is the honest half of an action row this build only half has.
                if let Some(reason) = action.pending_reason(&state, missing) {
                    let title = self.title_of(rom_id);
                    self.say(format!("\"{title}\" {reason}."));
                    return Task::none();
                }
                match action {
                    Action::Download if state == crate::downloads::SyncState::UpdateAvailable => self.update_game(rom_id),
                    Action::Download if state.is_here() => self.play_game(rom_id),
                    Action::Download => self.download_game(rom_id),
                    Action::Remove => self.remove_game(rom_id),
                    Action::Pin => self.toggle_pin(rom_id),
                    Action::Steam => self.press_steam(rom_id),
                    Action::Saves => self.update(Message::OpenSaves(rom_id)),
                    Action::Favourite => self.toggle_favourite(rom_id),
                    // This returned a reason above.
                    Action::LaunchWith => Task::none(),
                }
            }
            Message::TabSelected(tab) => {
                self.chrome = None;
                let current = match &self.screen {
                    Screen::Browsing(route) => route.tab(),
                    _ => return Task::none(),
                };
                // Pressing the tab you are already on takes you back to the top of it — the same
                // thing a tab bar does everywhere else, and the fastest way back from deep in a
                // page.
                //
                // Except from a detail or search page, each a pushed destination showing under the tab
                // it was opened from: there the press has to *leave* it for that tab's own page,
                // which is what the bar looks like it is offering. Scrolling a page you are not
                // on is the one thing it cannot mean.
                // A collection's grid sits under the Collections tab the same way: pressing it goes
                // back to the page of collections.
                let on_detail = matches!(
                    self.screen,
                    Screen::Browsing(
                        Route::Detail { .. } | Route::Search { .. } | Route::Saves { .. } | Route::Browse { scope: Scope::Collection(_) }
                    )
                );
                if tab == current && !on_detail {
                    match tab {
                        Tab::Home => self.focus.reset_to_top(&self.shelves),
                        Tab::Downloads => self.downloads_focus = Default::default(),
                        Tab::Settings => self.settings_focus = Default::default(),
                        _ => {}
                    }
                    return Task::batch([self.scroll_page_to_top(), self.scroll_cursor_into_view()]);
                }
                match tab {
                    Tab::Home => {
                        self.screen = Screen::Browsing(Route::Home);
                        Task::batch([self.scroll_page_to_top(), self.scroll_cursor_into_view()])
                    }
                    // Back to the search a game was opened from, query and all; on the search
                    // itself, back up to its keyboard; anywhere else, a new search.
                    Tab::Search => {
                        fn search_behind(route: &Route) -> Option<Route> {
                            match route {
                                Route::Search { .. } => Some(route.clone()),
                                Route::Detail { from, .. } | Route::Saves { from, .. } => search_behind(from),
                                _ => None,
                            }
                        }
                        let behind = match &self.screen {
                            Screen::Browsing(route) => search_behind(route),
                            _ => None,
                        };
                        match behind {
                            Some(search) => {
                                if matches!(self.screen, Screen::Browsing(Route::Search { .. })) {
                                    self.search_focus.in_results = false;
                                }
                                self.screen = Screen::Browsing(search);
                                Task::none()
                            }
                            None => self.update(Message::OpenSearch),
                        }
                    }
                    Tab::Collections => {
                        // The page keeps its kind and card between visits: coming back from a
                        // collection's grid should land where you left, not on the first chip.
                        self.screen = Screen::Browsing(Route::Collections);
                        // Scrolled to the card it was left on, which is also what the cover window
                        // is cut from — so the art asked for is the art on screen.
                        let scroll = self.scroll_cursor_into_view();
                        Task::batch([scroll, self.load_collections(), self.refresh_own_collections(), self.fetch_art()])
                    }
                    Tab::Browse => {
                        // Browse needs a system. Carry the one you were in, else the first that has
                        // any games — an empty grid would be a dead end.
                        let system = match &self.screen {
                            Screen::Browsing(route) => self.current_system(route),
                            _ => None,
                        }
                        .or_else(|| {
                            self.library
                                .systems
                                .iter()
                                .find(|s| !self.library.games_for(&s.id).is_empty())
                                .map(|s| s.id.clone())
                        });
                        match system {
                            Some(system) => self.update(Message::OpenSystem(system)),
                            None => Task::none(),
                        }
                    }
                    Tab::Settings => {
                        self.settings_focus = Default::default();
                        self.connection_check = Check::default();
                        // A finished sync's counts carry no time either, so they go the same way. A
                        // sync still running keeps saying so: its answer has not arrived yet.
                        if !matches!(self.save_sync, SaveSync::Running) {
                            self.save_sync = SaveSync::Idle;
                        }
                        self.screen = Screen::Browsing(Route::Settings);
                        self.retrodeck = crate::launch::RetroDeck::detect().is_some();
                        self.steam_found = crate::steam::Steam::find().is_some();
                        Task::batch([
                            self.scroll_page_to_top(),
                            self.measure_image_cache(),
                            self.locate_saves(),
                            self.read_hooks(),
                        ])
                    }
                    Tab::Downloads => {
                        self.downloads_focus = Default::default();
                        self.screen = Screen::Browsing(Route::Downloads);
                        self.scroll_page_to_top()
                    }
                }
            }
            Message::Nav(direction) => self.navigate(direction),
            Message::Resized(width, height) => {
                // iced divides a window's size by every scale it applies — the system's and this
                // app's — so the size is in layout units already: what the page has to lay out in.
                // Multiplied back by the scale iced applied, it is the window in the system's own
                // pixels, which the screen is fitted against.
                let applied = self.applied_scale.get();
                let before = self.layout_scale();
                self.window = (width * applied, height * applied);
                let scale = self.layout_scale();
                self.content_width = self.window.0 / scale;
                self.content_height = self.window.1 / scale;
                // A size reported under one scale and read under another is the one way that can
                // be wrong: the screen's own resize and the size asked for at launch can cross. So
                // when the scale moves, the size is asked again — iced applies the new scale before
                // it answers — and that answer settles it.
                let confirm = if (scale - before).abs() > before * 1e-3 { self.ask_window_size() } else { Task::none() };
                // A resize changes the column count, which moves every row: the art window has to
                // be recomputed against the new geometry rather than kept from the old one. The
                // density is re-read with it because a window dragged to another monitor reports
                // the move as a resize and nothing else.
                Task::batch([
                    iced::window::latest()
                        .and_then(iced::window::scale_factor)
                        .map(Message::PanelDensity),
                    self.fetch_art(),
                    confirm,
                ])
            }
            Message::Rescaled => self.ask_window_size(),
            Message::PanelDensity(density) => {
                let density = density.max(0.1);
                if (density - self.panel_density).abs() < 0.01 {
                    return Task::none();
                }
                let denser = density > self.panel_density;
                self.panel_density = density;
                // Art decoded for a coarser panel is short of rows for this one, and would be drawn
                // soft for as long as it stayed held. Dropping it costs a re-decode off the disk
                // cache; keeping it would be a permanently blurry page. The other direction needs
                // nothing — art with pixels to spare looks the same and ages out on its own.
                if denser {
                    self.covers.drop_art();
                    self.art_generation += 1;
                }
                self.fetch_art()
            }
            Message::Scrolled => {
                self.scrolls = self.scrolls.wrapping_add(1);
                Task::none()
            }
            Message::DetailScrolled(offset) => {
                self.scrolls = self.scrolls.wrapping_add(1);
                self.detail_offset = offset.max(0.0);
                Task::none()
            }
            // The mouse in use is the page in use: the bar gives up the cursor.
            Message::PointerUsed => {
                self.chrome = None;
                Task::none()
            }
            Message::DismissBanner => {
                self.banner = None;
                if self.chrome == Some(Chrome::Banner) {
                    self.chrome = Some(Chrome::Tabs);
                }
                Task::none()
            }
            // The cursor on its Dismiss moves to the bar in `update`, as for a banner dismissed.
            Message::BannerTick => {
                self.banner_tick(std::time::Instant::now());
                Task::none()
            }
            Message::GridScrolled(offset) => {
                self.scrolls = self.scrolls.wrapping_add(1);
                let offset = offset.max(0.0);
                // A scroll only changes what is worth fetching when it crosses a row boundary, and
                // it takes several events to cross one. Without this the art pipeline would be
                // re-entered on every scrolled pixel — each one re-deriving the view and spawning a
                // task — to discover it had nothing new to ask for.
                let same_row = self.browse_row_at(self.grid_scroll) == self.browse_row_at(offset);
                self.grid_scroll = offset;
                if same_row {
                    return Task::none();
                }
                self.fetch_art()
            }
            // Nothing to do: arriving at all is the point. iced rebuilds the view after every
            // message, and the view is what reads the fade's current position.
            Message::Redraw => {
                if let Some(log) = &mut self.frame_log {
                    log.tick(std::time::Instant::now());
                }
                Task::none()
            }
            Message::CycleTab(delta) => {
                let Screen::Browsing(route) = &self.screen else { return Task::none() };
                // Every destination, in the bar's order, wrapping — so the shoulders reach Downloads
                // and Settings like everything else.
                let here = Tab::ALL.iter().position(|t| *t == route.tab()).unwrap_or(0);
                let next = (here as isize + delta).rem_euclid(Tab::ALL.len() as isize) as usize;
                self.update(Message::TabSelected(Tab::ALL[next]))
            }
            Message::CycleSystem(delta) => {
                let Screen::Browsing(route) = &self.screen else { return Task::none() };
                // Settings has no system to step; the same keys walk its sections instead, which is
                // what the Kotlin app's shoulder buttons do there.
                if *route == Route::Settings {
                    if self.settings_focus.jump_section(delta) {
                        return self.scroll_cursor_into_view();
                    }
                    return Task::none();
                }
                // A collection's grid steps through its kind's collections; anything else through
                // the systems, from the one it is showing.
                let scope = match route {
                    Route::Browse { scope } => scope.clone(),
                    route => match self.current_system(route) {
                        Some(system) => Scope::System(system),
                        None => return Task::none(),
                    },
                };
                let places = self.places(&scope);
                let Some(here) = places.iter().position(|p| p.scope == scope) else { return Task::none() };
                let next = (here as isize + delta).rem_euclid(places.len() as isize) as usize;
                self.open_scope(places[next].scope.clone())
            }
            // Not a reason to take the cursor out of the bar: a card reports the pointer entering it
            // whenever the page moves under a pointer that is resting there — see `PointerUsed`.
            Message::Hovered(card) => {
                self.hovered = Some(card);
                Task::none()
            }
            // Only if it is still the card being left: moving straight from one card to the next
            // can deliver the new card's enter before the old one's exit.
            Message::Unhovered(card) => {
                if self.hovered == Some(card) {
                    self.hovered = None;
                }
                Task::none()
            }
            Message::FavouriteChanged { tile, adding, ids, result } => {
                self.favouriting.remove(&tile);
                // A list of collections asked for while this was on its way may predate it.
                self.favourite_edits += 1;
                match result {
                    // RomM's collection as it now is: whatever else changed in it meanwhile, from
                    // another device or the web app, comes with it.
                    Ok(mut collection) => {
                        collection.is_favorite = true;
                        match self.collections.iter_mut().find(|c| c.id == collection.id) {
                            Some(held) => *held = collection,
                            None => self.collections.push(collection),
                        }
                        self.favourites = favourites_of(&self.collections);
                    }
                    Err(why) => {
                        for id in ids {
                            if adding {
                                self.favourites.remove(&id);
                            } else {
                                self.favourites.insert(id);
                            }
                        }
                        let title = self.title_of(tile);
                        self.say(format!("\"{title}\" could not be {} favourites: {why}", if adding { "added to" } else { "taken out of" }));
                    }
                }
                self.rebuild_collections();
                Task::none()
            }
            Message::HomeCardClicked(shelf, card) => {
                if !matches!(self.screen, Screen::Browsing(Route::Home)) {
                    return Task::none();
                }
                self.chrome = None;
                // A second click on the card the spotlight is on presses it, as Confirm would; the
                // first only puts the spotlight there, which is what a click on a shelf is for.
                if self.focus.is_focused(&self.shelves, shelf, card) {
                    return self.activate();
                }
                self.focus.focus_card(&self.shelves, shelf, card);
                self.scroll_cursor_into_view()
            }
            Message::HomeWheel(delta) => {
                use iced::mouse::ScrollDelta;
                if !matches!(self.screen, Screen::Browsing(Route::Home)) {
                    return Task::none();
                }
                // A notch is a shelf. A touchpad's pixels are saved up until they come to one.
                let steps = match delta {
                    ScrollDelta::Lines { y, .. } => y.round(),
                    ScrollDelta::Pixels { y, .. } => {
                        self.wheel += y;
                        let steps = (self.wheel / 60.0).trunc();
                        self.wheel -= steps * 60.0;
                        steps
                    }
                };
                // Up the wheel is up the page. It walks the shelves only: it does not climb out of
                // them onto the spotlight's buttons, or past those into the bar.
                let direction = if steps > 0.0 { Direction::Up } else { Direction::Down };
                let mut tasks = Vec::new();
                for _ in 0..(steps.abs() as usize).min(3) {
                    if direction == Direction::Up && (self.focus.shelf_pos() == 0 || self.focus.zone() != crate::focus::Zone::Shelves) {
                        break;
                    }
                    tasks.push(self.navigate_page(Route::Home, direction));
                }
                self.up_stopped = false;
                Task::batch(tasks)
            }
            // The spotlight's heart is the detail page's Favourite.
            Message::FavouritePressed(rom_id) => self.toggle_favourite(rom_id),
            Message::AwardedLoaded(result) => {
                // A record that cannot be read leaves the spotlight saying only how many there are.
                if let Ok(awarded) = result {
                    self.awarded = awarded;
                }
                Task::none()
            }
            Message::Activate => self.activate(),
            Message::GameActivated(rom_id) => {
                let state = self.game_state(rom_id);
                if state.is_here() {
                    self.play_game(rom_id)
                } else if state.is_busy() {
                    // The button is inert while a download runs; this can only be a stale press.
                    Task::none()
                } else {
                    self.download_game(rom_id)
                }
            }
            Message::Dismiss => {
                // Back from the tab bar is back down into the page.
                if self.chrome.take().is_some() {
                    return Task::none();
                }
                // Escape means "get me out of here" before it means "hide that message": from a
                // detail page it goes back, and only dismisses the banner once there is nowhere
                // left to go.
                if matches!(self.screen, Screen::Browsing(Route::Detail { .. } | Route::Search { .. } | Route::Saves { .. })) {
                    return self.update(Message::Back);
                }
                // A collection's grid was opened from the Collections page, and that is where back is.
                if matches!(self.screen, Screen::Browsing(Route::Browse { scope: Scope::Collection(_) })) {
                    return self.update(Message::TabSelected(Tab::Collections));
                }
                self.banner = None;
                Task::none()
            }
            Message::SignOut => {
                // Local only: the token is dropped here, not revoked on the server. Revoking is a
                // remote, irreversible act that needs its own confirmation, and this build has no
                // screen to host that conversation.
                self.settings.connection = None;
                // The downloads stop being this session's: every queue item and every record is
                // keyed to the server it belongs to, so they are hidden rather than thrown away —
                // the files are real, and signing back in has to find them again.
                self.downloads.set_host("");
                let _ = self.save_settings();
                // The cached library belongs to the pairing being dropped. Leaving it would hand
                // the next sign-in someone else's shelves for the frame before the sync lands.
                cache::clear();
                self.system_counts.clear();
                self.library_read_at = None;
                self.installs = Installs::Unknown;
                self.install_requests.clear();
                self.library = Arc::new(Library::default());
                self.collections.clear();
                self.collection_sources = None;
                self.rebuild_collections();
                self.shelves.clear();
                self.covers = Covers::default();
                self.backdrop = Default::default();
                self.awarded = None;
                self.say("Signed out locally. The token is still valid in RomM — revoke it there. Downloaded games are left on disk.".into());
                self.screen = Screen::Login(LoginState::new("https://".into()));
                Task::none()
            }
            Message::SettingsClicked(index) => {
                // No scroll: the row is already under the pointer, and moving the page would slide
                // it out from there.
                self.settings_focus.focus(index);
                self.activate()
            }
            Message::SettingsPick(setting, option) => self.pick_setting(setting, option),
            Message::ConnectionTested(result) => {
                self.connection_check = match result {
                    Ok(user) => Check::Reachable(user),
                    Err(error) => Check::Failed(error),
                };
                Task::none()
            }
            Message::ImageCacheMeasured(bytes) => {
                self.image_cache_bytes = Some(bytes);
                Task::none()
            }
            Message::SaveFolderFound(config, folder) => {
                self.retroarch_found = Some(config);
                self.save_folder = Some(folder);
                Task::none()
            }
            Message::HooksRead(status) => {
                self.hooks = Some(status);
                Task::none()
            }
            Message::HooksChanged(result, status) => {
                self.say(match result {
                    Ok(steps) if steps.is_empty() => "ES-DE's hooks were not installed.".into(),
                    Ok(steps) => format!("ES-DE hooks: {}.", steps.join("; ")),
                    Err(why) => format!("ES-DE hooks: {why}."),
                });
                self.hooks = Some(status);
                Task::none()
            }
            Message::Download(rom_id) => self.download_game(rom_id),
            Message::DownloadChoices(tile, result) => self.offer_choices(tile, result),
            Message::ClaimInstalls => self.claim_installs(),
            Message::InstallsClaimed(result) => self.on_installs_claimed(result),
            Message::InstallDetail(request, result) => self.on_install_detail(request, result),
            // Answered in `update` while a picker or an entry waits; one arriving with none is stale.
            Message::Picked(_) | Message::EntryKey(_) | Message::Pasted(_) => Task::none(),
            Message::Evicted(title, result) => {
                if let Err(why) = result {
                    self.say(format!("\"{title}\" was taken off the list, but its files could not all be removed: {why}"));
                }
                Task::none()
            }
            Message::LocationSet(result) => {
                match result {
                    Ok(path) => {
                        let default = Settings::default_roms_dir();
                        self.settings.roms_dir = (path != default).then(|| path.clone());
                        self.write_settings();
                        self.say(format!(
                            "New downloads go to {} — games already downloaded stay where they are.",
                            path.display()
                        ));
                        // A folder with games in it already: found and recorded.
                        return self.detect_games();
                    }
                    Err(why) => self.say(why),
                }
                Task::none()
            }
            Message::DownloadEvent(event) => self.on_download_event(event),
            Message::RemoveDownload(rom_id) => self.remove_download(rom_id),
            Message::DownloadRemoved(title, result) => {
                self.say(match result {
                    Ok(0) => format!("\"{title}\" was already gone from this device."),
                    Ok(_) => format!("\"{title}\" removed from this device — press Download to get it back."),
                    Err(why) => format!("\"{title}\" could not be removed: {why}"),
                });
                Task::none()
            }
            Message::LaunchEvent(event) => self.on_launch_event(event),
            // Answered in `update`, before dispatch; one arriving with nothing waiting is stale.
            Message::ConflictChosen(_) => Task::none(),
            Message::Readied(rom_id, notes) => {
                // Said only when it did something: a pull that found no save is the common case.
                if !notes.is_empty() {
                    let title = self.title_of(rom_id);
                    let size = self.downloads.installed(rom_id).map(|i| downloads::human_bytes(i.bytes));
                    self.say(match size {
                        Some(size) => format!("\"{title}\" is on this device — {size}; {}.", notes.join("; ")),
                        None => format!("\"{title}\": {}.", notes.join("; ")),
                    });
                }
                Task::none()
            }
            Message::DownloadsClicked(index) => {
                // No scroll: the row is already under the pointer, and moving the page would slide
                // it out from there.
                self.downloads_focus.focus(index);
                self.activate()
            }
            Message::RemoveFocusedDownload => {
                // Bound app-wide, because the subscription cannot express "only on this page" —
                // so this is where it has to be. Delete anywhere else must take nothing away.
                if !matches!(self.screen, Screen::Browsing(Route::Downloads)) {
                    return Task::none();
                }
                let rows = self.download_rows();
                let Some(rom_id) =
                    rows.get(self.downloads_focus.index(rows.len())).map(|e| e.rom_id)
                else {
                    return Task::none();
                };
                self.remove_download(rom_id)
            }
            Message::SavesSynced(result) => {
                let failed = result.is_err();
                self.save_sync = match result {
                    Ok(report) => {
                        // What the sync wrote is where things stand now, not a change to sync again.
                        // Only what it wrote: a game running outside Deckhand may have written its
                        // own save while this ran, and that is still news.
                        self.watcher.wrote(&report.written);
                        self.save_folder = Some(Ok(report.folder.clone()));
                        SaveSync::Done(Box::new(report))
                    }
                    Err(why) => {
                        // Nothing came back to say what it wrote, so the next look starts afresh.
                        self.watcher.rebase();
                        SaveSync::Failed(why)
                    }
                };
                // A sync RomM turned away because it no longer knows this device registers it again
                // here — see [`crate::sync::device`]. Without this the window would go on failing the
                // same way until it was restarted, since it only registers as it opens.
                if failed { self.ensure_device() } else { Task::none() }
            }
            Message::CheckRelease => self.check_release(),
            Message::GamesFound(found) => {
                let added = self.downloads.adopt(found);
                if added > 0 {
                    self.match_downloads();
                    self.rebuild_shelves();
                    if self.banner.is_none() {
                        self.say(match added {
                            1 => "Found a game already in the downloads folder: it's marked as on this device.".into(),
                            n => format!("Found {n} games already in the downloads folder: they're marked as on this device."),
                        });
                    }
                }
                Task::none()
            }
            Message::WatchTick => {
                if self.looking || self.playing.is_some() || matches!(self.save_sync, SaveSync::Running) {
                    return Task::none();
                }
                self.looking = true;
                // Which folders to look over is worked out from RetroArch's config and the download
                // records — read off the disk, so kept for a minute rather than read every tick, and
                // read again sooner when the config chosen or the games here change.
                let fingerprint = (self.settings.retroarch_config.clone(), self.downloads.installed_fingerprint());
                let known = self
                    .watch_folders
                    .as_ref()
                    .filter(|(_, at, of)| at.elapsed() < WATCH_FOLDERS_FOR && *of == fingerprint)
                    .map(|(folders, _, _)| folders.clone());
                let (chosen, store) = (self.settings.retroarch_config.clone(), Settings::config_dir());
                Task::perform(
                    async move {
                        off_thread(move || {
                            let found = known.is_none().then(|| {
                                crate::sync::retroarch::locate(chosen.as_deref())
                                    .map(|install| install.with_content_dirs(crate::downloads::content_dirs(&store)).watched())
                                    .unwrap_or_default()
                            });
                            let seen = crate::watch::snapshot(known.as_deref().or(found.as_deref()).unwrap_or_default());
                            (found.map(|folders| (folders, fingerprint)), seen)
                        })
                        .await
                        .unwrap_or_default()
                    },
                    |(found, seen)| Message::WatchScanned(found, seen),
                )
            }
            Message::WatchScanned(found, seen) => {
                self.looking = false;
                if let Some((folders, fingerprint)) = found {
                    self.watch_folders = Some((folders, std::time::Instant::now(), fingerprint));
                }
                let settled = self.watcher.observe(seen, std::time::Instant::now());
                if !settled.is_empty() && self.playing.is_none() && !self.library.is_empty() {
                    return self.sync_saves_of(Some(settled));
                }
                Task::none()
            }
            Message::ReleaseChecked(result) => self.on_release_checked(result),
            Message::ReleaseEvent(event) => self.on_release_event(event),
        }
    }
}
