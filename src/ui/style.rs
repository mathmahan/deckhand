//! The app's button looks, shared: a list row, a chip, a key or an action looks and lights the same on
//! every page. They were written page by page, and the focus looks drifted apart — a primary button
//! that out-shone the one the cursor was on, a chip whose ring vanished on its own fill.
//!
//! Each is a style function for `button(..).style(..)`. `lit` is the pad's cursor on it; the mouse
//! pointer's hover lights the fill only, so the two can never be mistaken for each other.

use iced::widget::button::{Status, Style};
use iced::{Background, Border, Color, Shadow, Theme};

use crate::theme;

fn hovered(status: Status) -> bool {
    matches!(status, Status::Hovered)
}

fn style(background: Option<Color>, text: Color, border: Border) -> Style {
    Style { background: background.map(Background::Color), text_color: text, border, shadow: Shadow::default(), snap: true }
}

/// A row of a list, or a card: the surface, lit with the focus fill and a 2 px ring.
pub fn row(lit: bool, radius: f32) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        style(
            Some(if lit || hovered(status) { theme::FOCUS_FILL } else { theme::SURFACE }),
            theme::TEXT_PRIMARY,
            Border { color: if lit { theme::FOCUS_RING } else { theme::BORDER }, width: if lit { 2.0 } else { 1.0 }, radius: radius.into() },
        )
    }
}

/// A filter, sort or kind chip: filled with the accent when chosen, and ringed in white under the
/// cursor — a ring in the accent disappeared into a chosen chip's accent fill.
pub fn chip(selected: bool, lit: bool) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        let fill = if selected {
            Some(theme::ACCENT)
        } else if lit || hovered(status) {
            Some(theme::FOCUS_FILL)
        } else {
            None
        };
        let ring = if lit {
            theme::TEXT_PRIMARY
        } else if selected {
            theme::ACCENT
        } else {
            theme::BORDER
        };
        style(
            fill,
            if selected { Color::WHITE } else { theme::TEXT_SECONDARY },
            Border { color: ring, width: if lit { 2.0 } else { 1.0 }, radius: 16.0.into() },
        )
    }
}

/// A key of an on-screen keyboard: filled with the accent under the cursor. `on` is a key held
/// down — the shift, say — which keeps a fill of its own.
pub fn key(lit: bool, on: bool) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        let fill = if lit {
            theme::ACCENT
        } else if on || hovered(status) {
            theme::FOCUS_FILL
        } else {
            theme::SURFACE
        };
        style(
            Some(fill),
            if lit { theme::BACKGROUND } else { theme::TEXT_PRIMARY },
            Border { color: if on { theme::FOCUS_RING } else { theme::BORDER }, width: 1.0, radius: 6.0.into() },
        )
    }
}

/// One of a page's actions. Only the one under the cursor is filled bright; the main action keeps
/// a muted purple of its own so it still reads as the main one — at full strength it out-shone a
/// focused neighbour, and from a couch the brightest button is the one you're on.
pub fn action(primary: bool, lit: bool) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        let on = lit || hovered(status);
        let fill = match (primary, on) {
            (_, true) => theme::ACCENT,
            (true, false) => theme::ACCENT_800,
            (false, false) => theme::alpha(theme::SURFACE, 0.8),
        };
        let ring = match (lit, primary) {
            (true, _) => theme::ACCENT_400,
            (false, true) => theme::ACCENT_700,
            (false, false) => theme::BORDER,
        };
        style(
            Some(fill),
            if on || primary { Color::WHITE } else { theme::TEXT_PRIMARY },
            Border { color: ring, width: if lit { 2.0 } else { 1.0 }, radius: 10.0.into() },
        )
    }
}

/// One of the buttons in Home's spotlight, which sit on a picture rather than on the page.
///
/// The main one is filled with the accent a step down, dark enough to carry white words; the rest
/// are glass, so the picture shows through them and they recede behind it. Under the pad's cursor a
/// button takes a ring of the brightest ink — the mark the card under the cursor wears, so the eye
/// follows the ring up from the shelf — and the pointer only brightens the fill.
pub fn spotlight(primary: bool, lit: bool) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        let on = lit || hovered(status);
        let fill = match (primary, on) {
            (true, false) => theme::ACCENT_600,
            (true, true) => crate::animation::mix(theme::ACCENT_600, theme::ACCENT, 0.3),
            (false, false) => theme::glass(0.08),
            (false, true) => theme::glass(0.16),
        };
        let border = match (lit, primary) {
            (true, _) => Border { color: theme::INK, width: 2.0, radius: 14.0.into() },
            (false, true) => Border { radius: 14.0.into(), ..Border::default() },
            (false, false) => Border { color: theme::glass(0.16), width: 1.0, radius: 14.0.into() },
        };
        Style {
            background: Some(Background::Color(fill)),
            text_color: if primary { Color::WHITE } else { theme::INK },
            border,
            // The main action stands off the picture; glass lies on it.
            shadow: if primary {
                Shadow { color: theme::alpha(Color::BLACK, 0.35), offset: iced::Vector::new(0.0, 10.0), blur_radius: 28.0 }
            } else {
                Shadow::default()
            },
            snap: true,
        }
    }
}

/// A quiet outlined button — the banner's Dismiss, away from the cursor.
pub fn quiet(_: &Theme, status: Status) -> Style {
    let on = hovered(status);
    style(
        on.then_some(theme::FOCUS_FILL),
        if on { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY },
        Border { color: if on { theme::FOCUS_RING } else { theme::BORDER }, width: 1.0, radius: 8.0.into() },
    )
}

/// A button with no look of its own: a card, a tile or a tab whose content draws itself.
pub fn bare(_: &Theme, _: Status) -> Style {
    style(None, theme::TEXT_PRIMARY, Border::default())
}

/// A text link — a page's way back: quiet, brighter under the pointer.
pub fn link(_: &Theme, status: Status) -> Style {
    style(None, if hovered(status) { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY }, Border::default())
}
