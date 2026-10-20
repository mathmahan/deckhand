//! The pickers and the text field Settings opens: folders, RetroArch's config, emulators, tokens.

use super::*;

impl App {
    /// Where downloads go: home, or any drive mounted here — the picker, with what each has free.
    pub(super) fn pick_location(&mut self) -> Task<Message> {
        let places = downloads::storage::locations(&self.settings.roms_dir(), &Settings::default_roms_dir());
        let rows = places
            .iter()
            .map(|place| crate::ui::picker::Row {
                label: place.name.clone(),
                detail: place.path.display().to_string(),
                aside: match (place.free, place.total) {
                    (Some(free), Some(total)) => format!("{} free of {}", downloads::human_bytes(free), downloads::human_bytes(total)),
                    _ => String::new(),
                },
                mark: place.current.then(|| "in use".to_string()),
            })
            .collect();
        let mut rows: Vec<crate::ui::picker::Row> = rows;
        rows.push(crate::ui::picker::Row {
            label: "Another folder…".into(),
            detail: "Walk this device's folders to any one".into(),
            aside: String::new(),
            mark: None,
        });
        let cursor = places.iter().position(|p| p.current).unwrap_or(0);
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Where downloads go".into(),
                note: "New downloads go to the folder picked here. Games already downloaded stay where they are.".into(),
                rows,
                cursor,
            },
            then: Then::Location { places },
        });
        Task::none()
    }

    /// The keys and buttons that walk and answer a waiting picker. None for anything else, which
    /// goes on as usual — a download's progress keeps arriving while the player decides.
    pub(super) fn answer_pick(&mut self, message: &Message) -> Option<Task<Message>> {
        use crate::gamepad::Pad;
        let pending = self.pick.as_mut()?;
        let chosen = match message {
            Message::Nav(direction) | Message::Pad(Pad::Nav(direction)) => {
                let delta = match direction {
                    Direction::Up => -1,
                    Direction::Down => 1,
                    _ => return Some(Task::none()),
                };
                if pending.picker.step(delta) {
                    let at = crate::ui::picker::scroll_offset(pending.picker.cursor);
                    return Some(iced::widget::operation::scroll_to(crate::ui::picker::list_id(), iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: at }));
                }
                return Some(Task::none());
            }
            Message::Activate | Message::Pad(Pad::Confirm) => pending.picker.cursor,
            Message::Picked(index) => *index,
            Message::Dismiss | Message::Pad(Pad::Back) => {
                self.pick = None;
                return Some(Task::none());
            }
            _ => return None,
        };
        let pending = self.pick.take()?;
        Some(match pending.then {
            Then::Download { tile, choices } => {
                let Some(choice) = choices.get(chosen) else { return Some(Task::none()) };
                let Some(game) = self.game_of(tile).cloned() else { return Some(Task::none()) };
                if choice.installed {
                    self.say(format!("\"{}\" — {} is on this device already.", game.title, choice.label));
                    return Some(Task::none());
                }
                self.enqueue_choice(&game, choice)
            }
            Then::Play { builds } => match builds.get(chosen) {
                Some(id) => self.play(*id),
                None => Task::none(),
            },
            Then::Remove { builds } => match builds.get(chosen) {
                Some(id) => self.remove_download(*id),
                None => Task::none(),
            },
            Then::Location { places } => match places.get(chosen) {
                Some(place) => Self::send_downloads_to(place.path.clone()),
                None => {
                    let from = crate::folders::start(&self.settings.roms_dir());
                    self.browse(Browsing::RomsFolder, from, None)
                }
            },
            Then::Browse { purpose, dir, steps } => match steps.get(chosen).cloned() {
                Some(crate::folders::Step::Open(path)) => {
                    // Up lands on the folder it came out of.
                    let came_from = (dir.parent() == Some(path.as_path())).then_some(dir);
                    self.browse(purpose, path, came_from)
                }
                Some(crate::folders::Step::Use(path)) => Self::send_downloads_to(path),
                Some(crate::folders::Step::Pick(path)) => match purpose {
                    Browsing::Emulator(system) => self.picked_emulator(system, &path),
                    _ => self.set_retroarch_config(Some(path)),
                },
                None => Task::none(),
            },
            Then::RetroArch { choices } => match choices.get(chosen).cloned() {
                Some(RetroArchChoice::Auto) => self.set_retroarch_config(None),
                Some(RetroArchChoice::Use(path)) => self.set_retroarch_config(Some(path)),
                Some(RetroArchChoice::Browse) => {
                    let from = self
                        .settings
                        .retroarch_config
                        .as_deref()
                        .and_then(std::path::Path::parent)
                        .map(crate::folders::start)
                        .or_else(dirs::home_dir)
                        .unwrap_or_default();
                    self.browse(Browsing::RetroArchConfig, from, None)
                }
                None => Task::none(),
            },
            Then::Steam { games, add } => match chosen {
                0 => self.change_steam(&games, add, true),
                _ => Task::none(),
            },
            Then::Extras { rom_id, extras } => match extras.get(chosen) {
                Some(extra) => self.pick_extra(rom_id, extra),
                None => Task::none(),
            },
            Then::Restore { tile, version } => match chosen {
                0 => self.restore_save(tile, *version),
                _ => Task::none(),
            },
            Then::PutState { tile, state } => {
                use crate::sync::states::Target;
                match chosen {
                    0 => self.put_state(tile, *state, Target::Next),
                    1 => self.put_state(tile, *state, Target::Zero),
                    2 => self.put_state(tile, *state, Target::Auto),
                    _ => Task::none(),
                }
            }
            Then::Launcher => match chosen {
                0 => {
                    self.settings.launch_command = None;
                    self.write_settings();
                    self.say("The default emulator is now RetroDECK.".into());
                    Task::none()
                }
                _ => self.type_launch_command(),
            },
            Then::SystemEmulators { systems } => match systems.get(chosen) {
                Some(system) => self.pick_emulator_for(system.clone()),
                None => Task::none(),
            },
            Then::SystemEmulator { system, choices } => match choices.get(chosen).cloned() {
                Some(EmulatorChoice::Pick(from)) => self.browse(Browsing::Emulator(system), from, None),
                Some(EmulatorChoice::Type) => {
                    let current = self.settings.system_commands.get(&system).map(|w| crate::launch::join_command(w)).unwrap_or_default();
                    self.type_system_command(system, current, "")
                }
                Some(EmulatorChoice::Shared) => {
                    self.say(format!("{} games now use the default emulator.", self.system_label(&system)));
                    self.settings.system_commands.remove(&system);
                    self.write_settings();
                    Task::none()
                }
                None => Task::none(),
            },
        })
    }

    /// Send new downloads to `path` — made first: a folder that cannot be created is not one to send
    /// a download to.
    pub(super) fn send_downloads_to(path: std::path::PathBuf) -> Task<Message> {
        Task::perform(
            async move {
                off_thread(move || {
                    std::fs::create_dir_all(&path)
                        .map(|()| path.clone())
                        .map_err(|e| format!("Downloads can't go to {}: {e}", path.display()))
                })
                .await
                .and_then(|made| made)
            },
            Message::LocationSet,
        )
    }

    /// One folder of a walk, as a picker. `came_from` puts the cursor on the folder just left.
    pub(super) fn browse(&mut self, purpose: Browsing, dir: std::path::PathBuf, came_from: Option<std::path::PathBuf>) -> Task<Message> {
        use crate::folders::{Step, Want};
        let want = match &purpose {
            Browsing::RomsFolder => Want::Folder,
            Browsing::RetroArchConfig => Want::File("cfg"),
            Browsing::Emulator(_) => Want::Program,
        };
        let (rows, steps) = crate::folders::list(&dir, want);
        let cursor = came_from.and_then(|from| steps.iter().position(|s| *s == Step::Open(from.clone()))).unwrap_or(0);
        let (title, note) = match &purpose {
            Browsing::RomsFolder => ("Where downloads go".to_string(), "Walk to the folder new downloads go into, then pick Use this folder."),
            Browsing::RetroArchConfig => ("RetroArch config".to_string(), "Walk to the retroarch.cfg save sync should read."),
            Browsing::Emulator(system) => (
                format!("Emulator for {}", self.system_label(system)),
                "Navigate to the emulator and select it.",
            ),
        };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker { title, note: format!("{note}  Now in {}", dir.display()), rows, cursor },
            then: Then::Browse { purpose, dir, steps },
        });
        // The list is the same widget from folder to folder, so it would keep the last one's scroll.
        iced::widget::operation::scroll_to(
            crate::ui::picker::list_id(),
            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: crate::ui::picker::scroll_offset(cursor) },
        )
    }

    /// The RetroArch config save sync reads: found automatically, one of those found here, or one
    /// walked to.
    pub(super) fn pick_retroarch_config(&mut self) -> Task<Message> {
        use crate::ui::picker::Row;
        let chosen = self.settings.retroarch_config.clone();
        let found: Vec<std::path::PathBuf> = crate::sync::retroarch::candidates().into_iter().filter(|c| c.is_file()).collect();
        let in_use = |on: bool| on.then(|| "in use".to_string());
        let mut rows = vec![Row {
            label: "Find it automatically".into(),
            detail: found.first().map_or_else(|| "none found on this device".into(), |p| p.display().to_string()),
            aside: String::new(),
            mark: in_use(chosen.is_none()),
        }];
        let mut choices = vec![RetroArchChoice::Auto];
        let mut offered = found;
        if let Some(path) = chosen.as_ref().filter(|p| !offered.contains(p)) {
            offered.push(path.clone());
        }
        for path in offered {
            let label = match path.to_string_lossy() {
                p if p.contains(crate::launch::RETRODECK) => "RetroDECK's RetroArch",
                p if p.contains("org.libretro.RetroArch") => "RetroArch (Flatpak)",
                _ => "RetroArch",
            };
            rows.push(Row { label: label.into(), detail: path.display().to_string(), aside: String::new(), mark: in_use(chosen.as_ref() == Some(&path)) });
            choices.push(RetroArchChoice::Use(path));
        }
        rows.push(Row { label: "Another file…".into(), detail: "Walk this device's folders to a retroarch.cfg".into(), aside: String::new(), mark: None });
        choices.push(RetroArchChoice::Browse);
        let cursor = rows.iter().position(|r| r.mark.is_some()).unwrap_or(0);
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "RetroArch config".into(),
                note: "The retroarch.cfg save sync reads: it says where RetroArch keeps saves, states and BIOS files.".into(),
                rows,
                cursor,
            },
            then: Then::RetroArch { choices },
        });
        Task::none()
    }

    /// Read `config` from now on — once it has been read once, so a file that is not a RetroArch
    /// config is turned down here rather than at the next sync. None goes back to finding one.
    pub(super) fn set_retroarch_config(&mut self, config: Option<std::path::PathBuf>) -> Task<Message> {
        if let Some(path) = &config
            && let Err(why) = crate::sync::retroarch::locate(Some(path))
        {
            self.say(format!("{} can't be used: {why}", path.display()));
            return Task::none();
        }
        self.say(match &config {
            Some(path) => format!("Save sync reads {}.", path.display()),
            None => "Save sync finds RetroArch's config by itself.".into(),
        });
        self.settings.retroarch_config = config;
        self.write_settings();
        self.locate_saves()
    }

    /// What starts games: RetroDECK, or a command of the player's own.
    pub(super) fn pick_launcher(&mut self) -> Task<Message> {
        use crate::ui::picker::Row;
        let custom = self.settings.launch_command.as_deref().filter(|c| !c.is_empty());
        let in_use = |on: bool| on.then(|| "in use".to_string());
        let rows = vec![
            Row {
                label: "RetroDECK".into(),
                detail: "Uses the emulator you set up in RetroDECK for each system and game".into(),
                aside: if self.retrodeck { String::new() } else { "not installed".into() },
                mark: in_use(custom.is_none()),
            },
            Row {
                label: "Your own command…".into(),
                detail: custom.map_or_else(|| "One command for every game, such as RetroArch with a core".into(), crate::launch::join_command),
                aside: String::new(),
                mark: in_use(custom.is_some()),
            },
        ];
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Default emulator".into(),
                note: "Starts the games of every system that doesn't have its own emulator. \
                       Give a system its own under Emulator for each system."
                    .into(),
                rows,
                cursor: usize::from(custom.is_some()),
            },
            then: Then::Launcher,
        });
        Task::none()
    }

    /// A system's name as the library has it, by its ES-DE system name — `ps2` is "PlayStation 2".
    pub(super) fn system_label(&self, system: &str) -> String {
        self.library
            .systems
            .iter()
            .find(|s| crate::launch::es_system(&s.id) == system)
            .map_or_else(|| system.to_string(), |s| s.name.clone())
    }

    /// The default emulator, which starts the games of every system without its own — the one command
    /// for every game, or RetroDECK — and whether it can start anything at all.
    pub(super) fn default_emulator(&self) -> (String, bool) {
        match self.settings.launch_command.as_deref().filter(|c| !c.is_empty()) {
            Some(words) => (crate::launch::join_command(words), true),
            None => ("RetroDECK".into(), self.retrodeck),
        }
    }

    /// What a system without an emulator of its own starts its games with, said plainly — including
    /// when that is nothing.
    pub(super) fn uses_default(&self) -> String {
        match self.default_emulator() {
            (name, true) => format!("Uses the default emulator: {name}"),
            (name, false) => format!("No emulator yet: the default, {name}, isn't installed"),
        }
    }

    /// The library's systems, each with the emulator it starts with — its own, or the shared one — to
    /// pick one to change. A system with a command but no games left in the library is listed too, so
    /// the command can still be cleared.
    pub(super) fn pick_system_emulators(&mut self) -> Task<Message> {
        use crate::ui::picker::Row;
        let mut systems: Vec<(String, String)> = Vec::new();
        for system in &self.library.systems {
            let es = crate::launch::es_system(&system.id);
            if !systems.iter().any(|(s, _)| *s == es) {
                systems.push((es, system.name.clone()));
            }
        }
        for es in self.settings.system_commands.keys() {
            if !systems.iter().any(|(s, _)| s == es) {
                systems.push((es.clone(), es.clone()));
            }
        }
        let default = self.uses_default();
        let rows = systems
            .iter()
            .map(|(es, name)| {
                let own = self.settings.system_commands.get(es);
                Row {
                    label: name.clone(),
                    detail: own.map_or_else(|| default.clone(), |words| crate::launch::join_command(words)),
                    aside: String::new(),
                    mark: own.map(|_| "own emulator".to_string()),
                }
            })
            .collect();
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Emulator for each system".into(),
                note: "Assign a specific emulator per system."
                    .into(),
                rows,
                cursor: 0,
            },
            then: Then::SystemEmulators { systems: systems.into_iter().map(|(es, _)| es).collect() },
        });
        Task::none()
    }

    /// How one system's emulator is set: walked to, typed, or none of its own.
    pub(super) fn pick_emulator_for(&mut self, system: String) -> Task<Message> {
        use crate::ui::picker::Row;
        let own = self.settings.system_commands.get(&system).filter(|w| !w.is_empty()).cloned();
        let home = dirs::home_dir().unwrap_or_default();
        let applications = home.join("Applications");
        let name = self.system_label(&system);
        let mut rows = vec![Row {
            label: "Choose the emulator…".into(),
            detail: "Find a program or an AppImage on this device".into(),
            aside: String::new(),
            mark: None,
        }];
        let mut choices = vec![EmulatorChoice::Pick(if applications.is_dir() { applications } else { home.clone() })];
        // Where Flatpak puts a launcher for each app it installs — the emulators Discover installs.
        if let Some(exports) = [home.join(".local/share/flatpak/exports/bin"), "/var/lib/flatpak/exports/bin".into()].into_iter().find(|d| d.is_dir()) {
            rows.push(Row {
                label: "Choose a Flatpak emulator…".into(),
                detail: "One installed from Flathub, for instance through Discover".into(),
                aside: String::new(),
                mark: None,
            });
            choices.push(EmulatorChoice::Pick(exports));
        }
        rows.push(Row {
            label: "Type the command…".into(),
            detail: own.as_deref().map_or_else(|| "Write the whole command yourself".into(), crate::launch::join_command),
            aside: String::new(),
            mark: own.is_some().then(|| "in use".to_string()),
        });
        choices.push(EmulatorChoice::Type);
        let (default, works) = self.default_emulator();
        rows.push(Row {
            label: "Use RetroDeck".into(),
            detail: if works { default } else { format!("{default} is not installed, please install RetroDeck") },
            aside: String::new(),
            mark: own.is_none().then(|| "in use".to_string()),
        });
        choices.push(EmulatorChoice::Shared);
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: format!("Emulator for {name}"),
                note: format!(
                    "Choose how {name} games are started. When you choose an emulator, Deckhand fills in \
                     the usual options for it, and you can check the command before it's saved."
                ),
                rows,
                cursor: 0,
            },
            then: Then::SystemEmulator { system, choices },
        });
        Task::none()
    }

    /// The emulator walked to: made runnable, then its command offered to check before it is saved.
    pub(super) fn picked_emulator(&mut self, system: String, program: &std::path::Path) -> Task<Message> {
        let mut note = String::new();
        if program.to_string_lossy().to_lowercase().contains("retroarch") {
            note.push_str("RetroArch also needs a core: add -L and the path to the core's file, which ends in _libretro. ");
        }
        let words = crate::launch::preset(program);
        let task = self.type_system_command(system, crate::launch::join_command(&words), &note);
        // A downloaded AppImage arrives unable to run; picking it as the emulator is asking it to.
        if let (Err(why), Some(pending)) = (crate::folders::make_runnable(program), self.entry.as_mut()) {
            pending.entry.error = Some(format!("{} can't be made runnable: {why}", program.display()));
        }
        task
    }

    /// The command for `system`'s games, to type or check — `text` to start from, `note` ahead of
    /// what the field is for.
    pub(super) fn type_system_command(&mut self, system: String, text: String, note: &str) -> Task<Message> {
        let name = self.system_label(&system);
        let entry = crate::ui::entry::Entry::new(
            format!("Command for {name} games"),
            format!(
                "{note}This command starts every {name} game. Write it as you would in a terminal. {{rom}} is replaced \
                 by the game's file; if you leave it out, the file is added at the end. Leave the field empty to use \
                 the default emulator instead."
            ),
            text,
        );
        self.entry = Some(PendingEntry { entry, purpose: Typing::SystemCommand(system) });
        Task::none()
    }

    pub(super) fn type_launch_command(&mut self) -> Task<Message> {
        self.entry = Some(PendingEntry {
            entry: crate::ui::entry::Entry::new(
                "Default command",
                "This command starts every game, except systems that have their own emulator. Write it as you would \
                 in a terminal. {rom} is replaced by the game's file, and {system} by the system's short name, such \
                 as snes or ps2. If you leave out {rom}, the file is added at the end. For RetroArch, add -L and \
                 the core's file, so save sync knows which core runs the game. Leave the field empty to use \
                 RetroDECK instead.",
                self.settings.launch_command.as_deref().map(crate::launch::join_command).unwrap_or_default(),
            ),
            purpose: Typing::LaunchCommand,
        });
        Task::none()
    }

    /// The keys and buttons that type into a waiting text entry — every key, while it is up: what
    /// they would do on the page beneath is not what they were pressed for.
    pub(super) fn answer_entry(&mut self, message: &Message) -> Option<Task<Message>> {
        use crate::gamepad::Pad;
        use crate::ui::entry::{Control, Key};
        let entry = &mut self.entry.as_mut()?.entry;
        let control = match message {
            Message::Nav(direction) | Message::Pad(Pad::Nav(direction)) => {
                entry.step(*direction);
                None
            }
            Message::Pad(Pad::Confirm) => entry.press(entry.key()),
            Message::EntryKey(key) => {
                entry.focus(*key);
                entry.press(*key)
            }
            Message::Pad(Pad::Erase) => {
                entry.erase();
                None
            }
            Message::Pad(Pad::Search) => entry.press(Key::Control(Control::Shift)),
            Message::KeyChar(typed, true) if typed.eq_ignore_ascii_case("v") => Some(Control::Paste),
            Message::KeyChar(typed, false) => {
                if typed.contains(crate::search::BACKSPACE) {
                    entry.erase();
                } else {
                    entry.type_str(typed);
                }
                None
            }
            Message::Pasted(pasted) => {
                if let Some(pasted) = pasted {
                    entry.type_str(pasted.trim());
                }
                None
            }
            Message::Activate | Message::Pad(Pad::Start) => Some(Control::Done),
            Message::Dismiss | Message::Pad(Pad::Back) => {
                self.entry = None;
                return Some(Task::none());
            }
            // The rest of what the keys and the pad send is held back while the field is up.
            Message::KeyChar(..)
            | Message::Pad(_)
            | Message::CycleTab(_)
            | Message::CycleSystem(_)
            | Message::RemoveFocusedDownload => None,
            _ => return None,
        };
        Some(match control {
            Some(Control::Paste) => iced::clipboard::read().map(Message::Pasted),
            Some(Control::Done) => self.finish_entry(),
            _ => Task::none(),
        })
    }

    /// Save what the field holds, or say under it why it cannot be.
    pub(super) fn finish_entry(&mut self) -> Task<Message> {
        let Some(mut pending) = self.entry.take() else { return Task::none() };
        let text = pending.entry.text.trim().to_string();
        match pending.purpose.clone() {
            Typing::LoginUrl => {
                if let Screen::Login(state) = &mut self.screen {
                    state.url = text;
                    state.focus = crate::ui::login::Row::Qr;
                }
                Task::none()
            }
            Typing::LoginCode => {
                let task = self.update(Message::CodeChanged(text));
                if let Screen::Login(state) = &mut self.screen {
                    state.focus = crate::ui::login::Row::Connect;
                }
                task
            }
            Typing::LaunchCommand => match crate::launch::split_command(&text) {
                Err(why) => {
                    pending.entry.error = Some(format!("Not saved: {why}."));
                    self.entry = Some(pending);
                    Task::none()
                }
                Ok(words) => {
                    self.say(if words.is_empty() {
                        "The default emulator is now RetroDECK.".into()
                    } else {
                        format!("The default emulator is now: {}", crate::launch::join_command(&words))
                    });
                    self.settings.launch_command = (!words.is_empty()).then_some(words);
                    self.write_settings();
                    Task::none()
                }
            },
            Typing::SystemCommand(system) => match crate::launch::split_command(&text) {
                Err(why) => {
                    pending.entry.error = Some(format!("Not saved: {why}."));
                    self.entry = Some(pending);
                    Task::none()
                }
                Ok(words) => {
                    let name = self.system_label(&system);
                    self.say(if words.is_empty() {
                        format!("{name} games now use the default emulator.")
                    } else {
                        format!("{name} games now start with: {}", crate::launch::join_command(&words))
                    });
                    if words.is_empty() {
                        self.settings.system_commands.remove(&system);
                    } else {
                        self.settings.system_commands.insert(system, words);
                    }
                    self.write_settings();
                    Task::none()
                }
            },
        }
    }
}
