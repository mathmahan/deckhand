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
            SettingRow::SyncSaves => self.sync_saves(),
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
        let chosen = self.settings.retroarch_config.clone();
        Task::perform(
            async move {
                off_thread(move || {
                    let config = chosen.or_else(crate::sync::retroarch::found);
                    let saves = crate::sync::retroarch::locate(config.as_deref()).map(|install| install.saves);
                    (config, saves)
                })
                .await
                .unwrap_or_else(|why| (None, Err(why)))
            },
            |(config, saves)| Message::SaveFolderFound(config, saves),
        )
    }
}
