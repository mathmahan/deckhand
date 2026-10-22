//! The Settings page: its rows, and what they change.

use super::*;

impl App {
    /// Confirm on a settings row.
    pub(super) fn activate_setting(&mut self, setting: SettingRow) -> Task<Message> {
        match setting {
            SettingRow::Connection => self.test_connection(),
            // Confirm cycles, wrapping, so a stepped row is fully usable from the one button.
            SettingRow::UiScale | SettingRow::GridDensity | SettingRow::Budget | SettingRow::Resume | SettingRow::WatchSaves => match setting.stepped(&self.settings) {
                Some((options, selected)) => self.pick_setting(setting, (selected + 1) % options.len()),
                None => Task::none(),
            },
            SettingRow::ImageCache => self.clear_image_cache(),
            // Installed hooks come off; anything else installs, or reinstalls what is stale.
            SettingRow::EsDeHooks => {
                let remove = self.hooks.as_ref().is_some_and(|s| s.describe().0);
                Task::perform(
                    async move {
                        match off_thread(move || {
                            let result = if remove { crate::hooks::remove() } else { crate::hooks::install() };
                            (result, crate::hooks::status())
                        })
                        .await
                        {
                            Ok(done) => done,
                            // Where things stand is still worth reading, on the pool like the rest.
                            Err(why) => (Err(why), off_thread(crate::hooks::status).await.unwrap_or_default()),
                        }
                    },
                    |(result, status)| Message::HooksChanged(result, status),
                )
            }
            // Confirm looks again — for a RetroArch installed while this page was open.
            SettingRow::SaveFolder => self.locate_saves(),
            SettingRow::StatePictures => self.press_state_pictures(),
            // The header's Refresh: the full read, which is the answer to a rom edited in place
            // that the count-based sync cannot see.
            SettingRow::LibraryCache => self.load_library(),
            // The total is here; which game to remove is a question only the Downloads page has
            // the per-game sizes to answer.
            SettingRow::Downloaded => self.update(Message::TabSelected(Tab::Downloads)),
            // A list of the drives here, with what each has free.
            SettingRow::RomsFolder => self.pick_location(),
            // Ask, install or restart, whichever is next.
            SettingRow::Updates => self.press_updates(),
            SettingRow::SignOut => self.update(Message::SignOut),
            // While RetroDECK installs, its row is how the install is stopped.
            SettingRow::Launcher if matches!(self.retrodeck_setup, RetroDeckSetup::Installing(_)) => self.ask_to_stop_retrodeck(),
            SettingRow::Launcher => self.pick_launcher(),
            SettingRow::SystemEmulators => self.pick_system_emulators(),
            SettingRow::SteamLibrary => self.add_all_to_steam(),
            SettingRow::RetroArchConfig => self.pick_retroarch_config(),
            // Read-only on purpose. The address is whatever pairing established, together with a
            // token for it; editing it alone could only point the app at a host with no token.
            SettingRow::ServerAddress => {
                self.say(
                    "The server address comes from pairing. Sign out and pair again to change it."
                        .into(),
                );
                Task::none()
            }
            SettingRow::Certificate | SettingRow::Device | SettingRow::Version | SettingRow::ConfigFile => {
                Task::none()
            }
        }
    }

    /// Set a stepped setting to one of its options, and write it through.
    pub(super) fn pick_setting(&mut self, setting: SettingRow, option: usize) -> Task<Message> {
        let task = match setting {
            SettingRow::UiScale => {
                let Some(&scale) = theme::ui_scale::STEPS.get(option) else { return Task::none() };
                self.set_ui_scale(scale)
            }
            SettingRow::GridDensity => {
                let Some(&density) = crate::grid::density::STEPS.get(option) else {
                    return Task::none();
                };
                self.settings.grid_density = density;
                Task::none()
            }
            SettingRow::Resume => {
                self.settings.resume_state = option == 1;
                Task::none()
            }
            SettingRow::WatchSaves => {
                self.settings.watch_saves = option == 1;
                // Started again from a fresh look, so nothing changed while it was off counts twice.
                self.watcher = crate::watch::Watcher::default();
                Task::none()
            }
            SettingRow::Budget => {
                let Some(&gib) = downloads::storage::BUDGET_STEPS.get(option) else { return Task::none() };
                self.settings.storage_budget_gb = (gib > 0).then_some(gib);
                Task::none()
            }
            _ => return Task::none(),
        };
        self.write_settings();
        task
    }

