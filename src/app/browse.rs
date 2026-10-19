//! Collections, and what Browse shows.

use super::*;

impl App {
    /// Open a system's or a collection's grid.
    pub(super) fn open_scope(&mut self, scope: Scope) -> Task<Message> {
        // Entering puts the cursor on the first tile rather than wherever the last grid's cursor
        // happened to be — a position in one set of games means nothing in another.
        self.browse_focus = Default::default();
        // The list's cursor rests on the open place, so stepping into the list starts from where
        // you are — and the list scrolls to show it, whichever way you got here.
        let index = self.places(&scope).iter().position(|p| p.scope == scope).unwrap_or(0);
        self.browse_focus.system = index;
        self.screen = Screen::Browsing(Route::Browse { scope });
        Task::batch([self.scroll_page_to_top(), reveal_system_row(index)])
    }

    /// The list down Browse's left edge for a scope: every system beside a system's grid; beside a
    /// collection's, the other collections of its kind.
    pub(super) fn places(&self, scope: &Scope) -> Vec<crate::ui::browse::Place> {
        use crate::ui::browse::Place;
        match scope {
            Scope::System(_) => self
                .library
                .systems
                .iter()
                .map(|s| Place {
                    scope: Scope::System(s.id.clone()),
                    name: s.name.clone(),
                    count: self.library.games_for(&s.id).len(),
                    logo: Some(self.platform_logo(&s.id)),
                })
                .collect(),
            Scope::Collection(key) => match self.all_collections.get(key) {
                Some(open) => self
                    .all_collections
                    .of_kind(open.kind)
                    .map(|c| Place { scope: Scope::Collection(c.key.clone()), name: c.name.clone(), count: c.members.len(), logo: None })
                    .collect(),
                None => Vec::new(),
            },
        }
    }

    /// RomM's icon for a platform — the one its own interface shows beside the name.
    pub(super) fn platform_logo(&self, slug: &str) -> String {
        self.asset_url(&format!("/assets/platforms/{slug}.ico"))
    }

