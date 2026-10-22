//! Playing a game — see [`crate::launch`].

use super::*;

impl App {
    /// Play a game that is on this device — see [`crate::launch::play`].
    pub(super) fn play(&mut self, rom_id: i64) -> Task<Message> {
        if let Some(other) = self.playing {
            // A press while a launch is getting ready, or while the app is back up between a game
            // closing and its save going up. Either way the first one is not finished.
            self.say(format!("\"{}\" is still running.", self.title_of(other)));
            return Task::none();
        }
        let title = self.title_of(rom_id);
        let Some(installed) = self.downloads.installed(rom_id).cloned() else {
            self.say(format!("\"{title}\" isn't on this device — download it first."));
            return Task::none();
        };
        self.playing = Some(rom_id);
        // The BIOS and save steps can take a few seconds with nothing else moving, so the press is
        // acknowledged now, across the whole window, rather than when the game appears.
        self.handover = Some(Handover::new(rom_id, false));
        let ctx = self.launch_context();
        let settings = self.settings.clone();
        // What the window being asked to close asks of the game — see `crate::launch::Stop`.
        let stop = crate::launch::Stop::default();
        self.stop = Some(stop.clone());
        Task::run(
            iced::stream::channel(2, move |mut output: Sender<crate::launch::Event>| async move {
                let mut stages = output.clone();
                // A save changed on both sides is put to the window, and the sync waits for the
                // answer — the newer side if the window has gone.
                let asks = output.clone();
                let resolver: crate::sync::Resolver = std::sync::Arc::new(move |conflict: crate::sync::Conflict| {
                    let mut asks = asks.clone();
                    Box::pin(async move {
                        let newer = conflict.newer;
                        let (reply, answer) = futures::channel::oneshot::channel();
                        let event = crate::launch::Event::Conflict(rom_id, conflict, crate::launch::Reply::new(reply));
                        if asks.send(event).await.is_err() {
                            return newer;
                        }
                        answer.await.unwrap_or(newer)
                    })
                });
                let play = crate::launch::play(ctx, settings, installed, stop, move |stage| {
                    let _ = stages.try_send(match stage {
                        crate::launch::Stage::Running => crate::launch::Event::Running(rom_id),
                        crate::launch::Stage::Saving => crate::launch::Event::Saving(rom_id),
                    });
                });
                // Ended is what gives the pad back and lets another game start, so it is sent even
                // when the launch panics part way — otherwise the app would wait on a game that is
                // not running, ignoring the pad, for the rest of the session.
                let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(crate::sync::with_resolver(resolver, play)))
                    .await
                    .unwrap_or_else(|_| Err("Deckhand hit a fault while starting it".into()));
                let _ = output.send(crate::launch::Event::Ended(rom_id, result)).await;
            }),
            Message::LaunchEvent,
        )
    }

    /// Asked to close by a signal. With no game running, the window goes. With one, the game is asked
    /// to close first, the way its own Quit would, and the window goes once its saves have gone up —
    /// or been left in its marker for the next start (see `crate::playing`). Asked again meanwhile,
    /// it goes at once: the marker has it.
    pub(super) fn stop_asked(&mut self) -> Task<Message> {
        match &self.stop {
            Some(stop) if !self.closing => {
                stop.ask();
                self.closing = true;
                Task::none()
            }
            _ => iced::exit(),
        }
    }

    /// Step aside while a game runs, and come back with what happened when it ends.
    pub(super) fn on_launch_event(&mut self, event: crate::launch::Event) -> Task<Message> {
        use crate::launch::Event;
        match event {
            Event::Conflict(rom_id, conflict, reply) => {
                let keep = conflict.newer;
                self.conflict = Some(PendingConflict { rom_id, conflict, reply, keep });
                Task::none()
            }
            Event::Running(rom_id) => {
                self.handover = None;
                self.say(format!(
                    "Playing \"{}\" — its save goes up to RomM when it closes.",
                    self.title_of(rom_id)
                ));
                // The window is left alone: the emulator opens its own on top. Minimising this one
                // is what the Kotlin app does, but on Wayland a window can minimise itself and never
                // come back, which would strand the app behind the game it launched.
                Task::none()
            }
            Event::Saving(rom_id) => {
                // The window to the front for it: the game's has gone, and quitting this one now
                // would cut its save short.
                self.handover = Some(Handover::new(rom_id, true));
                iced::window::latest().and_then(iced::window::gain_focus)
            }
            Event::Ended(rom_id, result) => {
                self.watcher.rebase();
                self.playing = None;
                self.handover = None;
                self.stop = None;
                // Asked to close while the game ran: it has, and what its closing owed is done or left
                // in its marker, so the window goes too.
                if self.closing {
                    return iced::exit();
                }
                // The sync around the game has moved its saves; the counts and the page follow.
                let refresh = match &self.screen {
                    Screen::Browsing(Route::Detail { rom_id: open, .. } | Route::Saves { rom_id: open, .. }) if *open == rom_id => {
                        self.load_saves(rom_id)
                    }
                    _ => {
                        self.game_saves.remove(&rom_id);
                        Task::none()
                    }
                };
                let title = self.title_of(rom_id);
                // What the sync after the game did: where the saves icon stands now, unless a sync of
                // every save is under way, whose answer is newer.
                let report = result.as_ref().ok().and_then(|session| session.report.clone()).map(|mut report| {
                    self.name_games(&mut report);
                    report
                });
                let pictureless = report.as_ref().is_some_and(|r| r.pictureless > 0);
                if let Some(report) = report
                    && !matches!(self.save_sync, SaveSync::Running)
                {
                    self.save_sync = SaveSync::Done(report);
                    self.save_synced_at = Some(std::time::Instant::now());
                }
                let no_emulator = matches!(&result, Err(why) if why == crate::launch::NO_EMULATOR);
                let words = match result {
                    Err(why) => format!("\"{title}\" could not be launched: {why}."),
                    Ok(session) => {
                        // *Continue playing* follows last_played, which RomM has just moved; mirrored
                        // here so it says so before the next sync.
                        if let Some(when) = session.played_at {
                            self.downloads.mark_played(rom_id, &when);
                            Arc::make_mut(&mut self.library).mark_played(rom_id, when);
                            self.rebuild_shelves();
                        }
                        let mut parts = Vec::new();
                        if !session.exit.success {
                            let code = session.exit.code.map_or_else(|| "a signal".to_string(), |c| format!("code {c}"));
                            parts.push(match session.exit.last_line {
                                Some(line) => format!("it exited with {code}: {line}"),
                                None => format!("it exited with {code}"),
                            });
                        }
                        parts.extend(session.notes);
                        if parts.is_empty() {
                            format!("Back from \"{title}\".")
                        } else {
                            format!("Back from \"{title}\" — {}.", parts.join("; "))
                        }
                    }
                };
                self.say(words);
                // Nothing to start it with: RetroDECK is offered, where it can be installed. A state of it
                // went up without a picture: RetroArch's state pictures are.
                let retrodeck = if no_emulator { self.look_for_retrodeck(Offering::NoEmulator) } else { Task::none() };
                let pictures = if pictureless { self.offer_state_pictures() } else { Task::none() };
                // Back to the front for the result. A compositor may refuse; then it is one click away.
                Task::batch([iced::window::latest().and_then(iced::window::gain_focus), refresh, retrodeck, pictures])
            }
        }
    }

    /// The Downloads page's rows, which both the view and the cursor are measured against.
    pub(super) fn download_rows(&self) -> Vec<crate::ui::downloads::Entry<'_>> {
        crate::ui::downloads::entries(&self.library, &self.downloads)
    }

    /// A game's title, for a banner. Falls back to the queue's copy, then to the id: a download can
    /// finish for a game the library no longer holds, and a resumed queue can name one it has never
    /// held.
    pub(super) fn title_of(&self, rom_id: i64) -> String {
        self.library
            .game(rom_id)
            .map(|g| g.title.clone())
            .or_else(|| {
                self.downloads.installed(rom_id).map(|i| i.title.clone()).or_else(|| {
                    self.downloads
                        .in_progress()
                        .find(|(i, _)| i.rom_id == rom_id)
                        .map(|(i, _)| i.title.clone())
                })
            })
            .unwrap_or_else(|| format!("#{rom_id}"))
    }

    /// Rebuild Home's shelves against the current installed set, keeping the cursor in bounds.
    ///
    /// Needed because a shelf holds *cloned* games rather than borrowing them, so "On this device"
    /// cannot follow a download finishing on its own.
    pub(super) fn rebuild_shelves(&mut self) {
        self.shelves = home_shelves(&self.library, &self.collections, &self.downloads.installed_ids());
        self.focus.reconcile(&self.shelves);
        // A refresh may have rewritten what a game says about itself.
        self.spotlight_text.borrow_mut().clear();
        self.hero_titles.borrow_mut().clear();
    }

    /// Delete the art caches, then report what is left — a fetch in flight may already have put
    /// something back.
    pub(super) fn clear_image_cache(&mut self) -> Task<Message> {
        self.image_cache_bytes = None;
        Task::perform(
            async {
                off_thread(|| {
                    covers::clear_disk_caches();
                    covers::disk_usage()
                })
                .await
                .unwrap_or(0)
            },
            Message::ImageCacheMeasured,
        )
    }

    /// Resolve a host-relative RomM asset path against the connected server.
    pub(super) fn asset_url(&self, path: &str) -> String {
        match &self.settings.connection {
            _ if path.starts_with("http://") || path.starts_with("https://") => path.to_string(),
            Some(c) => format!("{}{}", c.host.trim_end_matches('/'), path),
            None => path.to_string(),
        }
    }
}