    /// Change the UI scale in place, keeping the layout the size it was.
    ///
    /// iced's scale factor multiplies the whole layout, so a larger scale on an unchanged window
    /// leaves fewer layout units to fill — at Huge the 420-unit hero would take two thirds of the
    /// height. `main` avoids that at launch by opening the window at the panel size times the
    /// scale; this does the same to the window already open, by the ratio of the two scales, so a
    /// window the user has sized keeps its proportions rather than snapping back to the default.
    ///
    /// Where the window cannot grow — maximised, or a fullscreen Deck — no resize event comes back,
    /// so the layout size is set here to what that window holds at the new scale. When the resize
    /// does land, its event puts it back.
    pub(super) fn set_ui_scale(&mut self, scale: f32) -> Task<Message> {
        let old = self.settings.ui_scale.max(0.1);
        if (scale - old).abs() < 0.001 {
            return Task::none();
        }
        let before = self.layout_scale();
        // The room the page has now, which the window keeps for it at the new scale.
        let layout = iced::Size::new(self.content_width, self.content_height);
        self.settings.ui_scale = scale;
        // Until the window follows, the layout is what the window holds at the new scale.
        self.content_width *= before / self.layout_scale();
        self.content_height *= before / self.layout_scale();

        // Art was decoded for the pixels the old scale drew. A larger scale draws it bigger, and it
        // would stay soft for as long as it was held — the same trade as a move to a denser panel.
        if scale > old {
            self.covers.drop_art();
            self.art_generation += 1;
        }
        // Filling the screen, the window stays as it is and the layout scales inside it.
        if self.fullscreen {
            return self.fetch_art();
        }
        // In layout units, as iced takes a size it is asked to make a window: it applies the new
        // scale itself, so the layout keeps the room it had — and the window grows with the type.
        let min_size = iced::Size::new(crate::PANEL.0, crate::PANEL.1) * 0.7;
        Task::batch([
            iced::window::latest().and_then(move |id| iced::window::resize(id, layout)),
            iced::window::latest().and_then(move |id| iced::window::set_min_size(id, Some(min_size))),
            self.fetch_art(),
        ])
    }

    /// Ask iced for the window's size — answered as a [`Message::Resized`].
    pub(super) fn ask_window_size(&self) -> Task<Message> {
        iced::window::latest().and_then(iced::window::size).map(|size| Message::Resized(size.width, size.height))
    }

    /// Write the settings through, as every change on the Settings page does.
    ///
    /// A failed write keeps the change for this session and says so: the setting is already on
    /// screen, and only remembering it failed.
    pub(super) fn write_settings(&mut self) {
        if let Err(e) = self.save_settings() {
            self.say(format!("The setting applies now, but could not be saved: {e}"));
        }
    }

    /// The one way the app writes `settings.json`: never from fixture mode, which is a synthetic
    /// session with no business rewriting the real file — and is what the tests run in.
    pub(super) fn save_settings(&self) -> std::io::Result<()> {
        if !self.writes_settings {
            return Ok(());
        }
        // `deckhand pre|post|play` may have registered this device with RomM since the window read
        // the file. Its id is kept rather than written over with none — losing it would have the
        // next launch register the device again.
        let mut settings = self.settings.clone();
        if let Some(connection) = settings.connection.as_mut().filter(|c| c.device_id.is_none())
            && let Some(theirs) = Settings::load().connection.filter(|c| c.host == connection.host).and_then(|c| c.device_id)
        {
            connection.device_id = Some(theirs);
        }
        settings.save()
    }

    /// Prove the stored token still reads, the same way pairing proved it before storing it.
    pub(super) fn test_connection(&mut self) -> Task<Message> {
        if matches!(self.connection_check, Check::Testing) {
            return Task::none();
        }
        let Some(connection) = self.settings.connection.clone() else {
            self.connection_check = Check::Failed("not paired with a RomM server".into());
            return Task::none();
        };
        self.connection_check = Check::Testing;
        let client = self.client.clone();
        Task::perform(
            async move {
                client
                    .fetch_current_user(&connection)
                    .await
                    .map(|user| user.username.unwrap_or_else(|| format!("user {}", user.id)))
                    .map_err(|e| e.to_string())
            },
            Message::ConnectionTested,
        )
    }

    /// Measure the art caches, off the UI thread: it is a walk over every file in them.
    pub(super) fn measure_image_cache(&mut self) -> Task<Message> {
        self.image_cache_bytes = None;
        Task::perform(
            async { off_thread(covers::disk_usage).await.unwrap_or(0) },
            Message::ImageCacheMeasured,
        )
    }

    /// Find where RetroArch keeps saves, off the UI thread: it reads RetroArch's config and the
    /// cores' info files.
    pub(super) fn locate_saves(&mut self) -> Task<Message> {
        self.save_folder = None;
        self.retroarch_found = None;
        self.state_pictures = None;
        let chosen = self.settings.retroarch_config.clone();
        Task::perform(
            async move {
                off_thread(move || {
                    let config = chosen.or_else(crate::sync::retroarch::found);
                    let install = crate::sync::retroarch::locate(config.as_deref());
                    let pictures = install.as_ref().map(|install| install.state_pictures()).map_err(Clone::clone);
                    (config, install.map(|install| install.saves), pictures)
                })
                .await
                .unwrap_or_else(|why| (None, Err(why.clone()), Err(why)))
            },
            |(config, saves, pictures)| Message::SaveFolderFound(config, saves, pictures),
        )
    }

