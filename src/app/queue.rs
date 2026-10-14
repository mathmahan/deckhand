//! The download workers: making room, starting them, and what they report.

use super::*;

impl App {
    /// Make room under the space budget for `item`, which has just started: the least recently used
    /// games go first — never a pinned one, never the game being played, never a build of the game
    /// being fetched. Says what went, since the player did not ask for it in so many words.
    pub(super) fn make_room(&mut self, item: &QueueItem) -> Task<Message> {
        let Some(budget) = downloads::storage::budget_bytes(self.settings.storage_budget_gb) else { return Task::none() };
        let mut kept: std::collections::HashSet<i64> = self.playing.iter().flat_map(|id| self.builds_of(*id)).collect();
        kept.extend(self.builds_of(item.rom_id));
        let played = |id: i64| self.game_of(id).and_then(|g| g.last_played.clone());
        let candidates = self.downloads.candidates(&played, &kept);
        let gone: Vec<(i64, String, u64)> = downloads::storage::make_room(&candidates, budget, self.downloads.active_bytes())
            .into_iter()
            .map(|c| (c.rom_id, c.title.clone(), c.bytes))
            .collect();
        if gone.is_empty() {
            return Task::none();
        }
        let freed = gone.iter().map(|(_, _, b)| b).sum::<u64>();
        let names = gone.iter().map(|(_, t, _)| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
        let mut tasks = Vec::new();
        for (rom_id, title, _) in gone {
            if let Some(installed) = self.downloads.remove(rom_id) {
                tasks.push(Task::perform(
                    async move {
                        let result = off_thread(move || downloads::delete(&installed)).await.and_then(|r| r.map_err(|e| e.to_string()));
                        (title, result)
                    },
                    |(title, result)| Message::Evicted(title, result),
                ));
            }
        }
        self.rebuild_shelves();
        self.say(format!(
            "Made room for \"{}\" under the space budget: removed {names} ({}). They stay in your library — one press brings each back.",
            item.title,
            downloads::human_bytes(freed)
        ));
        Task::batch(tasks)
    }

    /// Start as many queued downloads as the gate allows, and hand back the tasks that run them.
    ///
    /// The one place work is spawned, called after anything that could have freed a slot or added
    /// an item — a press, a finish, and the resumed queue at launch. [`Downloads::start_next`]
    /// moves the item into the active set as it hands it over, so this and the queue cannot
    /// disagree about how many workers there are.
    pub(super) fn pump_downloads(&mut self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let root = self.settings.roms_dir();
        let mut tasks = Vec::new();
        while let Some(item) = self.downloads.start_next() {
            tasks.push(self.make_room(&item));
            let existing = self.downloads.installed(item.rom_id).cloned();
            let client = self.client.clone();
            let connection = connection.clone();
            let root = root.clone();
            // A stream rather than a future, because a download is a sequence of answers: the
            // throttled progress reports, and then the one result. `Task::perform` could only
            // carry the last of them.
            let rom_id = item.rom_id;
            let (worker, handle) = Task::run(
                iced::stream::channel(16, move |mut output: Sender<downloads::Event>| async move {
                    let rom_id = item.rom_id;
                    let mut progress = output.clone();
                    let result = downloads::fetch(&client, &connection, &root, &item, existing, |phase| {
                        // A send that cannot keep up is dropped, not awaited: a report is only ever
                        // the latest state of a bar, so losing one costs nothing and blocking the
                        // transfer on the UI would cost the download.
                        let _ = progress.try_send(match phase {
                            downloads::Phase::Fetching(fraction) => downloads::Event::Progress(rom_id, fraction),
                            downloads::Phase::Unpacking(fraction) => downloads::Event::Unpacking(rom_id, fraction),
                        });
                    })
                    .await;
                    let _ = output
                        .send(downloads::Event::Finished(rom_id, result.map(Box::new)))
                        .await;
                }),
                Message::DownloadEvent,
            )
            .abortable();
            // Kept so *Cancel* can stop it — see `remove_download`.
            self.download_workers.insert(rom_id, handle);
            tasks.push(worker);
        }
        Task::batch(tasks)
    }

    /// Fold a worker's news into the state, and refill the slot a finish freed.
    pub(super) fn on_download_event(&mut self, event: downloads::Event) -> Task<Message> {
        match event {
            downloads::Event::Progress(rom_id, fraction) => {
                self.downloads.progress(rom_id, fraction);
                Task::none()
            }
            downloads::Event::Unpacking(rom_id, fraction) => {
                self.downloads.unpacking(rom_id, fraction);
                Task::none()
            }
            downloads::Event::Finished(rom_id, result) => {
                self.download_workers.remove(&rom_id);
                let title = self.title_of(rom_id);
                self.say(match &result {
                    Ok(installed) => format!(
                        "\"{}\" is on this device — {}.",
                        installed.name(),
                        downloads::human_bytes(installed.bytes)
                    ),
                    Err(why) => format!("\"{title}\" could not be downloaded: {why}"),
                });
                // A copy for the BIOS and save pull, which runs after the record is written.
                let ready = result.as_ref().ok().map(|installed| self.ready_download((**installed).clone()));
                let answered = self.answer_installs(rom_id, match &result {
                    Ok(_) => crate::romm::Outcome::Done,
                    Err(why) => crate::romm::Outcome::Failed(why.clone()),
                });
                self.downloads.finish(rom_id, result.map(|boxed| *boxed));
                // Home's "On this device" shelf is built from the installed set, so it has to be
                // rebuilt before the next frame draws — the shelves hold cloned games, which is
                // what makes this a rebuild rather than a field update.
                self.rebuild_shelves();
                Task::batch([self.pump_downloads(), self.fetch_art(), ready.unwrap_or_else(Task::none), answered])
            }
        }
    }

    /// Take a game off the device, dismiss a failure, or drop a queued one.
    ///
    /// The record goes here, in this frame; the files go on a blocking thread. A delete that fails
    /// therefore leaves files behind rather than a record of files that are gone, which is the
    /// harmless direction — the next download of that game overwrites them.
    pub(super) fn remove_download(&mut self, rom_id: i64) -> Task<Message> {
        let title = self.title_of(rom_id);
        // Queued or running: *Cancel*. The worker is stopped where it is, and its slot goes to the
        // next in line.
        if self.downloads.cancel(rom_id) {
            if let Some(worker) = self.download_workers.remove(&rom_id) {
                worker.abort();
            }
            self.say(format!("\"{title}\" won't be downloaded."));
            self.rebuild_shelves();
            let answered = self.answer_installs(rom_id, crate::romm::Outcome::Failed("cancelled on the device".into()));
            return Task::batch([self.pump_downloads(), answered]);
        }
        let Some(installed) = self.downloads.remove(rom_id) else {
            // Nothing was on disk: either a failure being dismissed, which `remove` has already
            // cleared, or a press on a game that was never here.
            self.rebuild_shelves();
            return Task::none();
        };
        self.rebuild_shelves();
        Task::perform(
            async move {
                let result = off_thread(move || downloads::delete(&installed)).await.and_then(|r| r.map_err(|e| e.to_string()));
                (title, result)
            },
            |(title, result)| Message::DownloadRemoved(title, result),
        )
    }

    /// What the BIOS and save steps need, from the app's own state.
    pub(super) fn launch_context(&self) -> crate::launch::Context {
        crate::launch::Context {
            client: self.client.clone(),
            connection: self.settings.connection.clone(),
            retroarch_config: self.settings.retroarch_config.clone(),
            store: Settings::config_dir(),
        }
    }

    /// A finished download's BIOS, and its save from RomM — see [`crate::launch::after_download`].
    pub(super) fn ready_download(&self, installed: downloads::Installed) -> Task<Message> {
        let rom_id = installed.rom_id;
        Task::perform(
            crate::launch::after_download(self.launch_context(), self.settings.clone(), installed),
            move |notes| Message::Readied(rom_id, notes),
        )
    }
}
