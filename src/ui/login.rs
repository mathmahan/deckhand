//! First-launch pairing — a port of `browse/ui/LoginView.kt`.
//!
//! The server's address, then either of RomM's two ways to let a device in: a QR code a phone
//! scans to approve it ([`crate::qr_pairing`]), or the 8-character code RomM shows when linking one.
//! The fields are real text inputs for a keyboard and a pointer, and the whole screen also answers
//! the pad: Up and Down walk it, and A on a field opens the on-screen keyboard — so a Deck pairs
//! without a keyboard at all, by QR code, typing only the address.
//!
//! Drawn as Settings is, on Home's ground with the wordmark where the header has it: what this is
//! for where Home has its spotlight, and beside it the form as glass rows, the one under the pad's
//! cursor lifted on the ink ring. The library's loading screen after it is drawn the same way, so
//! pairing runs into the library without a change of look.
//!
//! Kept free of network code so the flow stays testable: [`LoginState`] only decides what is
//! typed and whether it is ready.

use iced::widget::{button, column, container, row, stack, text, text_input, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};
use iced_fonts::lucide;

use crate::app::Message;
use crate::romm::normalize_code;
use crate::theme::{self, alpha, glass};
use crate::ui::tab_bar::{glyph, MARGIN, TAB_BAR_HEIGHT};

/// The pairing code is 8 alphanumerics. RomM shows it grouped `ABCD-EFGH`, and its copy button
/// includes the dash — so what is *typed* may be longer than what is sent.
pub const CODE_LEN: usize = 8;

/// The heading's width, beside the form.
const HEADING_W: f32 = 500.0;
/// The form's width, at the right; the heading takes what is left, up to its own.
const FORM_W: f32 = 560.0;
/// A row of the form, as tall as a Settings row.
const ROW_H: f32 = 60.0;
/// The column the rows' names stand in.
const LABEL_W: f32 = 150.0;
/// The QR code's side.
const QR_SIDE: f32 = 300.0;

#[derive(Debug, Clone, Default)]
pub struct LoginState {
    pub url: String,
    pub code: String,
    /// Accept a certificate that does not verify. Off by default and never set silently: a LAN RomM
    /// very often has a self-signed certificate, but that has to be the user's call, once, in view
    /// of the address they typed.
    pub insecure: bool,
    /// A pairing attempt is in flight — the button is disabled and the fields are frozen, so a
    /// second press cannot mint a second token from a code that is only valid once.
    pub connecting: bool,
    /// The last failure, or the standing hint. Shown under the fields.
    pub status: Option<String>,
    /// A request to pair by QR code, waiting on a phone. While there is one, the screen shows the
    /// code instead of the fields.
    pub qr: Option<Box<crate::qr_pairing::QrPairing>>,
    /// Bumped whenever a QR request is started or given up, so an answer to an older one is dropped.
    pub qr_generation: u64,
    /// Where the pad's cursor is.
    pub focus: Row,
}

/// The screen's rows, as the pad walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Row {
    #[default]
    Server,
    Qr,
    Code,
    Insecure,
    Connect,
}

impl Row {
    pub const ALL: [Row; 5] = [Row::Server, Row::Qr, Row::Code, Row::Insecure, Row::Connect];

    /// The row `delta` from this one, stopping at the ends.
    pub fn step(self, delta: isize) -> Row {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(0);
        Self::ALL[at.saturating_add_signed(delta).min(Self::ALL.len() - 1)]
    }
}

impl LoginState {
    pub fn new(url: String) -> Self {
        Self { url, ..Default::default() }
    }

    /// Ready to pair: a real server and a full-length code.
    pub fn complete(&self) -> bool {
        !self.url.trim().is_empty()
            && self.url.trim() != "https://"
            && normalize_code(&self.code).len() == CODE_LEN
    }
}

pub fn login_view(state: &LoginState) -> Element<'_, Message> {
    match &state.qr {
        Some(qr) => screen(qr_page(qr, state.status.as_deref()), vec![("B", "Back".to_string())]),
        None => screen(form_page(state), form_hints(state)),
    }
}

