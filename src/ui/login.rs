//! First-launch pairing — a port of `browse/ui/LoginView.kt`.
//!
//! The server's address, then either of RomM's two ways to let a device in: a QR code a phone
//! scans to approve it ([`crate::qr_pairing`]), or the 8-character code RomM shows when linking one.
//! The fields are real text inputs for a keyboard and a pointer, and the whole screen also answers
//! the pad: Up and Down walk it, and A on a field opens the on-screen keyboard — so a Deck pairs
//! without a keyboard at all, by QR code, typing only the address.
//!
//! Kept free of network code so the flow stays testable: [`LoginState`] only decides what is
//! typed and whether it is ready.

use iced::widget::{button, checkbox, column, container, row, text, text_input, Space};
use iced::{Alignment, Background, Border, Element, Length, Shadow};

use crate::app::Message;
use crate::romm::normalize_code;
use crate::theme;

/// The pairing code is 8 alphanumerics. RomM shows it grouped `ABCD-EFGH`, and its copy button
/// includes the dash — so what is *typed* may be longer than what is sent.
pub const CODE_LEN: usize = 8;

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
    let heading = column![
        iced::widget::image(theme::logo()).width(Length::Fixed(96.0)).height(Length::Fixed(96.0)),
        Space::new().height(Length::Fixed(14.0)),
        text("Deckhand").size(theme::text_size::DISPLAY_SMALL).font(theme::EXTRABOLD),
        Space::new().height(Length::Fixed(6.0)),
        text("Connect to your RomM server")
            .size(theme::text_size::BODY_LARGE)
            .color(theme::TEXT_SECONDARY),
    ]
    // Fill, not shrink: an alignment on a shrink-wrapped column only centres the lines against
    // each other, which left the heading sitting wherever its longest line happened to start.
    .width(Length::Fill)
    .align_x(Alignment::Center);

    if let Some(qr) = &state.qr {
        return qr_view(heading, qr, state.status.as_deref());
    }

    let steps = column![
        step(1, "Enter your RomM server address."),
        step(2, "Pair with a QR code: scan it with your phone, and approve Deckhand in RomM."),
        step(3, "Or, in RomM's web UI, press Pair Device and type the 8-character code it shows."),
    ]
    .spacing(6);

    let lit = |row: Row| state.focus == row;
    let url_field = field(
        lit(Row::Server),
        "Server",
        text_input("https://romm.local", &state.url)
            .on_input_maybe((!state.connecting).then_some(Message::UrlChanged))
            .on_submit(Message::Connect)
            .size(theme::text_size::BODY_LARGE)
            .padding(12)
            .style(input_style)
            .into(),
    );

    let mut qr_button = button(
        container(text("Pair with a QR code").size(theme::text_size::TITLE_MEDIUM).font(theme::MEDIUM)).center_x(Length::Fill),
    )
    .width(Length::Fill)
    .padding([12, 26])
    .style(move |_, status| {
        let on = lit(Row::Qr) || matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if on { theme::FOCUS_FILL } else { theme::BACKGROUND })),
            text_color: match status {
                button::Status::Disabled => theme::TEXT_MUTED,
                _ => theme::TEXT_PRIMARY,
            },
            border: Border { color: if on { theme::ACCENT } else { theme::BORDER }, width: if lit(Row::Qr) { 2.0 } else { 1.0 }, radius: 12.0.into() },
            shadow: Shadow::default(),
            snap: true,
        }
    });
    if !state.connecting {
        qr_button = qr_button.on_press(Message::StartQr);
    }

    let code_field = field(
        lit(Row::Code),
        "Pairing code",
        text_input("ABCD-EFGH", &state.code)
            .on_input_maybe((!state.connecting).then_some(Message::CodeChanged))
            .on_submit(Message::Connect)
            .size(theme::text_size::BODY_LARGE)
            .padding(12)
            .style(input_style)
            .into(),
    );

    let connect_lit = lit(Row::Connect);
    let mut connect = button(
        container(
            text(if state.connecting { "Connecting…" } else { "Connect" })
                .size(theme::text_size::TITLE_MEDIUM)
                .font(theme::BOLD),
        )
        .center_x(Length::Fill),
    )
    .width(Length::Fill)
    .padding([13, 26])
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered) || connect_lit;
        button::Style {
            background: Some(Background::Color(match status {
                // The dark tinted step, not a translucent accent: at 30% the accent still
                // composites to a confidently purple button, so the only thing saying "not yet"
                // was the label colour.
                button::Status::Disabled => theme::FOCUS_FILL,
                _ if hovered => theme::ACCENT_SOFT,
                _ => theme::ACCENT,
            })),
            text_color: match status {
                button::Status::Disabled => theme::TEXT_MUTED,
                _ => iced::Color::WHITE,
            },
            border: Border {
                color: if connect_lit { theme::TEXT_PRIMARY } else { iced::Color::TRANSPARENT },
                width: if connect_lit { 2.0 } else { 0.0 },
                radius: 12.0.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    });
    if state.complete() && !state.connecting {
        connect = connect.on_press(Message::Connect);
    }

    let insecure = container(
        checkbox(state.insecure)
            .label("Accept a self-signed certificate")
            .on_toggle_maybe((!state.connecting).then_some(Message::InsecureToggled))
            .size(16.0)
            .text_size(theme::text_size::LABEL_LARGE)
            .spacing(10),
    )
    .padding([4, 6])
    .style(move |_| container::Style {
        border: Border { color: if lit(Row::Insecure) { theme::ACCENT } else { iced::Color::TRANSPARENT }, width: 2.0, radius: 8.0.into() },
        ..Default::default()
    });

    let status: Element<Message> = match &state.status {
        Some(message) => text(message.clone())
            .size(theme::text_size::LABEL_LARGE)
            // A failure and the standing hint share the line, so the colour is what separates them.
            .color(if state.connecting { theme::TEXT_SECONDARY } else { theme::ERROR })
            .into(),
        None => Space::new().height(Length::Fixed(0.0)).into(),
    };

    let card = container(
        column![
            heading,
            Space::new().height(Length::Fixed(24.0)),
            steps,
            Space::new().height(Length::Fixed(26.0)),
            url_field,
            Space::new().height(Length::Fixed(14.0)),
            qr_button,
            Space::new().height(Length::Fixed(14.0)),
            text("or with a pairing code").size(theme::text_size::LABEL_MEDIUM).color(theme::TEXT_MUTED).width(Length::Fill).align_x(Alignment::Center),
            Space::new().height(Length::Fixed(8.0)),
            code_field,
            Space::new().height(Length::Fixed(14.0)),
            insecure,
            Space::new().height(Length::Fixed(22.0)),
            connect,
            Space::new().height(Length::Fixed(14.0)),
            status,
        ]
        .width(Length::Fixed(560.0)),
    )
    .padding(36)
    .style(|_| container::Style {
        background: Some(Background::Color(theme::SURFACE)),
        border: Border { color: theme::BORDER, width: 1.0, radius: 16.0.into() },
        ..Default::default()
    });

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::BACKGROUND)),
            ..Default::default()
        })
        .into()
}

