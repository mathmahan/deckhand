//! Offering RetroDECK to a device without it, and seeing the install through — see
//! [`crate::retrodeck`].
//!
//! Only ever once paired: RetroDECK is how Deckhand plays what RomM holds, and without a RomM there
//! is nothing to play. It is offered once a pairing has brought the library in, when a game cannot
//! start for want of an emulator, and in Settings ▸ Playing ▸ Default emulator.

use super::*;
use crate::retrodeck::{Event, Look, Presence, Scope, Stop};

impl App {
    /// Find out where RetroDECK stands, then offer what there is to offer — `offering` says why it
    /// is asked. Nothing at all without a pairing.
    pub(super) fn look_for_retrodeck(&mut self, offering: Offering) -> Task<Message> {
        if self.settings.connection.is_none() || self.retrodeck_setup != RetroDeckSetup::Idle {
            return Task::none();
        }
        // A fixture library plays nothing, and its device is not the one to install on.
        if !self.writes_settings {
            let look = Look { presence: if self.retrodeck { Presence::Ready } else { Presence::Missing }, scope: None, free: None };
            return self.update(Message::RetroDeckLooked(offering, look));
        }
        Task::perform(off_thread(crate::retrodeck::look), move |look| match look {
            Ok(look) => Message::RetroDeckLooked(offering, look),
            Err(_) => Message::RetroDeckLooked(offering, Look { presence: Presence::Missing, scope: None, free: None }),
        })
    }

