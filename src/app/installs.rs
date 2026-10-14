//! Installs asked of this device from RomM's web app: *Install on device* there, a download here.
//!
//! RomM 5.4 queues a rom for any device that says it takes them, and the device claims what is
//! waiting, downloads it, and says how it went. Claiming is a poll, every [`EVERY`] while the
//! window is open, and it is also what shows the device as online there. The first claim RomM
//! answers is what this device says it takes installs on: a device that never could would only be
//! a name in that list that nothing ever comes to.
//!
//! Two things keep a device from claiming. RomM lets only a token it tied to the device claim, and
//! only a QR pairing ties one — a token paired by code is answered 403, which Settings ▸ *This
//! device* turns into how to fix it. And a RomM before 5.4, or with installs turned off, answers
//! 404. Either way it is not asked again until the window opens next.
//!
//! What comes is downloaded the way Download fetches it here: the whole of that build. A request is
//! answered once its download ends, or at once when the build is already on this device; one called
//! off in RomM while its download is still on the way stops that download.

use std::collections::HashSet;
use std::time::Duration;

use super::*;
use crate::romm::models::{InstallRequest, RomDetail};
use crate::romm::Outcome;

/// How often RomM is asked: a third of the 90 seconds a claim shows the device as online for.
pub(super) const EVERY: Duration = Duration::from_secs(30);

/// An install RomM asked of this device and this one took, not yet answered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Taken {
    pub rom_id: i64,
    /// Its download was started for it, rather than already under way when it came: only then is
    /// it this request's to stop.
    pub started: bool,
}

impl App {
    /// Ask RomM for the installs waiting for this device, unless one ask is already out or RomM has
    /// said it will not hand any over.
    pub(super) fn claim_installs(&mut self) -> Task<Message> {
        if self.claiming || self.watch_lock.is_none() || !self.installs.taking() {
            return Task::none();
        }
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let Some(device) = connection.device_id.clone() else { return Task::none() };
        self.claiming = true;
        let client = self.client.clone();
        Task::perform(async move { client.claim_installs(&connection, &device).await }, Message::InstallsClaimed)
    }

    /// What RomM holds taken by this device: take what is new, and stop what RomM has called off.
    pub(super) fn on_installs_claimed(&mut self, result: Result<Vec<InstallRequest>, ApiError>) -> Task<Message> {
        self.claiming = false;
        let taken = match result {
            Ok(taken) => taken,
            Err(e) => {
                self.installs = match e.status {
                    403 => Installs::NotTied,
                    404 | 405 => Installs::Unavailable,
                    // Out of reach for now: asked again on the next tick.
                    _ => self.installs,
                };
                return Task::none();
            }
        };
        let mut tasks = Vec::new();
        if self.installs != Installs::Taking {
            self.installs = Installs::Taking;
            tasks.push(self.accept_installs());
        }

        let live: HashSet<&str> = taken.iter().map(|r| r.id.as_str()).collect();
        let gone: Vec<(String, Taken)> =
            self.install_requests.iter().filter(|(id, _)| !live.contains(id.as_str())).map(|(id, t)| (id.clone(), *t)).collect();
        for (id, was) in gone {
            self.install_requests.remove(&id);
            let still_wanted = self.install_requests.values().any(|t| t.rom_id == was.rom_id);
            if was.started && !still_wanted && self.downloads.is_queued(was.rom_id) {
                let title = self.title_of(was.rom_id);
                tasks.push(self.remove_download(was.rom_id));
                self.say(format!("\"{title}\" was called off in RomM, so it won't be downloaded."));
            }
        }

        for request in taken {
            if !self.install_requests.contains_key(&request.id) {
                tasks.push(self.take_install(request));
            }
        }
        Task::batch(tasks)
    }

