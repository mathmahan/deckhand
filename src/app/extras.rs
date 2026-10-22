//! A game's extras: what RomM keeps beside it — a manual, a hack, a translation — and puts on this
//! device only when asked. Its update and DLC are not among them: those come with the game (see
//! [`crate::romm::models::Part`]).

use super::*;
use crate::romm::models::{Part, RomDetail};

impl App {
    /// The build of `rom_id`'s game on this device whose extras its page offers: the tile's own when
    /// it is here, else the first build that is. Only a build in a folder of its own has any — RomM
    /// files an extra in a folder below the game's.
    pub(super) fn extras_build(&self, rom_id: i64) -> Option<&downloads::Installed> {
        self.builds_of(rom_id)
            .into_iter()
            .filter_map(|id| self.downloads.installed(id))
            .find(|record| record.files.iter().any(|f| f.contains('/')))
    }

    /// The extras RomM holds of the build of `rom_id`'s game on this device, as its page offers them:
    /// each with where it goes, and whether it is here. Empty until RomM has said.
    fn extras_of(&self, rom_id: i64) -> Vec<Extra> {
        let Some(record) = self.extras_build(rom_id) else { return Vec::new() };
        let Some(detail) = self.rom_files.get(&record.rom_id) else { return Vec::new() };
        extras(detail, record)
    }

    /// Whether the page of `rom_id`'s game has extras to offer — see [`crate::ui::detail::Action::Extras`].
    pub(super) fn has_extras(&self, rom_id: i64) -> bool {
        !self.extras_of(rom_id).is_empty()
    }

    /// The actions the page on screen draws, in order — what its cursor moves along.
    pub(super) fn detail_actions(&self) -> Vec<crate::ui::detail::Action> {
        let extras = match &self.screen {
            Screen::Browsing(Route::Detail { rom_id, .. }) => self.has_extras(*rom_id),
            _ => false,
        };
        crate::ui::detail::Action::shown(extras)
    }

    /// Ask RomM what the build of `rom_id`'s game on this device holds — every time its page opens,
    /// since an extra may have been added on RomM since. What was known stays shown meanwhile.
    pub(super) fn load_files(&self, rom_id: i64) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let Some(build) = self.extras_build(rom_id).map(|record| record.rom_id) else { return Task::none() };
        let client = self.client.clone();
        Task::perform(
            async move { client.fetch_rom_detail(&connection, build).await.map(Box::new).map_err(|e| e.to_string()) },
            move |result| Message::FilesLoaded(build, result),
        )
    }

    /// RomM's answer about a build's files. The Extras button may come in ahead of where the cursor
    /// is, so the cursor stays on the action it was on rather than its place in the row.
    pub(super) fn files_loaded(&mut self, build: i64, result: Result<Box<RomDetail>, String>) -> Task<Message> {
        // A failed ask leaves what was known: the button is a convenience, not worth a banner.
        let Ok(detail) = result else { return Task::none() };
        let before = self.detail_actions();
        self.rom_files.insert(build, Arc::from(detail));
        let after = self.detail_actions();
        if let Some(at) = before.get(self.detail_focus.action).and_then(|action| after.iter().position(|a| a == action)) {
            self.detail_focus.action = at;
        }
        Task::none()
    }

    /// Extras on the game's page: the picker of what RomM keeps beside the build here.
    pub(super) fn press_extras(&mut self, rom_id: i64) -> Task<Message> {
        let Some(build) = self.extras_build(rom_id).map(|record| record.rom_id) else { return Task::none() };
        let extras = self.extras_of(rom_id);
        let title = self.title_of(rom_id);
        if extras.is_empty() {
            self.say(format!("RomM keeps nothing beside \"{title}\"."));
            return Task::none();
        }
        let rows = extras
            .iter()
            .map(|extra| crate::ui::picker::Row {
                label: extra.label.clone(),
                detail: extra.name.clone(),
                aside: downloads::human_bytes(extra.bytes),
                mark: extra.here.then(|| "on this device".to_string()),
            })
            .collect();
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: format!("Extras of \"{title}\""),
                note: "What RomM keeps beside the game. Picking one puts it on this device, in its folder beside the game — or takes it off again, when it's here.".into(),
                rows,
                cursor: 0,
            },
            then: Then::Extras { rom_id: build, extras },
        });
        crate::ui::picker::scroll_to(0)
    }

    /// The extra picked: fetched into the build here, or taken off this device when it is here.
    pub(super) fn pick_extra(&mut self, rom_id: i64, extra: &Extra) -> Task<Message> {
        let Some(record) = self.downloads.installed(rom_id).cloned() else { return Task::none() };
        if extra.here {
            let (ids, paths, label) = (vec![extra.id], vec![extra.path.clone()], extra.label.clone());
            return Task::perform(
                async move { off_thread(move || downloads::shed(&record, &ids, &paths)).await.map(Box::new) },
                move |result| Message::ExtraRemoved(rom_id, label, result),
            );
        }
        self.enqueue_item(QueueItem {
            host: String::new(),
            rom_id,
            system_id: record.system_id.clone(),
            title: record.title.clone(),
            files: Some(vec![extra.id]),
            label: Some(extra.label.clone()),
            bytes: Some(extra.bytes),
            build: record.build.clone(),
            stamp: record.stamp.clone(),
            extra: true,
        })
    }

    /// An extra is off this device: the build's record without it.
    pub(super) fn extra_removed(&mut self, rom_id: i64, label: String, result: Result<Box<downloads::Installed>, String>) -> Task<Message> {
        let title = self.title_of(rom_id);
        match result {
            Ok(record) => {
                self.downloads.replace(*record);
                self.say(format!("\"{title}\" — {label} is off this device."));
            }
            Err(why) => self.say(format!("\"{title}\" — {label} could not be removed: {why}")),
        }
        Task::none()
    }
}

/// The extras of `detail` — every file that doesn't come with the game — as `record`'s game page
/// offers them.
fn extras(detail: &RomDetail, record: &downloads::Installed) -> Vec<Extra> {
    let places = downloads::layout::Places::of(detail);
    detail
        .files
        .iter()
        .filter(|f| f.part() == Part::Extra)
        .map(|f| {
            let path = places.path(f);
            Extra { id: f.id, here: record.files.contains(&path), path, label: f.category_label(), name: f.file_name.clone(), bytes: f.size() }
        })
        .collect()
}