    /// The Collections page's current kind's cards: its collections, or under favourites the games.
    pub(super) fn collection_cards(&self) -> Vec<crate::ui::collections::Card<'_>> {
        use crate::collections::Kind;
        use crate::ui::collections::Card;
        match self.all_collections.kinds().get(self.collections_focus.kind) {
            Some(Kind::Favourites) => self
                .all_collections
                .favourites()
                .iter()
                .filter_map(|id| self.library.game(*id))
                .map(|game| Card {
                    name: &game.title,
                    note: self.library.system_name(&game.system_id).to_string(),
                    cover_url: game.cover_url.as_deref(),
                    open: Message::OpenDetail(game.rom_id),
                })
                .collect(),
            Some(kind) => self
                .all_collections
                .of_kind(*kind)
                .map(|c| Card {
                    name: &c.name,
                    note: crate::ui::collections::games(c.members.len()),
                    cover_url: c.cover_url.as_deref(),
                    open: Message::OpenCollection(c.key.clone()),
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// What the Collections page says under its heading about the smart and generated collections.
    pub(super) fn collections_note(&self) -> Option<String> {
        match (&self.collection_sources, &self.settings.connection) {
            (_, None) => None,
            (None, Some(_)) => Some("Asking RomM for its smart and generated collections…".into()),
            (Some(Err(why)), Some(_)) => Some(format!("Couldn't read RomM's smart and generated collections: {why}")),
            (Some(Ok(_)), Some(_)) => None,
        }
    }

    /// Resolve every collection against the library again — after it, a collection, or a favourite
    /// changed.
    pub(super) fn rebuild_collections(&mut self) {
        let showing = self.all_collections.kinds().get(self.collections_focus.kind).copied();
        let built = {
            let (smart, generated) = match &self.collection_sources {
                Some(Ok((smart, generated))) => (smart.as_slice(), generated.as_slice()),
                _ => (&[][..], &[][..]),
            };
            crate::collections::Collections::build(&self.library, &self.favourites, &self.collections, smart, generated)
        };
        self.all_collections = built;
        // The cursor stays on its kind, which moves along when favourites come or go ahead of it. A
        // kind that is gone takes it to the one now in its place, or the last there is.
        let kinds = self.all_collections.kinds();
        match showing.and_then(|kind| kinds.iter().position(|k| *k == kind)) {
            Some(index) => {
                self.collections_focus.kind = index;
                let cards = self.collection_cards().len();
                self.collections_focus.cards.clamp(cards);
            }
            None => self.collections_focus.show(self.collections_focus.kind.min(kinds.len().saturating_sub(1))),
        }
    }

    /// Ask RomM for the player's own collections again — see [`crate::collections::load_own`].
    pub(super) fn refresh_own_collections(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let (client, edits) = (self.client.clone(), self.favourite_edits);
        Task::perform(crate::collections::load_own(client, connection), move |result| Message::OwnCollectionsLoaded { edits, result })
    }

    /// Ask RomM for its smart and generated collections, unless it has already answered. The
    /// player's own come with the library.
    pub(super) fn load_collections(&self) -> Task<Message> {
        if matches!(self.collection_sources, Some(Ok(_))) {
            return Task::none();
        }
        self.fetch_collections()
    }

    /// Ask RomM for its smart and generated collections, whatever it said last time — and keep the
    /// answer for the next launch, which opens the Collections page on it while asking again.
    pub(super) fn fetch_collections(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let client = self.client.clone();
        Task::perform(
            async move {
                let host = connection.host.clone();
                let sources = crate::collections::load(client, connection).await?;
                // A megabyte of JSON on the reference server: written off the UI thread, and handed
                // back once it is.
                off_thread(move || {
                    cache::save_sources(&host, &sources.0, &sources.1);
                    sources
                })
                .await
            },
            Message::CollectionsLoaded,
        )
    }

    /// Ask RomM for a game's saves and states — its own rom's and every sibling build's.
    pub(super) fn load_saves(&self, rom_id: i64) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            // Said rather than left loading forever — fixture mode has no server to ask.
            return Task::done(Message::SavesLoaded(rom_id, Err("not paired with a RomM server".into())));
        };
        let Some(game) = self.library.game(rom_id) else { return Task::none() };
        let mut rom_ids: Vec<i64> = std::iter::once(rom_id).chain(game.file_stems.iter().map(|(id, _)| *id)).collect();
        rom_ids.sort_unstable();
        rom_ids.dedup();
        // The build on this device, the tile's own first: its core decides which states land in a slot.
        let installed = std::iter::once(rom_id).chain(rom_ids.iter().copied()).find_map(|id| self.downloads.installed(id)).cloned();
        let (client, settings) = (self.client.clone(), self.settings.clone());
        Task::perform(
            async move {
                let core = match installed {
                    Some(installed) => off_thread(move || crate::sync::states::Core::for_game(&settings, &installed)).await.unwrap_or_default(),
                    None => crate::sync::states::Core::default(),
                };
                crate::game_saves::load(client, connection, rom_ids, Settings::config_dir(), core).await
            },
            move |result| Message::SavesLoaded(rom_id, result.map(Arc::new)),
        )
    }

    /// Ask RomM for a game's RetroAchievements list, and for which of them this user has earned —
    /// each time the page opens, since earning is what happens between two visits. Only for a game
    /// that has any.
    pub(super) fn load_achievements(&self, rom_id: i64) -> Task<Message> {
        let Some(game) = self.library.game(rom_id).filter(|g| g.achievements > 0) else {
            return Task::none();
        };
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let (client, ra_id) = (self.client.clone(), game.ra_id);
        Task::perform(
            async move {
                // Both at once: the list is the page's, the user's record only marks it up — so a
                // record that cannot be read leaves the list as it is.
                let (detail, user) = futures::join!(client.fetch_rom_detail(&connection, rom_id), client.fetch_current_user(&connection));
                let detail = detail.map_err(|e| e.to_string())?;
                let list = detail.merged_ra_metadata.map(|ra| ra.achievements).unwrap_or_default();
                Ok(Arc::new(crate::achievements::GameAchievements::new(list, user.ok().as_ref(), ra_id)))
            },
            move |result| Message::AchievementsLoaded(rom_id, result),
        )
    }

    /// Make a game a favourite in RomM, or not: in its favourites collection when it is out, every
    /// build of it out when it is in — RomM's web app can favourite a build the tile stands for.
    ///
    /// Shown at once and sent behind it, as RomM's own heart does: a press that waits on the server
    /// reads as a press that did nothing. RomM's answer is the collection as it then is, which is
    /// taken as the truth; a refusal puts the game back the way it was and says why. An account with
    /// no favourites collection yet gets one, as RomM's web app makes it.
    pub(super) fn toggle_favourite(&mut self, tile: i64) -> Task<Message> {
        if self.favouriting.contains(&tile) {
            return Task::none();
        }
        let builds = self.builds_of(tile);
        let adding = !builds.iter().any(|id| self.favourites.contains(id));
        let ids: Vec<i64> = if adding { vec![tile] } else { builds.into_iter().filter(|id| self.favourites.contains(id)).collect() };
        for id in &ids {
            if adding {
                self.favourites.insert(*id);
            } else {
                self.favourites.remove(id);
            }
        }
        self.favourite_edits += 1;
        self.rebuild_collections();
        // Fixture mode: nobody to tell.
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        self.favouriting.insert(tile);
        let collection = self.collections.iter().find(|c| c.is_favorite).map(|c| c.id);
        let client = self.client.clone();
        let sent = ids.clone();
        // A device paired by QR before favourites came to this app was never granted the right to
        // change collections, and RomM's 403 alone reads as a revoked token.
        let why = |e: ApiError| match e.status {
            403 => "RomM has not let this device change collections. Sign out and pair it again to favourite from here.".to_string(),
            _ => e.to_string(),
        };
        Task::perform(
            async move {
                let collection = match collection {
                    Some(id) => id,
                    None => client.create_favourites(&connection).await.map_err(why)?.id,
                };
                let answer = if adding {
                    client.add_to_collection(&connection, collection, &sent).await
                } else {
                    client.remove_from_collection(&connection, collection, &sent).await
                };
                answer.map_err(why)
            },
            move |result| Message::FavouriteChanged { tile, adding, ids: ids.clone(), result },
        )
    }

    /// What this user has earned of every game's achievements, for the spotlight: one read of their
    /// record in RomM, which carries the progress of the RetroAchievements account linked to it.
    /// None for a user with no account linked — there is nothing to earn with.
    pub(super) fn load_awarded(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let client = self.client.clone();
        Task::perform(
            async move {
                let user = client.fetch_current_user(&connection).await.map_err(|e| e.to_string())?;
                let linked = user.ra_username.as_deref().is_some_and(|name| !name.trim().is_empty());
                Ok(linked.then(|| {
                    user.ra_progression
                        .map(|progress| progress.results)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|game| Some((game.rom_ra_id?, usize::try_from(game.num_awarded?).ok()?)))
                        .collect()
                }))
            },
            Message::AwardedLoaded,
        )
    }