    /// Whether RetroArch keeps a picture with each state, read off the UI thread.
    fn read_state_pictures(&self) -> Task<Message> {
        let chosen = self.settings.retroarch_config.clone();
        Task::perform(
            async move {
                off_thread(move || crate::sync::retroarch::locate(chosen.as_deref()).map(|install| install.state_pictures()))
                    .await
                    .unwrap_or_else(Err)
            },
            Message::StatePicturesFound,
        )
    }

    /// Settings ▸ Saves ▸ State pictures: turned on when they are off. When an override of
    /// RetroArch's is what turns them off, that file is named rather than edited.
    pub(super) fn press_state_pictures(&mut self) -> Task<Message> {
        match &self.state_pictures {
            Some(Ok(pictures)) if !pictures.on => self.turn_on_state_pictures(),
            Some(Ok(pictures)) if !pictures.off_in.is_empty() => {
                self.say(overridden(&pictures.off_in));
                Task::none()
            }
            Some(Ok(_)) => {
                self.say("RetroArch keeps a picture with every save state already.".into());
                Task::none()
            }
            Some(Err(why)) => {
                self.say(format!("State pictures can't be set: {why}"));
                Task::none()
            }
            None => Task::none(),
        }
    }

    /// Turn RetroArch's state pictures on in its config — refused while RetroArch runs, which would
    /// turn them off again as it quits.
    pub(super) fn turn_on_state_pictures(&mut self) -> Task<Message> {
        let chosen = self.settings.retroarch_config.clone();
        Task::future(async move {
            let done = off_thread(move || {
                let config = crate::sync::retroarch::locate(chosen.as_deref())?.config().to_path_buf();
                let turned = crate::sync::retroarch::turn_on_state_pictures(&config, crate::sync::retroarch::retroarch_running);
                Ok((turned, crate::sync::retroarch::read(&config).map(|install| install.state_pictures())))
            })
            .await
            .unwrap_or_else(Err);
            match done {
                Ok((turned, now)) => Message::StatePicturesChanged(turned, now),
                Err(why) => Message::StatePicturesChanged(Err(why.clone()), Err(why)),
            }
        })
    }

    pub(super) fn state_pictures_changed(&mut self, turned: Result<(), String>, now: Result<StatePictures, String>) -> Task<Message> {
        let off_in = now.as_ref().map(|p| p.off_in.clone()).unwrap_or_default();
        self.say(match turned {
            Ok(()) if off_in.is_empty() => "State pictures are on: every state RetroArch saves from now on takes one, and RomM shows it. \
                                            States already saved get theirs the next time they're saved."
                .into(),
            Ok(()) => format!("State pictures are on. {}", overridden(&off_in)),
            Err(why) => format!("State pictures were left off: {why}."),
        });
        self.state_pictures = Some(now);
        Task::none()
    }

    /// A sync sent a RetroArch state up with no picture: turning state pictures on is offered, once a
    /// session, and never again once declined.
    pub(super) fn offer_state_pictures(&mut self) -> Task<Message> {
        if self.settings.state_pictures_declined || self.state_pictures_offered {
            return Task::none();
        }
        self.state_pictures_offered = true;
        self.read_state_pictures()
    }

    /// Where state pictures stand, read for the offer: put to the player when they are off.
    pub(super) fn state_pictures_found(&mut self, pictures: Result<StatePictures, String>) -> Task<Message> {
        use crate::ui::picker::Row;
        let off = matches!(&pictures, Ok(p) if !p.on);
        self.state_pictures = Some(pictures);
        if !off {
            return Task::none();
        }
        // Another choice is already up: this one waits for the next sync's.
        if self.pick.is_some() {
            self.state_pictures_offered = false;
            return Task::none();
        }
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Keep a picture with each save state?".into(),
                note: "A RetroArch state just went to RomM without a picture: RetroArch's state pictures are off, as \
                       RetroDECK sets them. Turned on, every state saved from now on takes one, which RomM and the \
                       game's Saves page show. States already saved get theirs the next time they're saved."
                    .into(),
                rows: vec![
                    Row::new("Turn them on", "RetroArch has to be closed while its config changes"),
                    Row::new("Not now", "Settings ▸ Saves ▸ State pictures turns them on later"),
                ],
                cursor: 0,
            },
            then: Then::StatePictures,
        });
        crate::ui::picker::scroll_to(0)
    }

    /// No to state pictures, which is not asked again.
    pub(super) fn decline_state_pictures(&mut self) {
        self.settings.state_pictures_declined = true;
        self.write_settings();
    }
}

/// What the banner says of overrides that turn state pictures off: RetroArch's own files, named for
/// the player to change rather than edited here.
fn overridden(off_in: &[std::path::PathBuf]) -> String {
    let files = off_in.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ");
    let them = if off_in.len() == 1 { "An override turns them off again for its core or game" } else { "Overrides turn them off again for their cores or games" };
    format!("{them}: {files}. Change that in RetroArch's menu, or in the file.")
}
