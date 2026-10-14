//! The focus cross-fade — the hand-off between the card the cursor left and the one it landed on.
//!
//! A port of what the Compose original does on focus: cross-fade the brightness step over 130 ms.
//! The original also scales the card 1.06; this port does not — see [`crate::ui::home::ringed`].
//! This port shipped without the fade because iced 0.13 had no per-widget animation; 0.14 added
//! `Animation`, which is what this is built on.

use std::time::{Duration, Instant};

use iced::animation::Easing;
use iced::{Animation, Color};

/// How long a hand-off takes. The Compose original's 130 ms.
const FADE: Duration = Duration::from_millis(130);

/// What the cursor is on, across every page that dims what it is not on.
///
/// One enum rather than a key per page, because there is one cursor: only a single item is focused
/// at a time, and moving between pages is just another move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// A card on Home: which shelf, and how far along it.
    HomeCard(usize, usize),
    /// A tile in a system's Browse grid, by position.
    BrowseTile(usize),
    /// The hero, a filter chip, the letter rail — anywhere that is not one of the dimmed items
    /// above. Carried rather than dropped so that moving *off* a card still fades that card out.
    Elsewhere,
}

/// A cross-fade between the item the cursor left and the one it landed on.
///
/// One `Animation` for the whole app, not one per card. Exactly one item is focused at a time, so
/// the transition is a single 0 -> 1 ramp read from both ends: `t` for the item being entered,
/// `1 - t` for the one being left. That is also what keeps it workable at all — the view is rebuilt
/// from scratch every frame and has nowhere of its own to keep per-card state.
#[derive(Debug)]
pub struct FocusFade {
    /// The item the cursor just left. `None` until it has moved at least once.
    previous: Option<Cursor>,
    /// 0 the instant the cursor moved, 1 once the hand-off has finished.
    progress: Animation<bool>,
}

/// A fresh ramp. `Animation` is a builder, so the easing and duration have to be re-applied every
/// time one is rebuilt.
fn ramp(settled: bool) -> Animation<bool> {
    Animation::new(settled).easing(Easing::EaseOut).duration(FADE)
}

impl Default for FocusFade {
    fn default() -> Self {
        // Settled rather than starting: on the first frame the card under the cursor should
        // already be lit, not fading up from dim.
        Self { previous: None, progress: ramp(true) }
    }
}

impl FocusFade {
    /// Start a hand-off away from `from`. Called once per move, from `App::update`.
    ///
    /// The ramp is rebuilt rather than reversed. A move that lands part-way through a fade should
    /// take the full 130 ms from where the new card actually is; driving the existing animation
    /// back down would give the second move whatever time the first one had left.
    pub fn moved(&mut self, from: Cursor, at: Instant) {
        self.previous = Some(from);
        self.progress = ramp(false);
        self.progress.go_mut(true, at);
    }

    /// How focused `item` is right now: 1 fully lit, 0 fully dimmed.
    ///
    /// `focused` is the caller's own answer to "is the cursor on this one", because the pages
    /// already derive that from their focus model and two answers to it would eventually disagree.
    pub fn amount(&self, item: Cursor, focused: bool, at: Instant) -> f32 {
        let t = self.progress.interpolate(0.0, 1.0, at);
        if focused {
            t
        } else if self.previous == Some(item) {
            1.0 - t
        } else {
            0.0
        }
    }

    /// Whether a hand-off is still running, so the caller can ask for frames only while one is.
    pub fn is_animating(&self, at: Instant) -> bool {
        self.progress.is_animating(at)
    }
}

/// How long the spotlight's picture takes to hand over to the next game's.
///
/// Three times a focus fade: this is the whole screen changing, and a quick cut of that much
/// picture reads as a flash rather than as the page following the cursor.
const CROSSFADE: Duration = Duration::from_millis(420);

/// Home's backdrop: the game whose picture is up, and the one it is fading in over.
///
/// Both layers fade, the old one out as the new one comes in, rather than the new one being laid
/// over a fixed old one. Over a picture faded at its own edges the second way leaves the old
/// picture showing through those edges until the moment it is dropped, and then it pops.
#[derive(Debug)]
pub struct Backdrop {
    /// The game whose picture is coming up, or is up.
    shown: Option<i64>,
    /// The game whose picture is going, and how much of it there was when it started to go — a
    /// move part-way through a hand-off takes the picture from where it had got to, not from full.
    leaving: Option<(i64, f32)>,
    /// 0 the moment the hand-off started, 1 once it is done.
    progress: Animation<bool>,
    /// A game the cursor landed on whose picture is still on its way — shown the moment it lands,
    /// so a slow picture does not fade the old one out to nothing first.
    waiting: Option<i64>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Self { shown: None, leaving: None, progress: Animation::new(true).duration(CROSSFADE), waiting: None }
    }
}

impl Backdrop {
    /// Bring `rom_id`'s picture up over whatever is up now. Nothing happens for the one already up.
    pub fn show(&mut self, rom_id: i64, at: Instant) {
        self.waiting = None;
        if self.shown == Some(rom_id) {
            return;
        }
        let t = self.progress.interpolate(0.0, 1.0, at);
        self.leaving = self.shown.map(|id| (id, t));
        self.shown = Some(rom_id);
        self.progress = Animation::new(false).easing(Easing::EaseInOut).duration(CROSSFADE);
        self.progress.go_mut(true, at);
    }