/// The library on its way in, after pairing and at every start: the same ground and wordmark, and
/// where it has got to.
pub fn loading_view(status: &str) -> Element<'_, Message> {
    let body = column![
        crate::ui::home::tag("LIBRARY".into(), true),
        text("Loading your library").size(theme::text_size::DISPLAY_MEDIUM).font(theme::DISPLAY).color(theme::INK),
        text(status).size(17.0).color(alpha(theme::INK, 0.82)),
    ]
    .spacing(12)
    .width(Length::Fixed(HEADING_W));
    screen(body.into(), Vec::new())
}

/// A sign-in screen: Home's ground with the accent wash, the wordmark where the header has it,
/// `page` in the middle of the height, and what the pad's buttons do at the foot.
fn screen<'a>(page: Element<'a, Message>, hints: Vec<(&'a str, String)>) -> Element<'a, Message> {
    let head = container(crate::ui::tab_bar::wordmark()).padding([0.0, MARGIN]).center_y(Length::Fixed(TAB_BAR_HEIGHT));
    let foot: Element<Message> = if hints.is_empty() {
        Space::new().height(Length::Fixed(44.0)).into()
    } else {
        crate::ui::tab_bar::hint_line(hints)
    };
    stack![
        crate::ui::home::plain_stage(),
        column![head, container(page).padding([0.0, MARGIN]).width(Length::Fill).center_y(Length::Fill), foot],
    ]
    .into()
}

/// The heading where Home has its spotlight: the tag, what is being asked, and why.
fn heading<'a>(eyebrow: &'a str, headline: &'a str, under: &'a str) -> iced::widget::Column<'a, Message> {
    column![
        row![
            crate::ui::home::tag("SIGN IN".into(), true),
            text(eyebrow).size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        text(headline).size(theme::text_size::DISPLAY_MEDIUM).font(theme::DISPLAY).color(theme::INK),
        text(under).size(17.0).color(alpha(theme::INK, 0.86)),
    ]
    .spacing(12)
    .width(Length::Fill)
}

/// The address and the two ways in: the heading and its steps at the left, the form at the right.
fn form_page(state: &LoginState) -> Element<'_, Message> {
    let steps = column![
        step(1, "Enter your RomM server's address."),
        step(2, "Pair with a QR code: scan it with your phone, and approve Deckhand in RomM."),
        step(3, "Or press Pair Device in RomM's web UI and type the 8-character code it shows."),
    ]
    .spacing(12);
    let left = heading(
        "PAIR THIS DEVICE",
        "Connect to RomM",
        "Your library, downloads and saves come from your RomM server. Pair once and Deckhand stays signed in.",
    )
    .push(Space::new().height(Length::Fixed(14.0)))
    .push(steps);

    let lit = |row: Row| state.focus == row;
    let frozen = state.connecting;
    let server = field_row(
        lit(Row::Server),
        "Server",
        text_input("https://romm.local", &state.url)
            .on_input_maybe((!frozen).then_some(Message::UrlChanged))
            .on_submit(Message::Connect)
            .size(theme::text_size::BODY_MEDIUM)
            .padding([8, 12])
            .style(input_style)
            .into(),
    );
    let qr = wide_button(
        row![lucide::qr_code().size(20.0), text("Pair with a QR code").size(16.0).font(theme::BOLD)].spacing(12).align_y(Alignment::Center),
        // The way on until a code is typed in full; then Connect is.
        !state.complete(),
        lit(Row::Qr),
        (!frozen).then_some(Message::StartQr),
    );
    let or = row![rule(), text("or with a pairing code").size(13.0).color(theme::TEXT_SECONDARY), rule()].spacing(14).align_y(Alignment::Center);
    let code = field_row(
        lit(Row::Code),
        "Pairing code",
        text_input("ABCD-EFGH", &state.code)
            .on_input_maybe((!frozen).then_some(Message::CodeChanged))
            .on_submit(Message::Connect)
            .size(theme::text_size::BODY_MEDIUM)
            .padding([8, 12])
            .style(input_style)
            .into(),
    );
    let insecure = field_row(
        lit(Row::Insecure),
        "Certificate",
        row![
            text("Accept a self-signed one").size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY).width(Length::Fill),
            off_on(state.insecure, frozen),
        ]
        .align_y(Alignment::Center)
        .into(),
    );
    let ready = state.complete() && !frozen;
    let connect = wide_button(
        text(if frozen { "Connecting…" } else { "Connect" }).size(16.0).font(theme::BOLD),
        ready,
        lit(Row::Connect),
        ready.then_some(Message::Connect),
    );

    let mut right = column![server, qr, or, code, insecure, connect].spacing(10).width(Length::Fixed(FORM_W));
    if let Some(message) = &state.status {
        // A failure and the standing hint share the line, so the colour is what separates them.
        right = right.push(text(message.clone()).size(theme::text_size::LABEL_LARGE).color(if frozen { theme::TEXT_SECONDARY } else { theme::ERROR }));
    }

    row![container(left.max_width(HEADING_W)).width(Length::Fill), right].spacing(48).align_y(Alignment::Center).into()
}

