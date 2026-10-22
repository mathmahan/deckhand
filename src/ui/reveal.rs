//! Scrolling a row into view by *measuring* how much of the page is on screen.
//!
//! Everywhere else in this app a scroll offset is computed in `update`, because rows and cards are
//! fixed-size by construction and iced reports no layout back. Where a row *is* is still computed
//! here. What is not is how much of the page a scrollable shows: the Browse grid's viewport and the
//! system list's are whatever the tab bar, the header, the chips and a banner that comes and goes
//! leave over, and the window height is only an over-estimate of that.
//!
//! So this asks the layout. A widget operation runs against the interface the runtime has just
//! rebuilt for the new state, reads the scrollable's viewport, and scrolls by as little as brings the
//! row into view, so a cursor walking inside what is already visible does not drag the page along
//! with it.
//!
//! The scroll happens inside the operation, chained straight onto the measurement, rather than
//! being handed back to `update` to issue. A round trip would leave a gap where a second key press
//! measures the page before the first one's scroll has landed, and on a held key that is the
//! ordinary case.

use iced::advanced::widget::operation::{Operation, Outcome, Scrollable};
use iced::advanced::widget::Id;
use iced::widget::scrollable::AbsoluteOffset;
use iced::{Rectangle, Task, Vector};

/// Which way a scrollable scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

/// Scroll `scrollable` until the rows from `top` to `bottom` — in content coordinates — are fully in
/// view with `margin` clear beside them, producing the offset it moved to. Produces nothing, and
/// moves nothing, when they already are or the scrollable is not on screen.
pub fn reveal(scrollable: Id, top: f32, bottom: f32, margin: f32) -> Task<f32> {
    iced::advanced::widget::operate(Reveal { axis: Axis::Vertical, scrollable, top, bottom, margin, found: None })
}

/// [`reveal`] for a row that scrolls sideways: the span from `left` to `right`.
pub fn reveal_across(scrollable: Id, left: f32, right: f32, margin: f32) -> Task<f32> {
    iced::advanced::widget::operate(Reveal { axis: Axis::Horizontal, scrollable, top: left, bottom: right, margin, found: None })
}

/// [`reveal`] for a part of a page whose place is the layout's to say rather than a sum — one under
/// text that wraps to however many lines the window gives it: the container `part`, found and
/// measured in the same pass as the scrollable it is in.
pub fn reveal_part(scrollable: Id, part: Id, margin: f32) -> Task<f32> {
    iced::advanced::widget::operate(RevealPart { measure: Measure::new(scrollable, part), margin })
}

/// Up toward the top of the container `part`, `step` at a time: while its top is above the view,
/// scroll by as much as `step` and produce where to — `Some` — and once it is in view, move nothing
/// and produce `None`. How a list longer than the screen is read back up before the cursor leaves it.
pub fn climb(scrollable: Id, part: Id, step: f32, margin: f32) -> Task<Option<f32>> {
    iced::advanced::widget::operate(Climb { measure: Measure::new(scrollable, part), step, margin })
}

/// The smallest move of a viewport `len` tall, now at `offset`, that shows `top..bottom` with
/// `margin` clear on either side. When the span cannot fit, its top wins: the top of a row is the
/// part that says what it is.
/// None when no move is needed.
pub fn offset_to_reveal(top: f32, bottom: f32, offset: f32, len: f32, margin: f32) -> Option<f32> {
    let (lead, tail) = (top - margin, bottom + margin);
    let target = if lead < offset {
        lead
    } else if tail > offset + len {
        (tail - len).min(lead)
    } else {
        return None;
    };
    // Already there — the start, wanted with a margin before it there is no room for.
    let target = target.max(0.0);
    (target != offset).then_some(target)
}

struct Reveal {
    axis: Axis,
    scrollable: Id,
    top: f32,
    bottom: f32,
    margin: f32,
    /// Where the scrollable is now, and how tall its viewport is.
    found: Option<(f32, f32)>,
}

impl Operation<f32> for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scrollable) {
            self.found = Some(match self.axis {
                Axis::Vertical => (translation.y, bounds.height),
                Axis::Horizontal => (translation.x, bounds.width),
            });
        }
    }

    fn finish(&self) -> Outcome<f32> {
        let Some((offset, len)) = self.found else { return Outcome::None };
        match offset_to_reveal(self.top, self.bottom, offset, len, self.margin) {
            Some(offset) => Outcome::Chain(Box::new(ScrollTo { axis: self.axis, scrollable: self.scrollable.clone(), offset, report: |offset| offset })),
            None => Outcome::None,
        }
    }
}

/// Where a scrollable is and how much of it shows, and where a container inside it is — read in one
/// pass over the interface.
struct Measure {
    scrollable: Id,
    part: Id,
    /// The scrollable's offset, its viewport's height, and the top of its content on screen.
    view: Option<(f32, f32, f32)>,
    /// The part's top and bottom, on screen as laid out — before the scrollable's offset moves it.
    span: Option<(f32, f32)>,
}

impl Measure {
    fn new(scrollable: Id, part: Id) -> Self {
        Self { scrollable, part, view: None, span: None }
    }

    fn scrollable(&mut self, id: Option<&Id>, bounds: Rectangle, content_bounds: Rectangle, translation: Vector) {
        if id == Some(&self.scrollable) {
            self.view = Some((translation.y, bounds.height, content_bounds.y));
        }
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        if id == Some(&self.part) {
            self.span = Some((bounds.y, bounds.y + bounds.height));
        }
    }

