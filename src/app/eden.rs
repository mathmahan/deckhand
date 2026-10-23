//! Offering Eden, for Switch games, to a device with no Switch emulator, and installing it — see
//! [`crate::launch::switch_emulator`]. RetroDECK has no Switch emulator and cannot start one, so a
//! Switch game needs one of its own. Eden comes from Flathub, the way RetroDECK does
//! ([`crate::retrodeck::install`]).
//!
//! It is offered when a Switch game cannot start for want of one, and in Settings ▸ Playing ▸
//! Emulator for each system ▸ Switch. It is small next to RetroDECK, so its progress shows in the
//! banner, and the game that asked for it starts once it is in.

use super::*;
use crate::retrodeck::{Progress, Scope, Stop};

/// About what Eden downloads from Flathub — its runtime aside, which RetroDECK's may already be.
pub const EDEN_DOWNLOAD_BYTES: u64 = 12_400_000;

impl App {
    /// Find out where Eden would go, then offer it. `play` is the game to start once it is in.
    pub(super) fn look_for_eden(&mut self, play: Option<i64>) -> Task<Message> {
        if self.eden_install.is_some() {
            return Task::none();
        }
        // A fixture library's device is not the one to install on.
        if !self.writes_settings {
            return self.update(Message::EdenLooked(play, Some(Scope::User)));
        }
        Task::perform(off_thread(crate::retrodeck::scope), move |scope| Message::EdenLooked(play, scope.ok().flatten()))
    }

    /// The offer itself, where Flatpak can put Eden.
    pub(super) fn offer_eden(&mut self, play: Option<i64>, scope: Option<Scope>) -> Task<Message> {
        use crate::ui::picker::Row;
        let Some(scope) = scope else {
            self.say(
                "Flatpak isn't on this device, so Deckhand can't install Eden. Choose a Switch emulator in \
                 Settings ▸ Playing ▸ Emulator for each system."
                    .into(),
            );
            return Task::none();
        };
        let then = if play.is_some() { " The game starts once it's in." } else { "" };
        let note = format!(
            "Switch games play in Eden, which isn't on this device yet. RetroDECK has no Switch emulator. Eden \
             comes from Flathub: about {} to download, plus its runtime if that isn't here. Your keys and firmware \
             go in from RomM when a game starts.{then}",
            downloads::human_bytes(EDEN_DOWNLOAD_BYTES)
        );
        let yes = Row::new(
            "Install Eden",
            match scope {
                Scope::System => "For everyone on this device, as Discover installs it",
                Scope::User => "For you alone, with Flathub added for you",
            },
        );
        let later = Row::new("Not now", "Settings ▸ Playing ▸ Emulator for each system offers it again");
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker { title: "Install Eden?".into(), note, rows: vec![yes, later], cursor: 0 },
            then: Then::Eden { scope, play },
        });
        crate::ui::picker::scroll_to(0)
    }

    /// Install Eden into `scope`, then start `play`, if a game asked for it.
    pub(super) fn install_eden(&mut self, scope: Scope, play: Option<i64>) -> Task<Message> {
        if self.eden_install.is_some() {
            return Task::none();
        }
        self.eden_install = Some(EdenInstall { play });
        self.say("Installing Eden…".into());
        Task::run(
            iced::stream::channel(8, move |mut output: Sender<EdenEvent>| async move {
                let mut progress = output.clone();
                let done = off_thread(move || {
                    // Dropped rather than waited for when the window is behind: only the latest counts.
                    crate::retrodeck::install(crate::launch::EDEN, scope, &crate::retrodeck::Cancel::default(), |p| {
                        let _ = progress.try_send(EdenEvent::Progress(p));
                    })
                })
                .await
                .unwrap_or_else(|why| Err(Stop::Failed(why)));
                let _ = output.send(EdenEvent::Done(done)).await;
            }),
            Message::EdenEvent,
        )
    }

    pub(super) fn on_eden_event(&mut self, event: EdenEvent) -> Task<Message> {
        match event {
            EdenEvent::Progress(progress) => {
                if self.eden_install.is_some() {
                    self.say(eden_progress(progress));
                }
                Task::none()
            }
            EdenEvent::Done(done) => {
                let play = self.eden_install.take().and_then(|install| install.play);
                match done {
                    Ok(()) if self.settings.system_commands.contains_key("switch") => {
                        self.say(
                            "Eden is installed. Switch games still start with your own command; Settings ▸ Playing ▸ \
                             Emulator for each system changes that."
                                .into(),
                        );
                        Task::none()
                    }
                    Ok(()) => {
                        self.say("Eden is installed: Switch games start in it now.".into());
                        play.map_or_else(Task::none, |rom_id| self.play(rom_id))
                    }
                    Err(Stop::Cancelled) => {
                        self.say("Eden's install was stopped.".into());
                        Task::none()
                    }
                    Err(Stop::Failed(why)) => {
                        self.say(format!("Eden could not be installed: {why}"));
                        Task::none()
                    }
                }
            }
        }
    }
}

/// How the banner reads Eden's install while it runs.
fn eden_progress(progress: Progress) -> String {
    if progress.of > 1 {
        format!("Installing Eden · {} of {} · {}%", progress.step, progress.of, progress.percent)
    } else {
        format!("Installing Eden · {}%", progress.percent)
    }
}