/// What the pad's buttons do where the cursor is.
fn form_hints(state: &LoginState) -> Vec<(&'static str, String)> {
    let press = match state.focus {
        Row::Server | Row::Code => "Type",
        Row::Qr => "Pair with a QR code",
        Row::Insecure => "Change",
        Row::Connect => "Connect",
    };
    vec![("A", press.to_string())]
}

/// One of the steps under the heading, its number in a glass ring.
fn step<'a>(n: u8, body: &'a str) -> Element<'a, Message> {
    let number = container(text(n.to_string()).size(13.0).font(theme::BOLD).color(theme::ACCENT_400)).center(Length::Fixed(28.0)).style(|_| container::Style {
        background: Some(Background::Color(glass(0.08))),
        border: Border { color: glass(0.14), width: 1.0, radius: 14.0.into() },
        ..Default::default()
    });
    row![number, text(body).size(15.0).color(alpha(theme::INK, 0.8)).width(Length::Fill)].spacing(14).align_y(Alignment::Center).into()
}

/// A row of the form: its name, and what it holds. A glass row, lifted on the ink ring under the
/// pad's cursor — the input inside answers the pointer and a keyboard itself.
fn field_row<'a>(lit: bool, label: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    button(
        row![text(label).size(16.0).font(theme::SEMIBOLD).color(theme::INK).width(Length::Fixed(LABEL_W)), content]
            .spacing(12)
            .align_y(Alignment::Center)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ROW_H))
    .padding([0, 20])
    .style(crate::ui::style::glass_row(lit, 16.0))
    .into()
}

/// One of the form's two buttons, as wide as its rows: the accent when it is the way on, glass when
/// it is not ready yet, ringed in the ink under the pad's cursor.
fn wide_button<'a>(label: impl Into<Element<'a, Message>>, primary: bool, lit: bool, press: Option<Message>) -> Element<'a, Message> {
    let spotlight = crate::ui::style::spotlight(primary, lit);
    button(container(label).center(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(56.0))
        .padding([0, 20])
        .on_press_maybe(press)
        .style(move |theme, status| {
            let mut style = spotlight(theme, status);
            if matches!(status, button::Status::Disabled) && !primary {
                style.text_color = alpha(theme::INK, 0.45);
            }
            style
        })
        .into()
}

/// The certificate row's two settings, as a Settings row steps between them.
fn off_on<'a>(on: bool, frozen: bool) -> Element<'a, Message> {
    let mut pills = row![].spacing(3).align_y(Alignment::Center);
    for (words, value) in [("Off", false), ("On", true)] {
        let set = on == value;
        pills = pills.push(
            button(container(text(words).size(13.0).font(if set { theme::SEMIBOLD } else { theme::MEDIUM })).center_y(Length::Fill))
                .height(Length::Fixed(28.0))
                .padding([0, 16])
                .on_press_maybe((!frozen).then_some(Message::InsecureToggled(value)))
                .style(move |_, status| button::Style {
                    background: match (set, matches!(status, button::Status::Hovered)) {
                        (true, _) => Some(Background::Color(theme::INK)),
                        (false, true) => Some(Background::Color(glass(0.12))),
                        (false, false) => None,
                    },
                    text_color: if set { theme::GROUND } else { alpha(theme::INK, 0.82) },
                    border: Border { radius: 999.0.into(), ..Default::default() },
                    shadow: Shadow::default(),
                    snap: true,
                }),
        );
    }
    container(pills)
        .padding(3)
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { color: glass(0.12), width: 1.0, radius: 999.0.into() },
            ..Default::default()
        })
        .into()
}

