//! Browse-UI theme: colour from the **Nocturne** design system, type from Deckhand's spec §11.6.
//!
//! A direct port of `browse/ui/BrowseTheme.kt`. The split it documents is preserved: Nocturne
//! supplies the palette, but its type scale (a compact desktop system, 15 px body) is deliberately
//! *not* imported — this app is read from a couch on a 7" 1280x800 panel, which is what the 16 sp
//! floor / 18 sp body ladder below exists for.

use iced::{Color, Theme};

/// `0xRRGGBB` → [`Color`]. `const` so the ramps below are compile-time constants like the Kotlin
/// `val`s they port, rather than lazily-built statics.
const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

/*
 * Tonal ramps, from the Nocturne design system. Generated in OKLCH on one shared perceptual
 * lightness scale, so the same step of either ramp carries the same visual weight — which is the
 * point of having them: a tint, a border and a pressed state picked from matching steps relate to
 * each other by construction, where hand-mixed one-off alphas only relate by luck.
 *
 * On this dark ground: the deep steps (700-900) are tinted fills, hovers and subtle borders, 500 is
 * the role's base, and the light steps (100-300) are text *on* those tints. Prefer a ramp step over
 * an ad-hoc alpha.
 */
pub const NEUTRAL_100: Color = rgb(0xF3F5FE);
pub const NEUTRAL_400: Color = rgb(0xB2B6CA);
pub const NEUTRAL_500: Color = rgb(0x9397AB);
pub const NEUTRAL_600: Color = rgb(0x75798C);
pub const NEUTRAL_700: Color = rgb(0x595D6C);
pub const NEUTRAL_800: Color = rgb(0x3F424D);
pub const NEUTRAL_900: Color = rgb(0x292B31);

pub const ACCENT_400: Color = rgb(0xB5ABFC);
pub const ACCENT_700: Color = rgb(0x5D5294);
pub const ACCENT_800: Color = rgb(0x423A6A);
pub const ACCENT_900: Color = rgb(0x2B2741);

// Palette. Roles keep the names every view already reads.
pub const BACKGROUND: Color = rgb(0x161826);
pub const SURFACE: Color = rgb(0x232532);
pub const SURFACE_VARIANT: Color = NEUTRAL_900;
pub const ACCENT: Color = rgb(0x9184D9);
pub const ACCENT_SOFT: Color = ACCENT_400;
pub const TEXT_PRIMARY: Color = rgb(0xE9E9ED);
pub const TEXT_SECONDARY: Color = NEUTRAL_400;

/// The third text tier, below [`TEXT_SECONDARY`]: uppercase tracked meta labels ("PLAYERS",
/// "SCREENSHOTS") that name the value beside them and are never read on their own. Deliberately
/// below the §11.6 contrast floor for content text — only ever used where the value it labels
/// carries the meaning.
pub const TEXT_MUTED: Color = NEUTRAL_600;

/// Long-form body copy — one step down from [`TEXT_SECONDARY`]. A paragraph set at full secondary
/// weight competes with the facts above it.
pub const TEXT_BODY: Color = NEUTRAL_500;

pub const BORDER: Color = NEUTRAL_800;
pub const ERROR: Color = rgb(0xFF6B6B);
/// Something here wants attention without being wrong — an update waiting on RomM.
pub const WARNING: Color = rgb(0xE5B567);

/// The focus idiom's two surfaces. [`FOCUS_FILL`] is "this is the current row", [`FOCUS_RING`] is
/// "the cursor is on it *right now*"; the accent itself stays for the one mark that should read as
/// the accent.
pub const FOCUS_FILL: Color = ACCENT_900;
pub const FOCUS_RING: Color = ACCENT_700;

/// Home's ground, a step below [`BACKGROUND`]: the spotlight's picture and its light fade out into
/// it, and a darker ground is what lets the screenshot carry the page's colour instead of the page.
pub const GROUND: Color = rgb(0x0D0E16);

/// The brightest ink: the selected destination's pill, a ring round the card under the cursor. A
/// step above [`TEXT_PRIMARY`], so the one thing marked with it outshines the text around it.
pub const INK: Color = NEUTRAL_100;

/// The accent as a fill under white text. [`ACCENT`] itself is too light to carry white at body
/// size (3.3:1); this is the same hue a step down, at 5:1.
pub const ACCENT_600: Color = rgb(0x6E5FC8);

/// Saves that are where they should be — the header's sync dot. Lighter than any other mark on the
/// bar, so it reads as "fine" without a word, and it is never the only way that is said.
pub const IN_SYNC: Color = rgb(0x8FD5A6);

/// A rating's star.
pub const STAR: Color = rgb(0xF4C76B);

/// White at `a` — the glass the header's pills and the spotlight's quieter buttons are cut from. Over
/// a picture, a translucent white lifts whatever colour is behind it rather than covering it.
pub fn glass(a: f32) -> Color {
    alpha(Color::WHITE, a)
}