fn step<'a>(n: u8, body: &'a str) -> Element<'a, Message> {
    row![
        text(format!("{n}."))
            .size(theme::text_size::LABEL_LARGE)
            .font(theme::BOLD)
            .color(theme::ACCENT)
            .width(Length::Fixed(20.0)),
        text(body).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_BODY),
    ]
    .spacing(4)
    .into()
}

/// A labelled input. `lit` is the pad's cursor on it, ringed outside the input's own focus ring —
/// the two are different cursors, a pointer's and a pad's.
fn field<'a>(lit: bool, label: &'a str, input: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(if lit { format!("{}  ·  A to type", label.to_uppercase()) } else { label.to_uppercase() })
            .size(11.0)
            .font(theme::BOLD)
            .color(if lit { theme::ACCENT } else { theme::TEXT_MUTED }),
        Space::new().height(Length::Fixed(6.0)),
        container(input).padding(2).style(move |_| container::Style {
            border: Border { color: if lit { theme::ACCENT } else { iced::Color::TRANSPARENT }, width: 2.0, radius: 12.0.into() },
            ..Default::default()
        }),
    ]
    .into()
}

/// The QR code, waiting on a phone: what to do with it, the short code to check it against, and
/// the way back.
fn qr_view<'a>(heading: iced::widget::Column<'a, Message>, qr: &crate::qr_pairing::QrPairing, status: Option<&str>) -> Element<'a, Message> {
    let code: Element<Message> = match &qr.qr {
        Some(handle) => iced::widget::image(handle.clone()).width(Length::Fixed(280.0)).height(Length::Fixed(280.0)).into(),
        None => Space::new().height(Length::Fixed(0.0)).into(),
    };
    let mut body = column![
        heading,
        Space::new().height(Length::Fixed(20.0)),
        text("Scan this with your phone, sign in to RomM there, and approve Deckhand.")
            .size(theme::text_size::BODY_LARGE)
            .color(theme::TEXT_BODY)
            .width(Length::Fill)
            .align_x(Alignment::Center),
        Space::new().height(Length::Fixed(18.0)),
        container(code).center_x(Length::Fill),
        Space::new().height(Length::Fixed(16.0)),
        text(qr.user_code.clone()).size(theme::text_size::HEADLINE_SMALL).font(theme::BOLD).color(theme::ACCENT).width(Length::Fill).align_x(Alignment::Center),
        text(qr.url.clone()).size(theme::text_size::LABEL_MEDIUM).color(theme::TEXT_MUTED).width(Length::Fill).align_x(Alignment::Center),
        Space::new().height(Length::Fixed(16.0)),
        text("Waiting for approval…").size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY).width(Length::Fill).align_x(Alignment::Center),
    ]
    .width(Length::Fixed(560.0));
    if let Some(status) = status {
        body = body.push(text(status.to_string()).size(theme::text_size::LABEL_LARGE).color(theme::ERROR));
    }
    body = body.push(Space::new().height(Length::Fixed(18.0))).push(
        button(container(text("Back").size(theme::text_size::LABEL_LARGE)).center_x(Length::Fill))
            .width(Length::Fill)
            .padding([10, 20])
            .on_press(Message::CancelQr)
            .style(|_, status| button::Style {
                background: Some(Background::Color(if matches!(status, button::Status::Hovered) { theme::FOCUS_FILL } else { theme::BACKGROUND })),
                text_color: theme::TEXT_PRIMARY,
                border: Border { color: theme::BORDER, width: 1.0, radius: 10.0.into() },
                shadow: Shadow::default(),
                snap: true,
            }),
    );
    body = body.push(
        text("B or Esc goes back").size(theme::text_size::LABEL_MEDIUM).color(theme::TEXT_MUTED).width(Length::Fill).align_x(Alignment::Center),
    );
    container(
        container(body).padding(36).style(|_| container::Style {
            background: Some(Background::Color(theme::SURFACE)),
            border: Border { color: theme::BORDER, width: 1.0, radius: 16.0.into() },
            ..Default::default()
        }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(|_| container::Style { background: Some(Background::Color(theme::BACKGROUND)), ..Default::default() })
    .into()
}

fn input_style(_theme: &iced::Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(theme::BACKGROUND),
        border: Border {
            color: if focused { theme::ACCENT } else { theme::BORDER },
            width: if focused { 2.0 } else { 1.0 },
            radius: 10.0.into(),
        },
        icon: theme::TEXT_MUTED,
        placeholder: theme::TEXT_MUTED,
        value: theme::TEXT_PRIMARY,
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