    /// Start on one install RomM asked for.
    fn take_install(&mut self, request: InstallRequest) -> Task<Message> {
        let rom_id = request.rom_id;
        if self.downloads.is_queued(rom_id) {
            // Already on its way, asked for here: answered when it lands, but not this request's to stop.
            self.install_requests.insert(request.id, Taken { rom_id, started: false });
            return Task::none();
        }
        if self.downloads.installed(rom_id).is_some_and(|i| i.subset.is_none()) {
            return self.report_install(request.id, Outcome::AlreadyInstalled);
        }
        let Some(game) = self.game_of(rom_id).cloned() else {
            // Not in the library as this device last read it — added to RomM since, most likely.
            // Held while RomM is asked what it is, so the next claim does not take it again.
            self.install_requests.insert(request.id.clone(), Taken { rom_id, started: false });
            let Some(connection) = self.settings.connection.clone() else { return Task::none() };
            let client = self.client.clone();
            return Task::perform(
                async move { client.fetch_rom_detail(&connection, rom_id).await.map(Box::new).map_err(|e| e.to_string()) },
                move |result| Message::InstallDetail(request.clone(), result),
            );
        };
        let installed = self.downloads.installed_builds(game.builds());
        let whole = downloads::choice::choices(&game, None, &installed).into_iter().find(|c| c.rom_id == rom_id && c.files.is_none());
        let Some(choice) = whole else {
            return self.report_install(request.id, Outcome::Failed("RomM has no file for it on its disk".into()));
        };
        self.install_requests.insert(request.id, Taken { rom_id, started: true });
        let task = self.enqueue_choice(&game, &choice);
        self.say(format!("RomM sent \"{}\" to this device — downloading it.", game.title));
        task
    }

    /// RomM's word on a rom it asked for that the library does not have: downloaded from that.
    pub(super) fn on_install_detail(&mut self, request: InstallRequest, result: Result<Box<RomDetail>, String>) -> Task<Message> {
        // Called off in RomM while it was being asked about.
        if !self.install_requests.contains_key(&request.id) {
            return Task::none();
        }
        let detail = match result {
            Ok(detail) => detail,
            Err(why) => {
                self.install_requests.remove(&request.id);
                return self.report_install(request.id, Outcome::Failed(format!("RomM could not say what it is: {why}")));
            }
        };
        let title = detail.name.clone().or_else(|| detail.fs_name.clone()).unwrap_or_else(|| format!("Game #{}", detail.id));
        let Some(system_id) = detail.platform_slug.clone() else {
            self.install_requests.remove(&request.id);
            return self.report_install(request.id, Outcome::Failed("RomM did not say which system it is for".into()));
        };
        self.install_requests.insert(request.id, Taken { rom_id: request.rom_id, started: true });
        let task = self.enqueue_item(QueueItem {
            host: String::new(),
            rom_id: request.rom_id,
            system_id,
            title: title.clone(),
            files: None,
            label: None,
            bytes: detail.fs_size_bytes.and_then(|b| u64::try_from(b).ok()),
            build: None,
            stamp: detail.stamp(),
        });
        self.say(format!("RomM sent \"{title}\" to this device — downloading it."));
        task
    }

    /// Tell RomM how every install it asked for of `rom_id` ended — a download finished, failed or
    /// was cancelled here.
    pub(super) fn answer_installs(&mut self, rom_id: i64, outcome: Outcome) -> Task<Message> {
        let answered: Vec<String> = self.install_requests.iter().filter(|(_, t)| t.rom_id == rom_id).map(|(id, _)| id.clone()).collect();
        let tasks: Vec<_> = answered
            .into_iter()
            .map(|id| {
                self.install_requests.remove(&id);
                self.report_install(id, outcome.clone())
            })
            .collect();
        Task::batch(tasks)
    }

    /// One answer to RomM. Not waited on and not retried: a request it does not hear about stays
    /// taken, and the next claim hands it back — to be answered as already installed, or fetched again.
    fn report_install(&self, request: String, outcome: Outcome) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let Some(device) = connection.device_id.clone() else { return Task::none() };
        let client = self.client.clone();
        Task::future(async move {
            let _ = client.report_install(&connection, &device, &request, &outcome).await;
        })
        .discard()
    }

    /// Have RomM offer this device under *Install on device*.
    fn accept_installs(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else { return Task::none() };
        let Some(device) = connection.device_id.clone() else { return Task::none() };
        let client = self.client.clone();
        Task::future(async move {
            let _ = client.accept_installs(&connection, &device).await;
        })
        .discard()
    }
}