    /// The offer itself: to install RetroDECK, or to finish setting up the one here.
    pub(super) fn offer_retrodeck(&mut self, offering: Offering, look: Look) -> Task<Message> {
        use crate::ui::picker::Row;
        if self.settings.connection.is_none() || self.retrodeck_setup != RetroDeckSetup::Idle {
            return Task::none();
        }
        self.retrodeck = look.presence == Presence::Ready;
        let later = Row {
            label: "Not now".into(),
            detail: "Settings ▸ Playing ▸ Default emulator offers it again".into(),
            aside: String::new(),
            mark: None,
        };
        let (title, note, yes) = match (look.presence, look.scope) {
            (Presence::Ready, _) => {
                if offering == Offering::Asked {
                    self.say("RetroDECK is installed and set up.".into());
                }
                return Task::none();
            }
            (Presence::NotSetUp, _) => (
                "Set up RetroDECK?",
                "RetroDECK is installed, but hasn't been set up yet. It opens for its first-run setup, where \
                 you choose where it keeps its files. Close it once that's done, and Deckhand takes it from there."
                    .to_string(),
                Row { label: "Open RetroDECK".into(), detail: "Its first-run setup".into(), aside: String::new(), mark: None },
            ),
            (Presence::Missing, None) => {
                if offering != Offering::Pairing {
                    self.say("Flatpak isn't on this device, so Deckhand can't install RetroDECK. Choose an emulator of your own in Settings ▸ Playing.".into());
                }
                return Task::none();
            }
            (Presence::Missing, Some(scope)) => {
                let room = match look.free {
                    Some(free) if free < crate::retrodeck::NEEDED_BYTES => format!(
                        " Only {} is free where it goes, and it needs about {}.",
                        downloads::human_bytes(free),
                        downloads::human_bytes(crate::retrodeck::NEEDED_BYTES)
                    ),
                    _ => String::new(),
                };
                (
                    "Install RetroDECK?",
                    format!(
                        "Deckhand starts your games through RetroDECK, which isn't on this device yet. It comes from \
                         Flathub: about {} to download and {} installed, plus its runtime if that isn't here.{room} \
                         Once it's in, it opens for its first-run setup.",
                        downloads::human_bytes(crate::retrodeck::DOWNLOAD_BYTES),
                        downloads::human_bytes(crate::retrodeck::INSTALLED_BYTES),
                    ),
                    Row {
                        label: "Install RetroDECK".into(),
                        detail: match scope {
                            Scope::System => "For everyone on this device, as Discover installs it".into(),
                            Scope::User => "For you alone, with Flathub added for you".into(),
                        },
                        aside: look.free.map(|free| format!("{} free", downloads::human_bytes(free))).unwrap_or_default(),
                        mark: None,
                    },
                )
            }
        };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker { title: title.into(), note, rows: vec![yes, later], cursor: 0 },
            then: Then::RetroDeck(look.scope),
        });
        iced::widget::operation::scroll_to(crate::ui::picker::list_id(), iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 })
    }

    /// Install RetroDECK into `scope`, then open it for its first-run setup. Its progress shows in
    /// Settings ▸ Playing ▸ Default emulator, which is also where it is stopped.
    pub(super) fn install_retrodeck(&mut self, scope: Scope) -> Task<Message> {
        if self.retrodeck_setup != RetroDeckSetup::Idle {
            return Task::none();
        }
        let cancel = crate::retrodeck::Cancel::default();
        self.retrodeck_cancel = Some(cancel.clone());
        self.retrodeck_setup = RetroDeckSetup::Installing(None);
        self.say("Installing RetroDECK — Settings ▸ Playing ▸ Default emulator shows how far it has got.".into());
        Task::run(
            iced::stream::channel(8, move |mut output: Sender<Event>| async move {
                let mut progress = output.clone();
                let mut setting_up = output.clone();
                let done = off_thread(move || {
                    // Dropped rather than waited for when the window is behind: only the latest counts.
                    crate::retrodeck::install(scope, &cancel, |p| {
                        let _ = progress.try_send(Event::Progress(p));
                    })?;
                    let _ = setting_up.try_send(Event::SettingUp);
                    crate::retrodeck::set_up()
                })
                .await
                .unwrap_or_else(|why| Err(Stop::Failed(why)));
                let _ = output.send(Event::Done(done)).await;
            }),
            Message::RetroDeckEvent,
        )
    }

    /// Open the RetroDECK that is here for its first-run setup.
    pub(super) fn set_up_retrodeck(&mut self) -> Task<Message> {
        if self.retrodeck_setup != RetroDeckSetup::Idle {
            return Task::none();
        }
        let _ = self.update(Message::RetroDeckEvent(Event::SettingUp));
        Task::perform(off_thread(crate::retrodeck::set_up), |done| {
            Message::RetroDeckEvent(Event::Done(done.unwrap_or_else(|why| Err(Stop::Failed(why)))))
        })
    }

    pub(super) fn on_retrodeck_event(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Progress(progress) => {
                if matches!(self.retrodeck_setup, RetroDeckSetup::Installing(_)) {
                    self.retrodeck_setup = RetroDeckSetup::Installing(Some(progress));
                }
                Task::none()
            }
            Event::SettingUp => {
                self.retrodeck_setup = RetroDeckSetup::SettingUp;
                self.retrodeck_cancel = None;
                self.say("RetroDECK is open for its first-run setup. Close it once that's done.".into());
                Task::none()
            }
            Event::Done(done) => {
                self.retrodeck_setup = RetroDeckSetup::Idle;
                self.retrodeck_cancel = None;
                match done {
                    Ok(Presence::Ready) => {
                        self.retrodeck = true;
                        self.say("RetroDECK is ready: games start through it now.".into());
                        if !self.hooks.as_ref().is_some_and(|s| s.describe().0) {
                            self.offer_hooks();
                        }
                        // Back to the front from RetroDECK. A compositor may refuse; then it is one
                        // click away.
                        iced::window::latest().and_then(iced::window::gain_focus)
                    }
                    Ok(_) => {
                        self.say(
                            "RetroDECK closed before its setup was done. Settings ▸ Playing ▸ Default emulator opens it again."
                                .into(),
                        );
                        iced::window::latest().and_then(iced::window::gain_focus)
                    }
                    Err(Stop::Cancelled) => {
                        self.say("RetroDECK's install was stopped.".into());
                        Task::none()
                    }
                    Err(Stop::Failed(why)) => {
                        self.say(format!("RetroDECK could not be installed: {why}"));
                        Task::none()
                    }
                }
            }
        }
    }

    /// Ask whether to stop RetroDECK's install, which a press on its row in Settings does.
    pub(super) fn ask_to_stop_retrodeck(&mut self) -> Task<Message> {
        use crate::ui::picker::Row;
        let row = |label: &str, detail: &str| Row { label: label.into(), detail: detail.into(), aside: String::new(), mark: None };
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Stop installing RetroDECK?".into(),
                note: "What it has downloaded so far is kept by Flatpak, so installing it again carries on from there.".into(),
                rows: vec![row("Stop the install", ""), row("Keep installing", "")],
                cursor: 1,
            },
            then: Then::StopRetroDeck,
        });
        Task::none()
    }

    pub(super) fn stop_retrodeck(&mut self) {
        if let Some(cancel) = &self.retrodeck_cancel {
            cancel.cancel();
        }
    }

    /// With RetroDECK ready, games started from ES-DE can sync too — the hooks Settings installs.
    fn offer_hooks(&mut self) {
        use crate::ui::picker::Row;
        self.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker {
                title: "Sync games started from ES-DE?".into(),
                note: "Games started from RetroDECK's own ES-DE skip Deckhand. A pair of ES-DE hooks syncs their \
                       saves with RomM all the same, and sends their play time."
                    .into(),
                rows: vec![
                    Row { label: "Install ES-DE's hooks".into(), detail: "Settings ▸ Saves ▸ ES-DE launches takes them off again".into(), aside: String::new(), mark: None },
                    Row { label: "Not now".into(), detail: String::new(), aside: String::new(), mark: None },
                ],
                cursor: 0,
            },
            then: Then::EsDeHooks,
        });
    }

    /// How the Default emulator row reads RetroDECK's install, while one runs.
    pub(super) fn retrodeck_progress(&self) -> Option<String> {
        match self.retrodeck_setup {
            RetroDeckSetup::Idle => None,
            RetroDeckSetup::Installing(progress) => Some(match progress {
                Some(p) if p.of > 1 => format!("RetroDECK · installing {} of {} · {}%", p.step, p.of, p.percent),
                Some(p) => format!("RetroDECK · installing · {}%", p.percent),
                None => "RetroDECK · starting the install".into(),
            }),
            RetroDeckSetup::SettingUp => Some("RetroDECK · open for its setup — close it when it's done".into()),
        }
    }
}