/// A faint line either side of the "or".
fn rule<'a>() -> Element<'a, Message> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(glass(0.12))), ..Default::default() })
        .into()
}

/// The QR code, waiting on a phone: what to do with it, the short code to check it against and the
/// address it opens at the left, the code itself on a white card at the right, and the way back.
fn qr_page<'a>(qr: &crate::qr_pairing::QrPairing, status: Option<&str>) -> Element<'a, Message> {
    let mut left = heading("WAITING FOR YOUR PHONE", "Scan to pair", "Scan this with your phone, sign in to RomM there, and approve Deckhand.")
        .push(Space::new().height(Length::Fixed(10.0)))
        .push(text("THE CODE ROMM WILL SHOW").size(12.0).font(theme::BOLD).color(theme::TEXT_SECONDARY))
        .push(text(qr.user_code.clone()).size(40.0).font(theme::DISPLAY).color(theme::ACCENT_400))
        .push(text(qr.url.clone()).size(13.0).color(theme::TEXT_SECONDARY))
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(
            row![lucide::clock_four().size(16.0).color(alpha(theme::INK, 0.7)), text("Waiting for approval…").size(15.0).color(alpha(theme::INK, 0.82))]
                .spacing(10)
                .align_y(Alignment::Center),
        );
    if let Some(status) = status {
        left = left.push(text(status.to_string()).size(theme::text_size::LABEL_LARGE).color(theme::ERROR));
    }
    let back = button(row![glyph("B", false), text("Back").size(15.0).font(theme::SEMIBOLD)].spacing(9).align_y(Alignment::Center))
        .padding(Padding { top: 9.0, right: 18.0, bottom: 9.0, left: 10.0 })
        .on_press(Message::CancelQr)
        .style(crate::ui::style::spotlight(false, false));
    left = left.push(Space::new().height(Length::Fixed(8.0))).push(back);

    let code: Element<Message> = match &qr.qr {
        Some(handle) => iced::widget::image(handle.clone()).width(Length::Fixed(QR_SIDE)).height(Length::Fixed(QR_SIDE)).into(),
        None => Space::new().width(Length::Fixed(QR_SIDE)).height(Length::Fixed(QR_SIDE)).into(),
    };
    // White to the edge of a quiet zone, as a phone's camera wants it, and stood off the ground.
    let card = container(code).padding(18).style(|_| container::Style {
        background: Some(Background::Color(Color::WHITE)),
        border: Border { radius: 20.0.into(), ..Default::default() },
        shadow: Shadow { color: alpha(Color::BLACK, 0.45), offset: iced::Vector::new(0.0, 14.0), blur_radius: 32.0 },
        ..Default::default()
    });

    row![container(left.max_width(HEADING_W)).width(Length::Fill), card, Space::new().width(Length::Fixed(80.0))].spacing(48).align_y(Alignment::Center).into()
}

/// A field inside a glass row: a shade darker than the row, with the accent edge while it has the
/// keyboard's focus.
fn input_style(_theme: &iced::Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(alpha(theme::GROUND, 0.55)),
        border: Border { color: if focused { theme::ACCENT_400 } else { glass(0.10) }, width: 1.0, radius: 10.0.into() },
        icon: theme::TEXT_MUTED,
        placeholder: alpha(theme::INK, 0.38),
        value: theme::INK,
        selection: theme::ACCENT_800,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_stays_disabled_until_both_fields_are_real() {
        let mut state = LoginState::new("https://".into());
        assert!(!state.complete(), "the scheme alone is not a server");

        state.url = "https://romm.local".into();
        assert!(!state.complete(), "no code yet");

        state.code = "ABCD".into();
        assert!(!state.complete(), "half a code is not a code");

        state.code = "ABCD-EFGH".into();
        assert!(state.complete(), "the grouping dash does not count against the length");
    }
}