    /// Add to Steam on a game's page: in when it is out, out when it is in.
    pub(super) fn press_steam(&mut self, tile: i64) -> Task<Message> {
        let builds = self.builds_of(tile);
        let host = self.host().to_string();
        let added: Vec<i64> = builds.iter().copied().filter(|id| self.steam_bindings.get(&host, *id).is_some()).collect();
        if !added.is_empty() {
            return self.ask_steam(added, false);
        }
        // The build to start: the one on this device — the first, when several are.
        let Some(build) = self.downloads.installed_builds(builds.iter().copied()).first().map(|i| i.rom_id) else {
            return Task::none();
        };
        self.ask_steam(vec![build], true)
    }

    /// Every downloaded game into Steam at once — one restart of Steam for all of them. Those in it
    /// already go in again, which brings their art up to date.
    pub(super) fn add_all_to_steam(&mut self) -> Task<Message> {
        let mut games: Vec<i64> = self.downloads.installed_ids().into_iter().collect();
        if games.is_empty() {
            self.say("Nothing is downloaded yet, so there is nothing to add to Steam.".into());
            return Task::none();
        }
        games.sort_unstable();
        self.ask_steam(games, true)
    }

    /// Change Steam's library — first asking to close Steam, when it is running, since it rewrites
    /// its shortcuts as it closes and would undo the change otherwise. Never in Game Mode, where
    /// closing Steam ends the session this runs in.
    pub(super) fn ask_steam(&mut self, games: Vec<i64>, add: bool) -> Task<Message> {
        if crate::steam::Steam::find().is_none() {
            self.say("No Steam found on this device.".into());
            return Task::none();
        }
        if crate::steam::game_mode() {
            self.say(
                "Steam can't take a change to its library while Game Mode runs it. Do it from Desktop Mode, \
                 and it's in Game Mode from then on."
                    .into(),
            );
            return Task::none();
        }
        if !crate::steam::running() {
            return self.change_steam(&games, add, false);
        }
        let what = match games.as_slice() {
            [rom_id] => format!("\"{}\"", self.title_of(*rom_id)),
            many => format!("{} games", many.len()),
        };
        let row = |label: &str, detail: &str| crate::ui::picker::Row { label: label.into(), detail: detail.into(), aside: String::new(), mark: None };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: if !add {
                    format!("Take {what} out of Steam?")
                } else if games.iter().all(|id| self.steam_bindings.get(self.host(), *id).is_some()) {
                    format!("Refresh {what} in Steam?")
                } else {
                    format!("Add {what} to Steam?")
                },
                note: "Steam keeps its library in memory and writes it out as it closes, so it has to close for this — \
                       it opens again straight after."
                    .into(),
                rows: vec![row("Close Steam and go on", "It opens again by itself"), row("Not now", "Steam stays as it is")],
                cursor: 0,
            },
            then: Then::Steam { games, add },
        });
        Task::none()
    }

    /// Put `games` into Steam's library with their art, or take them out — closing Steam first and
    /// opening it again after, when `restart`.
    pub(super) fn change_steam(&mut self, games: &[i64], add: bool, restart: bool) -> Task<Message> {
        let Some(steam) = crate::steam::Steam::find() else { return Task::none() };
        let host = self.host().to_string();
        if !add {
            let ids: Vec<(i64, u32)> = games.iter().filter_map(|id| Some((*id, self.steam_bindings.get(&host, *id)?))).collect();
            self.say("Taking it out of Steam…".into());
            return Task::perform(
                async move {
                    let appids: Vec<u32> = ids.iter().map(|(_, a)| *a).collect();
                    let result = off_thread(move || {
                        if restart {
                            steam.shut_down()?;
                        }
                        let result = crate::steam::remove(&steam, &appids);
                        if restart {
                            steam.start();
                        }
                        result.map(|()| crate::steam::Gaps::default())
                    })
                    .await
                    .and_then(|done| done);
                    (ids, result)
                },
                move |(games, result)| Message::SteamChanged { add: false, games, restarted: restart, result },
            );
        }
        let Some(deckhand) = crate::steam::deckhand_path() else {
            self.say("Deckhand could not tell where it runs from, so there is nothing for Steam to start.".into());
            return Task::none();
        };
        // What each shortcut is, and where its art is — read here, fetched off the UI thread.
        let wanted: Vec<(i64, crate::steam::Shortcut, [Option<String>; 4])> = games
            .iter()
            .filter_map(|id| {
                let installed = self.downloads.installed(*id)?;
                let game = self.game_of(*id);
                let system = self.library.system_name(&installed.system_id);
                let art = [
                    game.and_then(|g| g.cover_url.clone()),
                    game.and_then(|g| g.marquee_path.as_deref()).map(|p| self.asset_url(p)),
                    game.and_then(|g| g.logo_path.as_deref()).map(|p| self.asset_url(p)),
                    game.and_then(|g| g.screenshots.first().cloned()),
                ];
                Some((*id, crate::steam::Shortcut::for_game(&deckhand, *id, &installed.title, system), art))
            })
            .collect();
        // The ids these games are in Steam under now — the same as they are about to get, unless a
        // title changed or Deckhand moved, in which case the old shortcut goes.
        let stale: Vec<u32> = wanted.iter().filter_map(|(id, ..)| self.steam_bindings.get(&host, *id)).collect();
        let (client, connection) = (self.client.clone(), self.settings.connection.clone());
        self.say("Adding to Steam…".into());
        Task::perform(
            async move {
                let mut games = Vec::new();
                let mut ids = Vec::new();
                for (rom_id, shortcut, [cover, banner, logo, screenshot]) in wanted {
                    let fetch = |url: Option<String>| {
                        let (client, connection) = (client.clone(), connection.clone());
                        async move {
                            let (url, connection) = (url?, connection?);
                            client.fetch_asset(&connection, &url).await.ok()
                        }
                    };
                    let art = crate::steam::Art {
                        cover: fetch(cover).await,
                        banner: fetch(banner).await,
                        logo: fetch(logo).await,
                        screenshot: fetch(screenshot).await,
                    };
                    ids.push((rom_id, shortcut.appid()));
                    games.push((shortcut, art));
                }
                let result = off_thread(move || {
                    if restart {
                        steam.shut_down()?;
                    }
                    let result = crate::steam::add(&steam, &games, &stale);
                    if restart {
                        steam.start();
                    }
                    result
                })
                .await
                .and_then(|done| done);
                (ids, result)
            },
            move |(games, result)| Message::SteamChanged { add: true, games, restarted: restart, result },
        )
    }

    /// A or Enter on a row of the Saves page. An in-game save that is not the version on this device
    /// offers to be put back. A state in a slot says which; one in none offers to go into one.
    pub(super) fn press_save(&mut self, tile: i64) -> Task<Message> {
        use crate::game_saves::Tab;
        let Some(Ok(saves)) = self.game_saves.get(&tile) else { return Task::none() };
        let list = saves.list(self.saves_focus.tab);
        let Some(record) = list.get(self.saves_focus.row(list.len())).cloned() else { return Task::none() };
        if self.saves_focus.tab == Tab::States {
            return self.press_state(tile, record);
        }
        if saves.on_device.contains(&record.id) {
            self.say("This version is the one on this device already.".into());
            return Task::none();
        }
        let build = record.rom_id.unwrap_or(tile);
        if self.downloads.installed(build).is_none() {
            self.say(format!(
                "\"{}\" isn't on this device. Restoring puts the save where its emulator reads it, so download it first.",
                self.title_of(build)
            ));
            return Task::none();
        }
        let when = record
            .updated_at
            .as_deref()
            .map(|t| t.replace('T', " ").chars().take(16).collect::<String>())
            .unwrap_or_else(|| "an unknown time".into());
        let row = |label: &str, detail: &str| crate::ui::picker::Row { label: label.into(), detail: detail.into(), aside: String::new(), mark: None };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Restore this save?".into(),
                note: format!(
                    "{}, from {when}. The save on this device goes to save-backups first; then this version takes its place \
                     and goes to RomM as the newest, so your other devices take it too.",
                    record.file_name
                ),
                rows: vec![
                    row("Restore it", "Play on from this version"),
                    row("Keep the one here", "Change nothing"),
                ],
                // The safe answer first under the cursor: this replaces progress.
                cursor: 1,
            },
            then: Then::Restore { tile, version: Box::new(record) },
        });
        Task::none()
    }

    /// A or Enter on a state of the Saves page: where it is, when it is in a slot here, and otherwise
    /// the offer to put it into one — when this device can load it.
    fn press_state(&mut self, tile: i64, record: crate::romm::models::SaveAsset) -> Task<Message> {
        let Some(Ok(saves)) = self.game_saves.get(&tile) else { return Task::none() };
        if let Some(slot) = saves.slots.get(&record.id) {
            let slot = slot.in_words();
            self.say(if saves.on_device.contains(&record.id) {
                format!("This state is in {slot}: load it from RetroArch's Quick Menu while the game runs.")
            } else {
                format!("This is {slot}'s state on RomM. It comes down into that slot with the next sync.")
            });
            return Task::none();
        }
        if !record.file_name.to_lowercase().ends_with(".state") {
            self.say("This emulator's states come down with a sync, under their own names, where it reads them.".into());
            return Task::none();
        }
        if saves.other_core.contains(&record.id) {
            let core = saves.core.clone().unwrap_or_else(|| "the core this device runs it under".into());
            self.say(format!("Another core made this state, and {core} can't load it."));
            return Task::none();
        }
        let build = record.rom_id.unwrap_or(tile);
        if self.downloads.installed(build).is_none() {
            self.say(format!(
                "\"{}\" isn't on this device. A state goes into a slot beside the game, so download it first.",
                self.title_of(build)
            ));
            return Task::none();
        }
        let when = record
            .updated_at
            .as_deref()
            .map(|t| t.replace('T', " ").chars().take(16).collect::<String>())
            .unwrap_or_else(|| "an unknown time".into());
        let row = |label: &str, detail: &str| crate::ui::picker::Row { label: label.into(), detail: detail.into(), aside: String::new(), mark: None };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Put this state into a slot?".into(),
                note: format!(
                    "{}, from {when}. It goes into one of RetroArch's slots for this game, and up to RomM as that slot, \
                     so your other devices take it too. Whatever the slot holds here goes to save-backups first.",
                    record.file_name
                ),
                rows: vec![
                    row("The next free slot", "Past every slot here and on RomM: nothing is replaced"),
                    row("Slot 0", "Takes the place of what's in it"),
                    row("The auto slot", "What RetroArch loads by itself on start, with that turned on"),
                    row("Leave it", "Change nothing"),
                ],
                // The one that replaces nothing.
                cursor: 0,
            },
            then: Then::PutState { tile, state: Box::new(record) },
        });
        Task::none()
    }

    /// Put a state from RomM into one of a game's slots here — see [`crate::sync::put_state`].
    pub(super) fn put_state(&mut self, tile: i64, state: crate::romm::models::SaveAsset, target: crate::sync::states::Target) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            self.say("Not paired with a RomM server.".into());
            return Task::none();
        };
        let build = state.rom_id.unwrap_or(tile);
        let Some(installed) = self.downloads.installed(build).cloned() else { return Task::none() };
        let (client, settings, store) = (self.client.clone(), self.settings.clone(), Settings::config_dir());
        self.say("Putting the state into a slot…".into());
        Task::perform(
            async move {
                // Both read config files and ES-DE's tables: off the UI thread.
                let (install, plan) = off_thread(move || {
                    (crate::sync::retroarch::locate(settings.retroarch_config.as_deref()), crate::launch::Plan::new(&settings, &installed))
                })
                .await?;
                crate::sync::put_state(&client, &connection, install.as_ref().map_err(String::as_str), &store, &plan, &state, target).await
            },
            move |result| Message::StatePut(tile, result),
        )
    }

    /// Put an older version of one of a game's saves back — see [`crate::sync::restore`].
    pub(super) fn restore_save(&mut self, tile: i64, version: crate::romm::models::SaveAsset) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            self.say("Not paired with a RomM server.".into());
            return Task::none();
        };
        let build = version.rom_id.unwrap_or(tile);
        let Some(installed) = self.downloads.installed(build).cloned() else { return Task::none() };
        let (client, settings, store) = (self.client.clone(), self.settings.clone(), Settings::config_dir());
        self.say(format!("Restoring {}…", version.file_name));
        Task::perform(
            async move {
                // Both read config files and ES-DE's tables: off the UI thread.
                let (install, plan) = off_thread(move || {
                    (crate::sync::retroarch::locate(settings.retroarch_config.as_deref()), crate::launch::Plan::new(&settings, &installed))
                })
                .await?;
                crate::sync::restore(&client, &connection, install.as_ref().map_err(String::as_str), &store, &plan, &version).await
            },
            move |result| Message::SaveRestored(tile, result),
        )
    }

    /// How many rows the Saves page's current tab has.
    pub(super) fn saves_rows(&self, rom_id: i64) -> usize {
        match self.game_saves.get(&rom_id) {
            Some(Ok(saves)) => saves.list(self.saves_focus.tab).len(),
            _ => 0,
        }
    }

    /// Register this install with RomM as a device, or check it still is — see [`crate::sync::device`].
    pub(super) fn ensure_device(&self) -> Task<Message> {
        let Some(mut connection) = self.settings.connection.clone() else { return Task::none() };
        let client = self.client.clone();
        Task::future(async move { crate::sync::device::ensure(&client, &mut connection).await })
            .and_then(|id| Task::done(Message::DeviceReady(id)))
    }

    /// Where ES-DE's hooks stand — it compares this binary with the hooks' copy, so off the UI thread.
    pub(super) fn read_hooks(&mut self) -> Task<Message> {
        self.hooks = None;
        Task::perform(
            async { off_thread(crate::hooks::status).await.unwrap_or_default() },
            Message::HooksRead,
        )
    }

    /// Sync RetroArch's battery saves with RomM — see [`crate::sync`].
    pub(super) fn sync_saves(&mut self) -> Task<Message> {
        self.sync_saves_of(None)
    }

    /// [`App::sync_saves`], narrowed to the games `changed` are saves of when given — the watcher's.
    pub(super) fn sync_saves_of(&mut self, changed: Option<Vec<std::path::PathBuf>>) -> Task<Message> {
        if matches!(self.save_sync, SaveSync::Running) {
            return Task::none();
        }
        let Some(connection) = self.settings.connection.clone() else {
            self.save_sync = SaveSync::Failed("not paired with a RomM server".into());
            return Task::none();
        };
        self.save_sync = SaveSync::Running;
        Task::perform(
            crate::sync::sync_retroarch(
                self.client.clone(),
                connection,
                self.settings.clone(),
                crate::sync::builds(&self.library),
                Settings::config_dir(),
                changed,
            ),
            Message::SavesSynced,
        )
    }
}
