//! Pairing by QR code — see [`crate::qr_pairing`].

use super::*;

impl App {
    /// Ask RomM for a QR code to pair with, for the address typed.
    pub(super) fn start_qr(&mut self) -> Task<Message> {
        let Screen::Login(state) = &mut self.screen else { return Task::none() };
        if state.url.trim().is_empty() || state.url.trim() == "https://" {
            state.status = Some("Enter your RomM server address first.".into());
            state.focus = crate::ui::login::Row::Server;
            return Task::none();
        }
        state.qr_generation += 1;
        state.status = Some("Asking RomM for a code…".into());
        let (generation, url, insecure, client) = (state.qr_generation, state.url.clone(), state.insecure, self.client.clone());
        Task::perform(
            async move { crate::qr_pairing::start(&client, &url, insecure).await },
            move |result| Message::QrStarted(generation, result),
        )
    }

    /// Ask once more whether the QR code has been approved, after its interval.
    pub(super) fn poll_qr(&self) -> Task<Message> {
        let Screen::Login(state) = &self.screen else { return Task::none() };
        let Some(pairing) = state.qr.clone() else { return Task::none() };
        let (generation, client) = (state.qr_generation, self.client.clone());
        Task::perform(async move { crate::qr_pairing::poll(&client, &pairing).await }, move |poll| Message::QrPolled(generation, poll))
    }

    /// The pad on the login screen: Up and Down walk it, A presses or types into what the cursor is
    /// on, B leaves a waiting QR code. None for anything else.
    pub(super) fn login_pad(&mut self, message: &Message) -> Option<Task<Message>> {
        use crate::gamepad::Pad;
        use crate::ui::login::Row;
        let Screen::Login(state) = &mut self.screen else { return None };
        if state.connecting {
            return Some(Task::none());
        }
        match message {
            Message::Pad(Pad::Nav(direction)) => {
                if state.qr.is_none() {
                    match direction {
                        Direction::Up => state.focus = state.focus.step(-1),
                        Direction::Down => state.focus = state.focus.step(1),
                        _ => {}
                    }
                }
                Some(Task::none())
            }
            Message::Pad(Pad::Back) | Message::Dismiss if state.qr.is_some() => Some(self.update(Message::CancelQr)),
            Message::Pad(Pad::Confirm) if state.qr.is_none() => Some(match state.focus {
                Row::Server => {
                    self.entry = Some(PendingEntry {
                        entry: crate::ui::entry::Entry::new("Server address", "Where your RomM is: its address as the browser shows it, such as https://romm.example.com or 192.168.1.20:8080.", state.url.clone()),
                        purpose: Typing::LoginUrl,
                    });
                    Task::none()
                }
                Row::Qr => self.start_qr(),
                Row::Code => {
                    self.entry = Some(PendingEntry {
                        entry: crate::ui::entry::Entry::new("Pairing code", "The 8-character code RomM's Pair Device shows.", state.code.clone()),
                        purpose: Typing::LoginCode,
                    });
                    Task::none()
                }
                Row::Insecure => {
                    state.insecure = !state.insecure;
                    Task::none()
                }
                Row::Connect if state.complete() => self.connect(),
                Row::Connect => {
                    state.status = Some("Enter the server address and the pairing code first.".into());
                    Task::none()
                }
            }),
            Message::Pad(_) => Some(Task::none()),
            _ => None,
        }
    }

    /// Sign out of RomM: this device forgets the pairing, and goes back to pairing.
    pub(super) fn sign_out(&mut self) -> Task<Message> {
        // Local only: the token is dropped here, not revoked on the server. Revoking is a
        // remote, irreversible act that needs its own confirmation, and this build has no
        // screen to host that conversation.
        self.settings.connection = None;
        // The downloads stop being this session's: every queue item and every record is
        // keyed to the server it belongs to, so they are hidden rather than thrown away —
        // the files are real, and signing back in has to find them again.
        self.downloads.set_host("");
        let _ = self.save_settings();
        // The cached library belongs to the pairing being dropped. Leaving it would hand
        // the next sign-in someone else's shelves for the frame before the sync lands.
        cache::clear();
        self.system_counts.clear();
        self.library_read_at = None;
        self.installs = Installs::Unknown;
        self.install_requests.clear();
        self.library = Arc::new(Library::default());
        self.collections.clear();
        self.collection_sources = None;
        self.rebuild_collections();
        self.shelves.clear();
        self.covers = Covers::default();
        self.backdrop = Default::default();
        self.awarded = None;
        self.say("Signed out locally. The token is still valid in RomM — revoke it there. Downloaded games are left on disk.".into());
        self.screen = Screen::Login(LoginState::new("https://".into()));
        Task::none()
    }
}