    /// Show `rom_id` once its picture has landed — see [`Backdrop::waiting`].
    pub fn wait_for(&mut self, rom_id: i64) {
        self.waiting = (self.shown != Some(rom_id)).then_some(rom_id);
    }

    /// The game whose picture the backdrop is waiting on, if any.
    pub fn waiting(&self) -> Option<i64> {
        self.waiting
    }

    /// What to draw, bottom first: each game with how much of its picture shows.
    pub fn layers(&self, at: Instant) -> impl Iterator<Item = (i64, f32)> {
        let t = self.progress.interpolate(0.0, 1.0, at);
        let leaving = self.leaving.filter(|_| t < 1.0).map(|(id, from)| (id, from * (1.0 - t)));
        leaving.into_iter().chain(self.shown.map(|id| (id, t)))
    }

    /// Whether a hand-off is running, so frames are asked for only while one is.
    pub fn is_animating(&self, at: Instant) -> bool {
        self.progress.is_animating(at)
    }
}

/// How long Home's shelves take to slide up or down a row.
const GLIDE: Duration = Duration::from_millis(280);

/// Where Home's shelves are scrolled to, in rows: the shelf with the cursor sits at the top of the
/// window under the spotlight, and moving to another slides the whole column there.
///
/// A position rather than an offset in pixels, because every shelf is one height — so the
/// position says which shelves to lay out at all, and a shelf above the window is simply not built.
#[derive(Debug)]
pub struct Glide {
    from: f32,
    to: f32,
    progress: Animation<bool>,
}

impl Default for Glide {
    fn default() -> Self {
        Self { from: 0.0, to: 0.0, progress: Animation::new(true).duration(GLIDE) }
    }
}

impl Glide {
    /// Slide to `row`, from wherever the column is right now — part-way through a slide included.
    pub fn go(&mut self, row: usize, at: Instant) {
        let target = row as f32;
        if target == self.to {
            return;
        }
        self.from = self.position(at);
        self.to = target;
        self.progress = Animation::new(false).easing(Easing::EaseOutCubic).duration(GLIDE);
        self.progress.go_mut(true, at);
    }

    /// Be at `row` already — for a page that opens there, where a slide would only be noise.
    pub fn jump(&mut self, row: usize) {
        *self = Self { from: row as f32, to: row as f32, ..Self::default() };
    }

    fn position(&self, at: Instant) -> f32 {
        lerp(self.from, self.to, self.progress.interpolate(0.0, 1.0, at))
    }

    /// The first shelf to lay out, and how far into it the window starts, in rows.
    ///
    /// The first shelf is the higher of where the column is and where it is going, so a shelf the
    /// column is sliding *down* to is built from the first frame — above the window, but there for
    /// the scroll that reveals its card to find.
    pub fn window(&self, at: Instant) -> (usize, f32) {
        let position = self.position(at);
        let first = position.min(self.to).floor().max(0.0);
        (first as usize, position - first)
    }

    /// Whether a slide is running, so frames are asked for only while one is.
    pub fn is_animating(&self, at: Instant) -> bool {
        self.progress.is_animating(at)
    }
}

/// The value between `dim` and `lit` at a given focus `amount`.
pub fn lerp(dim: f32, lit: f32, amount: f32) -> f32 {
    dim + (lit - dim) * amount
}