/// The same colour at a different opacity. Views reach for this constantly — a scrim, a hairline
/// border, a dimmed tint — and every one of them was spelling it out inline.
pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The Deckhand palette as an iced [`Theme`]. Built once at startup and cloned into every view.
pub fn deckhand() -> Theme {
    Theme::custom(
        "Deckhand".to_string(),
        iced::theme::Palette {
            background: BACKGROUND,
            text: TEXT_PRIMARY,
            primary: ACCENT,
            success: ACCENT_SOFT,
            danger: ERROR,
            // 0.14 added a warning role; the palette has no amber, so the accent stands in.
            warning: ACCENT_SOFT,
        },
    )
}

/// The §11.6 UI-scale ladder. "Large" (1.15) is the default — at Deck/TV distance the smaller sizes
/// read cramped, which is why only Large ever felt useful.
pub mod ui_scale {
    /// The ladder, as the Settings page offers it.
    pub const STEPS: [f32; 4] = [1.0, 1.15, 1.3, 1.5];
    pub const LABELS: [&str; 4] = ["Normal", "Large", "Larger", "Huge"];
    pub const DEFAULT: f32 = 1.15;
}

/// Spec §11.6 type roles. Sizes are in points *before* the UI scale, which the app applies globally
/// through iced's `scale_factor` rather than per-widget — so these stay the literal spec numbers.
///
/// The full ladder runs displayMedium 46 · displaySmall 34 · headlineSmall 24 · titleMedium 18 ·
/// bodyLarge 18 · bodyMedium 16 · labelLarge 14 · labelMedium 12.
pub mod text_size {
    /// 46 — the Detail hero title, the one place in the app that shouts.
    pub const DISPLAY_MEDIUM: f32 = 46.0;
    /// 24 — page titles (the Systems heading, a system's name).
    pub const HEADLINE_SMALL: f32 = 24.0;
    /// 18 — body.
    pub const BODY_LARGE: f32 = 18.0;
    /// 16 — tile titles, section labels. The floor for must-read text.
    pub const BODY_MEDIUM: f32 = 16.0;
    /// 14 — meta, counts, chips, prompt bar.
    pub const LABEL_LARGE: f32 = 14.0;
    /// 12 — muted micro-labels, never for must-read content.
    pub const LABEL_MEDIUM: f32 = 12.0;
}

use iced::font::{Family, Weight};
use iced::Font;

/// The family every weight below resolves against.
///
/// DM Sans, bundled under `fonts/` and registered in `main`. Named rather than left as
/// `Family::SansSerif` because the generic maps to whatever the host calls sans — Segoe UI on
/// Windows, DejaVu on a Deck — and the type scale in this file is set against one face.
///
/// All five weights report this same family to `fontdb`, which is what makes asking for the family
/// plus a weight enough. Two of the five say so the long way round: DM Sans ships Medium, SemiBold
/// and ExtraBold with a *legacy* family name of their own ("DM Sans SemiBold"), and only the
/// typographic family (name ID 16) says "DM Sans". `fontdb` reads ID 16 first and falls back to
/// ID 1, so all five land in one family — but a font stack that read ID 1 would see four families
/// and quietly fall back to Regular for three of the weights.
pub const FAMILY: &str = "DM Sans";

