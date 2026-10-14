//! Deckhand's own updates — see [`crate::release`].

use super::*;

impl App {
    /// Ask the repository for its newest release — on Forgejo, when there is a token to ask with.
    pub(super) fn check_release(&mut self) -> Task<Message> {
        use crate::release::Status;
        let token = self.settings.release_token.as_deref().map(str::trim).unwrap_or_default().to_string();
        if token.is_empty() && crate::release::needs_token(self.release_repository) {
            self.release = Status::Off;
            return Task::none();
        }
        match self.release {
            Status::Checking | Status::Installing(..) | Status::Installed(_) => return Task::none(),
            // Asked again behind what is on show: a newer version stays offered meanwhile.
            Status::Out(_) => {}
            _ => self.release = Status::Checking,
        }
        let api = crate::release::latest_url(self.release_repository);
        Task::perform(async move { crate::release::check(&api, &token).await }, Message::ReleaseChecked)
    }

    pub(super) fn on_release_checked(&mut self, result: Result<crate::release::Release, String>) -> Task<Message> {
        use crate::release::Status;
        // An answer that was in flight does not undo an install under way, or done.
        if matches!(self.release, Status::Installing(..) | Status::Installed(_)) {
            return Task::none();
        }
        self.release = match (result, std::mem::take(&mut self.release)) {
            (Ok(release), _) if release.is_newer() => {
                // Said once a version, not every six hours.
                if self.release_announced.as_deref() != Some(release.version.as_str()) {
                    self.release_announced = Some(release.version.clone());
                    if self.banner.is_none() {
                        self.say(self.release_news(&release));
                    }
                }
                Status::Out(Box::new(release))
            }
            (Ok(_), _) => Status::Current,
            // Asking again and failing changes nothing about what is out.
            (Err(_), Status::Out(release)) => Status::Out(release),
            (Err(why), _) => Status::Failed(why, None),
        };
        Task::none()
    }

    /// What the banner says of a new version: where to get it.
    pub(super) fn release_news(&self, release: &crate::release::Release) -> String {
        if self.appimage.is_some() && release.appimage.is_some() {
            format!("Deckhand {} is out — Settings ▸ About ▸ Updates installs it.", release.version)
        } else {
            format!("Deckhand {} is out — {}", release.version, crate::release::releases_page(self.release_repository))
        }
    }

    /// Confirm on the Updates row: ask, install or restart, whichever is next.
    pub(super) fn press_updates(&mut self) -> Task<Message> {
        use crate::release::Status;
        match self.release.clone() {
            // Asking needs a token; pressing the row is where it is given.
            Status::Off => self.type_release_token(),
            Status::Current | Status::Failed(_, None) => self.check_release(),
            Status::Out(release) | Status::Failed(_, Some(release)) => self.install_release(release),
            Status::Checking | Status::Installing(..) => Task::none(),
            Status::Installed(_) => self.restart(),
        }
    }

    /// Fetch a release's AppImage and put it in place of the one running.
    pub(super) fn install_release(&mut self, release: Box<crate::release::Release>) -> Task<Message> {
        use crate::release::{Event, Status};
        let Some(target) = self.appimage.clone() else {
            self.say(format!(
                "Deckhand {} is out. This copy is not an AppImage, so it is updated by hand: {}",
                release.version,
                crate::release::releases_page(self.release_repository)
            ));
            return Task::none();
        };
        if release.appimage.is_none() {
            self.say(format!(
                "Deckhand {} has no AppImage for this machine: {}",
                release.version,
                crate::release::releases_page(self.release_repository)
            ));
            return Task::none();
        }
        let token = self.settings.release_token.clone().unwrap_or_default();
        self.release = Status::Installing(release.clone(), 0.0);
        let api = crate::release::latest_url(self.release_repository);
        Task::run(
            iced::stream::channel(8, move |mut output: Sender<Event>| async move {
                let mut progress = output.clone();
                let result = crate::release::install(&api, token.trim(), &release, &target, |fraction| {
                    // Dropped rather than awaited when the UI is behind: only the latest one matters.
                    let _ = progress.try_send(Event::Progress(fraction));
                })
                .await;
                let _ = output.send(Event::Done(result)).await;
            }),
            Message::ReleaseEvent,
        )
    }

    pub(super) fn on_release_event(&mut self, event: crate::release::Event) -> Task<Message> {
        use crate::release::{Event, Status};
        let Status::Installing(release, _) = &self.release else { return Task::none() };
        let release = release.clone();
        match event {
            Event::Progress(fraction) => self.release = Status::Installing(release, fraction),
            Event::Done(Ok(())) => {
                self.say(format!(
                    "Deckhand {} is in place. Restart it from Settings ▸ About ▸ Updates, or it starts next time.",
                    release.version
                ));
                self.release = Status::Installed(release);
            }
            Event::Done(Err(why)) => {
                self.say(format!("Deckhand {} could not be installed: {why}", release.version));
                self.release = Status::Failed(why, Some(release));
            }
        }
        Task::none()
    }

    /// Start the version just installed and close this one — unless that would cut something short.
    pub(super) fn restart(&mut self) -> Task<Message> {
        let busy = if let Some(rom_id) = self.playing {
            Some(format!("\"{}\" is still running — restart once it is done.", self.title_of(rom_id)))
        } else if matches!(self.save_sync, SaveSync::Running) {
            Some("A save sync is running — restart once it is done.".to_string())
        } else if self.downloads.working() {
            Some("A download is running — restart once it is done, or it starts over.".to_string())
        } else {
            None
        };
        if let Some(busy) = busy {
            self.say(busy);
            return Task::none();
        }
        let Some(target) = self.appimage.clone() else { return Task::none() };
        match crate::release::relaunch(&target) {
            Ok(()) => iced::exit(),
            Err(e) => {
                self.say(format!("The new version could not be started: {e}"));
                Task::none()
            }
        }
    }

    /// The keys and buttons that answer a waiting conflict: Left and Right pick a side, confirm sends
    /// it, back keeps the newer. None for anything else, which goes on as usual.
    pub(super) fn answer_conflict(&mut self, message: &Message) -> Option<Task<Message>> {
        use crate::gamepad::Pad;
        use crate::sync::Choice;
        let pending = self.conflict.as_mut()?;
        let pick = |keep: Choice| if keep == Choice::Local { Choice::Remote } else { Choice::Local };
        let send = match message {
            Message::Nav(Direction::Left | Direction::Right) | Message::Pad(Pad::Nav(Direction::Left | Direction::Right)) => {
                pending.keep = pick(pending.keep);
                return Some(Task::none());
            }
            Message::Activate | Message::Pad(Pad::Confirm) => pending.keep,
            Message::Dismiss | Message::Pad(Pad::Back) => pending.conflict.newer,
            Message::ConflictChosen(choice) => *choice,
            _ => return None,
        };
        let pending = self.conflict.take()?;
        pending.reply.send(send);
        let title = self.title_of(pending.rom_id);
        let side = if send == Choice::Local { "this device's" } else { "RomM's" };
        self.say(format!("\"{title}\": keeping {side} {}.", pending.conflict.name));
        Some(Task::none())
    }
}