/// The colour between `dim` and `lit` at a given focus `amount`, for the parts of a card that dim
/// through their ink rather than through an image opacity.
///
/// Mixed in linear space, which is where a half-way blend actually looks half way: sRGB components
/// are gamma-encoded, so averaging them directly darkens the midpoint.
pub fn mix(dim: Color, lit: Color, amount: f32) -> Color {
    let (d, l) = (dim.into_linear(), lit.into_linear());
    Color::from_linear_rgba(
        lerp(d[0], l[0], amount),
        lerp(d[1], l[1], amount),
        lerp(d[2], l[2], amount),
        lerp(d[3], l[3], amount),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(n: usize) -> Cursor {
        Cursor::HomeCard(0, n)
    }

    #[test]
    fn a_settled_cursor_lights_its_card_fully_and_dims_the_rest() {
        let fade = FocusFade::default();
        let now = Instant::now();
        assert_eq!(fade.amount(card(0), true, now), 1.0, "the card under a resting cursor is lit");
        assert_eq!(fade.amount(card(1), false, now), 0.0);
        assert!(!fade.is_animating(now), "nothing animates until the cursor moves");
    }

    #[test]
    fn a_move_hands_the_light_from_one_card_to_the_other() {
        let mut fade = FocusFade::default();
        let start = Instant::now();
        fade.moved(card(0), start);

        // At the moment of the move the old card still holds all of it and the new one none.
        assert_eq!(fade.amount(card(1), true, start), 0.0);
        assert_eq!(fade.amount(card(0), false, start), 1.0);

        // Part way through, both are lit somewhere in between, and together they still make one
        // card's worth of light — that is what makes it a cross-fade rather than two fades.
        let mid = start + FADE / 2;
        let entering = fade.amount(card(1), true, mid);
        let leaving = fade.amount(card(0), false, mid);
        assert!(entering > 0.0 && entering < 1.0, "entering card is part way: {entering}");
        assert!((entering + leaving - 1.0).abs() < 1e-6);

        let after = start + FADE * 2;
        assert_eq!(fade.amount(card(1), true, after), 1.0);
        assert_eq!(fade.amount(card(0), false, after), 0.0);
        assert!(!fade.is_animating(after), "the ramp stops when it lands");
    }

    #[test]
    fn a_card_the_cursor_never_touched_stays_dim_throughout() {
        let mut fade = FocusFade::default();
        let start = Instant::now();
        fade.moved(card(0), start);
        for at in [start, start + FADE / 2, start + FADE * 2] {
            assert_eq!(fade.amount(card(9), false, at), 0.0);
        }
    }

    #[test]
    fn leaving_a_card_for_the_hero_still_fades_that_card_out() {
        // Nothing on the shelves is focused once the cursor is on the hero, so without carrying
        // the card it left, the shelf would snap to dim while the hero faded up.
        let mut fade = FocusFade::default();
        let start = Instant::now();
        fade.moved(card(0), start);
        assert_eq!(fade.amount(card(0), false, start), 1.0);
        assert!(fade.amount(card(0), false, start + FADE / 2) > 0.0);
    }

    #[test]
    fn a_new_picture_fades_in_as_the_old_one_fades_out() {
        let mut backdrop = Backdrop::default();
        let start = Instant::now();
        backdrop.show(1, start);
        let settled = start + CROSSFADE * 2;
        assert_eq!(backdrop.layers(settled).collect::<Vec<_>>(), vec![(1, 1.0)], "one picture once it lands");

        backdrop.show(2, settled);
        let layers: Vec<_> = backdrop.layers(settled + CROSSFADE / 2).collect();
        assert_eq!(layers.len(), 2, "both, part way through");
        let ((old, out), (new, coming)) = (layers[0], layers[1]);
        assert_eq!((old, new), (1, 2), "the old one underneath");
        assert!(out > 0.0 && out < 1.0 && coming > 0.0 && coming < 1.0);
        assert_eq!(backdrop.layers(settled + CROSSFADE * 2).collect::<Vec<_>>(), vec![(2, 1.0)]);
    }

    #[test]
    fn a_hand_off_cut_short_takes_the_picture_from_where_it_had_got_to() {
        let mut backdrop = Backdrop::default();
        let start = Instant::now();
        backdrop.show(1, start);
        backdrop.show(2, start + CROSSFADE * 2);
        let mid = start + CROSSFADE * 2 + CROSSFADE / 2;
        let half = backdrop.layers(mid).last().unwrap().1;
        backdrop.show(3, mid);
        let first = backdrop.layers(mid).next().unwrap();
        assert_eq!(first.0, 2);
        assert!((first.1 - half).abs() < 1e-4, "2 leaves from {half}, not from full: {}", first.1);
    }

    #[test]
    fn a_picture_still_on_its_way_is_waited_for_and_the_one_up_is_not() {
        let mut backdrop = Backdrop::default();
        backdrop.show(1, Instant::now());
        backdrop.wait_for(1);
        assert_eq!(backdrop.waiting(), None, "already up");
        backdrop.wait_for(2);
        assert_eq!(backdrop.waiting(), Some(2));
        backdrop.show(2, Instant::now());
        assert_eq!(backdrop.waiting(), None, "shown, so no longer waited for");
    }

    #[test]
    fn the_shelves_slide_with_the_row_they_are_going_to_already_built() {
        let mut glide = Glide::default();
        let start = Instant::now();
        assert_eq!(glide.window(start), (0, 0.0));

        // Down a row: the shelf being left stays built while it slides out of the top.
        glide.go(1, start);
        let (first, into) = glide.window(start + GLIDE / 2);
        assert_eq!(first, 0);
        assert!(into > 0.0 && into < 1.0);
        assert_eq!(glide.window(start + GLIDE * 2), (1, 0.0));

        // Back up: the shelf coming down is built from the very first frame, above the window.
        let later = start + GLIDE * 2;
        glide.go(0, later);
        assert_eq!(glide.window(later), (0, 1.0), "row 0 built, a whole row above the window");
        assert!(glide.is_animating(later + GLIDE / 2));
        assert_eq!(glide.window(later + GLIDE * 2), (0, 0.0));

        glide.jump(3);
        assert_eq!(glide.window(later), (3, 0.0), "a jump is already there");
    }

    #[test]
    fn mixing_a_colour_with_itself_changes_nothing() {
        let c = Color::from_rgb(0.2, 0.4, 0.6);
        for amount in [0.0, 0.5, 1.0] {
            let mixed = mix(c, c, amount);
            assert!((mixed.r - c.r).abs() < 1e-5 && (mixed.b - c.b).abs() < 1e-5);
        }
    }
}