pub const fn weighted(weight: Weight) -> Font {
    Font {
        family: Family::Name(FAMILY),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// The body face, and the app's default — anything that does not name a font gets this.
pub const REGULAR: Font = weighted(Weight::Normal);
pub const MEDIUM: Font = weighted(Weight::Medium);
pub const SEMIBOLD: Font = weighted(Weight::Semibold);
pub const BOLD: Font = weighted(Weight::Bold);
pub const EXTRABOLD: Font = weighted(Weight::ExtraBold);

/// The display face: Space Grotesk, for the few words set as headings — the wordmark, a shelf's
/// name, a game's name where it has no logo. A grotesk with squared-off curves against DM Sans's
/// round ones, so a heading reads as a heading by its shape and not only by its size.
///
/// Bold only. Upstream ships no static SemiBold, and the headings are few and large enough that
/// one weight covers them.
pub const DISPLAY: Font = Font {
    family: Family::Name("Space Grotesk"),
    weight: Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

/// Deckhand's logo — the same 512 px square the AppImage and the README carry.
pub const LOGO: &[u8] = include_bytes!("../packaging/deckhand.png");

/// The logo at `px` pixels square, decoded and resampled once per size: a widget handed the 512 px
/// original would be shrunk by the GPU with no mipmaps, which shimmers on pixel art like this.
fn logo_at(px: u32) -> image::RgbaImage {
    image::load_from_memory(LOGO)
        .map(|logo| logo.resize_exact(px, px, image::imageops::FilterType::CatmullRom).into_rgba8())
        .unwrap_or_else(|_| image::RgbaImage::new(1, 1))
}

/// The logo as the views draw it, made once: an image handle made each frame would be a new image to
/// the renderer every frame.
pub fn logo() -> iced::widget::image::Handle {
    static LOGO_HANDLE: std::sync::OnceLock<iced::widget::image::Handle> = std::sync::OnceLock::new();
    LOGO_HANDLE
        .get_or_init(|| {
            // Enough rows for the largest place it is drawn, 96 units, on a 2x panel at the Huge step.
            let logo = logo_at(288);
            iced::widget::image::Handle::from_rgba(logo.width(), logo.height(), logo.into_raw())
        })
        .clone()
}

/// Deckhand's wordmark: the pixel letters `packaging/art/render.py` draws, as the transparent PNG
/// Steam takes for its logo — each cell of the letters' grid drawn [`WORDMARK_CELL`] pixels square.
const WORDMARK: &[u8] = include_bytes!("../packaging/art/steam/logo.png");

/// How many pixels of [`WORDMARK`] one cell of the letters' grid takes: `render.py`'s unit.
const WORDMARK_CELL: u32 = 20;

/// The wordmark at a pixel a cell, and how many cells across and down that is — made once.
///
/// The header scales it up by whole pixels with nearest-neighbour filtering, so every cell stays a
/// hard-edged square, as the ship's do. The 1100 px drawing shrunk by the GPU would blur instead.
pub fn wordmark() -> (iced::widget::image::Handle, u32, u32) {
    static WORDMARK_HANDLE: std::sync::OnceLock<(iced::widget::image::Handle, u32, u32)> = std::sync::OnceLock::new();
    WORDMARK_HANDLE
        .get_or_init(|| {
            let cells = wordmark_cells();
            let (w, h) = cells.dimensions();
            (iced::widget::image::Handle::from_rgba(w, h, cells.into_raw()), w, h)
        })
        .clone()
}

/// [`WORDMARK`] cut down to one pixel a cell, each the colour at its cell's middle.
fn wordmark_cells() -> image::RgbaImage {
    let Ok(drawn) = image::load_from_memory(WORDMARK).map(|w| w.into_rgba8()) else { return image::RgbaImage::new(1, 1) };
    let (w, h) = ((drawn.width() / WORDMARK_CELL).max(1), (drawn.height() / WORDMARK_CELL).max(1));
    image::RgbaImage::from_fn(w, h, |x, y| *drawn.get_pixel(x * WORDMARK_CELL + WORDMARK_CELL / 2, y * WORDMARK_CELL + WORDMARK_CELL / 2))
}

/// The logo as the window's icon — what the taskbar and Alt-Tab show, where the system takes it from
/// the window rather than from a desktop entry.
pub fn window_icon() -> Option<iced::window::Icon> {
    let logo = logo_at(256);
    let (w, h) = logo.dimensions();
    iced::window::icon::from_rgba(logo.into_raw(), w, h).ok()
}

/// The bundled faces, in the order `main` registers them.
///
/// Embedded rather than loaded from disk: the app has to render its first frame with no filesystem
/// lookup and no assumption that the font is installed on the host.
pub const FONT_FACES: [&[u8]; 6] = [
    include_bytes!("../fonts/DMSans-Regular.ttf"),
    include_bytes!("../fonts/DMSans-Medium.ttf"),
    include_bytes!("../fonts/DMSans-SemiBold.ttf"),
    include_bytes!("../fonts/DMSans-Bold.ttf"),
    include_bytes!("../fonts/DMSans-ExtraBold.ttf"),
    include_bytes!("../fonts/SpaceGrotesk-Bold.ttf"),
];

/// A stable per-system hue, ported from `SystemPills.kt` — the dot beside the system name on the
/// hero, and the colour wash behind a game with no art.
///
/// Kotlin's `String.hashCode` is reimplemented rather than using Rust's `DefaultHasher`, so a
/// system keeps the exact colour it has in the Compose app: the hue is `hashCode % 360`, which is a
/// different number under any other hash.
pub fn system_hue(system_id: &str) -> Color {
    let hash = system_id
        .encode_utf16()
        .fold(0i32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as i32));
    let hue = ((hash % 360) + 360) % 360;
    hsv(hue as f32, 0.55, 0.88)
}

/// HSV → RGB, for [`system_hue`] alone (iced's palette is RGB-only).
fn hsv(h: f32, s: f32, v: f32) -> Color {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wordmark_is_its_letters_grid_a_pixel_a_cell() {
        let cells = wordmark_cells();
        assert_eq!(cells.dimensions(), (55, 7), "DECKHAND, seven cells high");
        // The D's top-left corner is the top band's pink; the gap after the letter is clear.
        assert_eq!(cells.get_pixel(0, 0).0, [0xFF, 0x4F, 0xD8, 0xFF]);
        assert_eq!(cells.get_pixel(5, 0).0[3], 0, "nothing between the letters");
        assert!(cells.pixels().all(|p| p.0[3] == 0 || p.0[3] == 0xFF), "every cell hard-edged");
    }
}