    /// The offset, the viewport's height, and the part's top and bottom in the content's own
    /// coordinates — what [`offset_to_reveal`] reads.
    fn read(&self) -> Option<(f32, f32, f32, f32)> {
        let (offset, len, origin) = self.view?;
        let (top, bottom) = self.span?;
        Some((offset, len, top - origin, bottom - origin))
    }
}

struct RevealPart {
    measure: Measure,
    margin: f32,
}

impl Operation<f32> for RevealPart {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        self.measure.container(id, bounds);
    }

    fn scrollable(&mut self, id: Option<&Id>, bounds: Rectangle, content_bounds: Rectangle, translation: Vector, _state: &mut dyn Scrollable) {
        self.measure.scrollable(id, bounds, content_bounds, translation);
    }

    fn finish(&self) -> Outcome<f32> {
        let Some((offset, len, top, bottom)) = self.measure.read() else { return Outcome::None };
        match offset_to_reveal(top, bottom, offset, len, self.margin) {
            Some(offset) => Outcome::Chain(Box::new(ScrollTo {
                axis: Axis::Vertical,
                scrollable: self.measure.scrollable.clone(),
                offset,
                report: |offset| offset,
            })),
            None => Outcome::None,
        }
    }
}

struct Climb {
    measure: Measure,
    step: f32,
    margin: f32,
}

impl Operation<Option<f32>> for Climb {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Option<f32>>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        self.measure.container(id, bounds);
    }

    fn scrollable(&mut self, id: Option<&Id>, bounds: Rectangle, content_bounds: Rectangle, translation: Vector, _state: &mut dyn Scrollable) {
        self.measure.scrollable(id, bounds, content_bounds, translation);
    }

    fn finish(&self) -> Outcome<Option<f32>> {
        let Some((offset, _, top, _)) = self.measure.read() else { return Outcome::None };
        match climb_to(top, offset, self.step, self.margin) {
            Some(offset) => Outcome::Chain(Box::new(ScrollTo {
                axis: Axis::Vertical,
                scrollable: self.measure.scrollable.clone(),
                offset,
                report: Some,
            })),
            None => Outcome::Some(None),
        }
    }
}

/// Where a step up from `offset` toward a part whose top is at `top` lands: as much as `step`, and
/// no further than shows the top with `margin` above it. None when that top is already in view.
pub fn climb_to(top: f32, offset: f32, step: f32, margin: f32) -> Option<f32> {
    let want = (top - margin).max(0.0);
    (want < offset - 0.5).then(|| (offset - step).max(want))
}

/// The second half of a reveal: move the scrollable, and report where to.
struct ScrollTo<T> {
    axis: Axis,
    scrollable: Id,
    offset: f32,
    /// What the move reports, made of where it went.
    report: fn(f32) -> T,
}

impl<T> Operation<T> for ScrollTo<T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
        _translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scrollable) {
            state.scroll_to(match self.axis {
                Axis::Vertical => AbsoluteOffset { x: None, y: Some(self.offset) },
                Axis::Horizontal => AbsoluteOffset { x: Some(self.offset), y: None },
            });
        }
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::Some((self.report)(self.offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_already_in_view_does_not_move_the_page() {
        assert_eq!(offset_to_reveal(322.0, 620.0, 0.0, 640.0, 0.0), None);
    }

    #[test]
    fn stepping_past_the_bottom_moves_just_far_enough_to_show_it() {
        // Row 2 of a ~322-unit pitch in a 600-unit viewport: its bottom lands on the viewport's.
        assert_eq!(offset_to_reveal(644.0, 942.0, 0.0, 600.0, 0.0), Some(342.0));
    }

    #[test]
    fn stepping_past_the_top_puts_the_span_at_the_top() {
        assert_eq!(offset_to_reveal(322.0, 620.0, 500.0, 600.0, 0.0), Some(322.0));
    }

    #[test]
    fn the_margin_is_kept_clear_on_the_side_it_moved_toward() {
        assert_eq!(offset_to_reveal(900.0, 1000.0, 0.0, 800.0, 60.0), Some(260.0));
        assert_eq!(offset_to_reveal(100.0, 200.0, 300.0, 800.0, 60.0), Some(40.0));
        assert_eq!(offset_to_reveal(10.0, 100.0, 300.0, 800.0, 60.0), Some(0.0), "never before the start");
    }

    #[test]
    fn climbing_steps_up_until_the_top_shows_then_stops() {
        assert_eq!(climb_to(900.0, 1400.0, 160.0, 24.0), Some(1240.0), "a step at a time while the top is far above");
        assert_eq!(climb_to(900.0, 950.0, 160.0, 24.0), Some(876.0), "no further than shows it");
        assert_eq!(climb_to(900.0, 876.0, 160.0, 24.0), None, "in view: the cursor may leave");
        assert_eq!(climb_to(900.0, 200.0, 160.0, 24.0), None, "below the view is not above it");
    }

    #[test]
    fn a_span_too_big_to_fit_shows_its_top() {
        assert_eq!(offset_to_reveal(1000.0, 1900.0, 0.0, 600.0, 0.0), Some(1000.0));
    }
}
