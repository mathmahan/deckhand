//! Downloads: asking for a game, the queue, and taking it off again.

use super::*;

impl App {
    /// The game a rom id is — its tile's own rom, or one of the builds under it.
    pub(super) fn game_of(&self, rom_id: i64) -> Option<&Game> {
        self.library.game_of_build(rom_id)
    }

    /// Every build of the game a rom id belongs to — just the id, when the library does not know it.
    pub(super) fn builds_of(&self, rom_id: i64) -> Vec<i64> {
        self.game_of(rom_id).map_or_else(|| vec![rom_id], |g| g.builds().collect())
    }

    /// The state to draw for the game a rom id belongs to — across every build of it.
    pub(super) fn game_state(&self, rom_id: i64) -> crate::downloads::SyncState {
        self.downloads.state_of(self.builds_of(rom_id))
    }

    /// Hold what is on this device to the library just set — see [`Downloads::reconcile`] — and
    /// note which have an update waiting.
    ///
    /// A game RomM keeps under another id now takes its Steam shortcut's binding along, so taking it
    /// out of Steam or refreshing it still finds the shortcut — whose `play <id>` still starts it
    /// ([`Downloads::installed_as`]). What moved and what RomM no longer has is said, since it is
    /// the sign RomM was rebuilt under this device.
    pub(super) fn match_downloads(&mut self) {
        let done = self.downloads.reconcile(&self.library);
        self.downloads.check_updates(&self.library);
        if done.moved.is_empty() && done.astray.is_empty() {
            return;
        }
        let host = self.host().to_string();
        // Every old binding read before any is written: two games can trade ids.
        let bound: Vec<(i64, u32)> = done.moved.iter().filter_map(|(was, is)| Some((*is, self.steam_bindings.get(&host, *was)?))).collect();
        for (was, _) in &done.moved {
            self.steam_bindings.unbind(&host, *was);
        }
        for (is, appid) in &bound {
            self.steam_bindings.bind(&host, *is, *appid);
        }
        if !bound.is_empty() {
            self.steam_bindings.save();
        }
        let mut said = Vec::new();
        match done.moved.len() {
            0 => {}
            1 => said.push("RomM keeps a game on this device under a new id now, and its saves go there with it.".to_string()),
            n => said.push(format!("RomM keeps {n} games on this device under new ids now, and their saves go there with them.")),
        }
        if !done.astray.is_empty() {
            let more = done.astray.len().saturating_sub(3);
            let named = done.astray.iter().take(3).map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
            let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
            said.push(format!(
                "RomM no longer has {named}{tail}: their files stay on this device, and they come back here if RomM gets them again."
            ));
        }
        self.say(said.join(" "));
    }

    /// Download a game: what a failed attempt asked for again, or — when the game has discs or other
    /// builds to choose from — the picker, once RomM has said what it holds.
    pub(super) fn download_game(&mut self, rom_id: i64) -> Task<Message> {
        if self.settings.connection.is_none() {
            self.say("Not paired with a RomM server, so there is nothing to download from.".into());
            return Task::none();
        }
        // A retry asks for exactly what failed — the same build, the same disc.
        if let Some(item) = self.builds_of(rom_id).into_iter().find_map(|id| self.downloads.failed_item(id)) {
            return self.enqueue_item(item);
        }
        let Some(game) = self.game_of(rom_id).cloned() else {
            // The library is the only place a rom id can be resolved to a system and a title, and
            // a download needs both to know where to put the files.
            self.say(format!("#{rom_id} is not in the loaded library."));
            return Task::none();
        };
        // Said before anything is asked or queued: RomM would only answer that the file is gone.
        if game.missing() {
            self.say(format!(
                "\"{}\" isn't on RomM's disk, so there is nothing to download — rescan its platform in RomM.",
                game.title
            ));
            return Task::none();
        }
        if downloads::choice::may_ask(&game) {
            let (client, connection, tile) = (self.client.clone(), self.settings.connection.clone(), game.rom_id);
            self.say(format!("Looking at what RomM has of \"{}\"…", game.title));
            return Task::perform(
                async move {
                    let Some(connection) = connection else { return Err("not paired".to_string()) };
                    client.fetch_rom_detail(&connection, tile).await.map(Box::new).map_err(|e| e.to_string())
                },
                move |result| Message::DownloadChoices(tile, result),
            );
        }
        let installed = self.downloads.installed_builds(game.builds());
        let choices = downloads::choice::choices(&game, None, &installed);
        self.enqueue_choice(&game, &choices[0])
    }

    /// What Download offers, now RomM has said what the game's rom holds — straight to the queue
    /// when there is only one thing, the picker otherwise.
    pub(super) fn offer_choices(&mut self, tile: i64, result: Result<Box<crate::romm::models::RomDetail>, String>) -> Task<Message> {
        let Some(game) = self.game_of(tile).cloned() else { return Task::none() };
        // Without the detail the builds can still be offered; only the discs need it.
        let detail = result.ok();
        let installed = self.downloads.installed_builds(game.builds());
        let choices = downloads::choice::choices(&game, detail.as_deref(), &installed);
        self.banner = None;
        match choices.len() {
            0 => {
                self.say(format!("\"{}\" has nothing on RomM's disk to download.", game.title));
                return Task::none();
            }
            1 => return self.enqueue_choice(&game, &choices[0]),
            _ => {}
        }
        let rows = choices
            .iter()
            .map(|c| crate::ui::picker::Row {
                label: c.label.clone(),
                detail: c.detail.clone(),
                aside: c.bytes.map(downloads::human_bytes).unwrap_or_default(),
                mark: c.installed.then(|| "on this device".to_string()),
            })
            .collect();
        let cursor = choices.iter().position(|c| !c.installed).unwrap_or(0);
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: format!("Download \"{}\"", game.title),
                note: "Choose what to put on this device. Each build keeps saves of its own; discs fetched one at a time join the ones already here.".into(),
                rows,
                cursor,
            },
            then: Then::Download { tile, choices },
        });
        crate::ui::picker::scroll_to(cursor)
    }

    /// Queue one of the choices Download offered for `game`.
    pub(super) fn enqueue_choice(&mut self, game: &Game, choice: &downloads::choice::Choice) -> Task<Message> {
        let own = choice.rom_id == game.rom_id;
        let item = QueueItem {
            // Stamped by `Downloads::enqueue`, which is the only thing that knows the server.
            host: String::new(),
            rom_id: choice.rom_id,
            system_id: game.system_id.clone(),
            title: game.title.clone(),
            files: choice.files.clone(),
            label: choice.files.as_ref().map(|_| choice.label.clone()),
            build: (!own).then(|| choice.label.clone()),
            bytes: choice.bytes.or_else(|| game.size_bytes.and_then(|b| u64::try_from(b).ok()).filter(|_| own && choice.files.is_none())),
            stamp: self.library.stamp(choice.rom_id),
            extra: false,
        };
        self.enqueue_item(item)
    }

    /// Put an item in the queue, and start it if a worker is free.
    pub(super) fn enqueue_item(&mut self, item: QueueItem) -> Task<Message> {
        let title = match &item.label {
            Some(part) => format!("{} — {part}", item.title),
            None => item.title.clone(),
        };
        if !self.downloads.enqueue(item) {
            self.say(format!("\"{title}\" is already downloading."));
            return Task::none();
        }
        // Said outright rather than left to the badge: the tile the press came from is often not
        // the thing being looked at, and the hero's button is not the grid.
        self.say(format!("Downloading \"{title}\"…"));
        self.pump_downloads()
    }

    /// Fetch again what changed on RomM, as it was asked for the first time — the same build, the
    /// same discs.
    pub(super) fn update_game(&mut self, rom_id: i64) -> Task<Message> {
        let stale = self
            .builds_of(rom_id)
            .into_iter()
            .find(|id| self.downloads.state(*id) == downloads::SyncState::UpdateAvailable)
            .and_then(|id| self.downloads.installed(id).cloned());
        let Some(record) = stale else { return Task::none() };
        self.enqueue_item(QueueItem {
            host: String::new(),
            rom_id: record.rom_id,
            system_id: record.system_id.clone(),
            title: record.title.clone(),
            files: record.subset.is_some().then(|| record.source_files.clone()),
            label: record.subset.clone(),
            build: record.build,
            bytes: None,
            stamp: record.stamp,
            extra: false,
        })
    }

    /// Play a game: the one build of it on this device, or — with several here — the one picked.
    pub(super) fn play_game(&mut self, rom_id: i64) -> Task<Message> {
        let builds: Vec<(i64, String)> =
            self.downloads.installed_builds(self.builds_of(rom_id)).iter().map(|i| (i.rom_id, i.name())).collect();
        match builds.as_slice() {
            [] => self.play(rom_id),
            [(id, _)] => self.play(*id),
            _ => {
                let title = self.title_of(rom_id);
                let rows = builds
                    .iter()
                    .map(|(_, name)| crate::ui::picker::Row::new(name.clone(), ""))
                    .collect();
                self.pick = Some(PendingPick {
                    picker: crate::ui::picker::Picker {
                        title: format!("Play \"{title}\""),
                        note: "More than one build of it is on this device. Each keeps saves of its own.".into(),
                        rows,
                        cursor: 0,
                    },
                    then: Then::Play { builds: builds.into_iter().map(|(id, _)| id).collect() },
                });
                Task::none()
            }
        }
    }

    /// Take a game off this device — asking which build, when there is more than one here.
    pub(super) fn remove_game(&mut self, rom_id: i64) -> Task<Message> {
        let builds: Vec<(i64, String)> =
            self.downloads.installed_builds(self.builds_of(rom_id)).iter().map(|i| (i.rom_id, i.name())).collect();
        match builds.as_slice() {
            // A failure being dismissed, which is about the rom itself.
            [] => self.remove_download(rom_id),
            [(id, _)] => self.remove_download(*id),
            _ => {
                let title = self.title_of(rom_id);
                let rows = builds
                    .iter()
                    .map(|(id, name)| crate::ui::picker::Row {
                        label: name.clone(),
                        detail: String::new(),
                        aside: self.downloads.installed(*id).map(|i| downloads::human_bytes(i.bytes)).unwrap_or_default(),
                        mark: None,
                    })
                    .collect();
                self.pick = Some(PendingPick {
                    picker: crate::ui::picker::Picker {
                        title: format!("Remove \"{title}\""),
                        note: "Which build should go? Its saves stay on RomM, and one press downloads it again.".into(),
                        rows,
                        cursor: 0,
                    },
                    then: Then::Remove { builds: builds.into_iter().map(|(id, _)| id).collect() },
                });
                Task::none()
            }
        }
    }

    /// Pin every build of a game here — kept when a space budget makes room — or unpin them.
    pub(super) fn toggle_pin(&mut self, rom_id: i64) -> Task<Message> {
        let builds = self.builds_of(rom_id);
        let pin = !self.downloads.installed_builds(builds.clone()).iter().any(|i| i.pinned);
        for id in builds {
            self.downloads.set_pinned(id, pin);
        }
        let title = self.title_of(rom_id);
        self.say(if pin {
            format!("\"{title}\" is pinned: making room for a download never removes it.")
        } else {
            format!("\"{title}\" is unpinned.")
        });
        Task::none()
    }

    /// A button on the game page on screen: done, or the reason it cannot be.
    pub(super) fn detail_action(&mut self, action: crate::ui::detail::Action) -> Task<Message> {
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
            Action::Extras => self.press_extras(rom_id),
            Action::Favourite => self.toggle_favourite(rom_id),
            // This returned a reason above.
            Action::LaunchWith => Task::none(),
        }
    }
}
