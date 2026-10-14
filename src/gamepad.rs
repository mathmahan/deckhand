//! Gamepad input — the half of the Kotlin app's `JsIntentBackend` and `RepeatEngine` this build was
//! missing. The cursor model every page already has is what this feeds.
//!
//! Pads are read through `gilrs`, whose SDL mappings make a Deck's controls (through Steam Input),
//! an Xbox pad and a DualSense all read as one layout. The layout:
//!
//! | Pad | Does |
//! |---|---|
//! | D-pad, left stick | move the cursor, repeating while held |
//! | A | confirm |
//! | B | back |
//! | Y | search |
//! | X | backspace, on the Search page |
//! | L1 / R1 | previous / next tab |
//! | L2 / R2 | previous / next system |
//! | Start | Settings |
//! | Select | Downloads |
//!
//! Held directions repeat with the Kotlin app's timings: a pause before the first repeat so a tap
//! never moves two, then a steady rate, then faster once it has been held a while — walking a long
//! list should not take as long as reading it.
//!
//! A pad is read on a thread of its own, since `gilrs` blocks and must stay on one thread. Nothing
//! here knows whether a game is running: the app ignores the pad while one is, which is what keeps
//! the grid from walking behind the game while it is being played.

use std::time::{Duration, Instant};

use futures::SinkExt;
use futures::channel::mpsc::Sender;
use gilrs::{Axis, Button, EventType, Gilrs};

use crate::app::Direction;

/// What a pad asked for, in this app's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pad {
    Nav(Direction),
    Confirm,
    Back,
    Search,
    Erase,
    PrevTab,
    NextTab,
    PrevSystem,
    NextSystem,
    Start,
    Select,
}

const INITIAL_DELAY: Duration = Duration::from_millis(350);
const REPEAT: Duration = Duration::from_millis(90);
const ACCEL_AFTER: Duration = Duration::from_millis(900);
const ACCEL_REPEAT: Duration = Duration::from_millis(45);

/// How far a stick has to lean to count as a press, and how far back to let go. Two thresholds, so a
/// stick resting near one does not chatter between pressed and released.
const STICK_PRESS: f32 = 0.6;
const STICK_RELEASE: f32 = 0.35;

/// Hold-to-repeat for one held direction.
#[derive(Debug, Default)]
pub struct Repeat {
    held: Option<(Direction, Instant, Instant)>,
}

impl Repeat {
    /// The direction held now, or none. A change of direction is a fresh press: it moves at once.
    pub fn set(&mut self, direction: Option<Direction>, now: Instant) -> Option<Direction> {
        match (direction, self.held) {
            (None, _) => {
                self.held = None;
                None
            }
            (Some(d), Some((held, _, _))) if d == held => None,
            (Some(d), _) => {
                self.held = Some((d, now, now));
                Some(d)
            }
        }
    }

    /// A repeat, if one is due.
    pub fn tick(&mut self, now: Instant) -> Option<Direction> {
        let (direction, pressed, last) = self.held?;
        let held = now.duration_since(pressed);
        let due = if held < INITIAL_DELAY {
            return None;
        } else if last == pressed {
            pressed + INITIAL_DELAY
        } else if held >= ACCEL_AFTER {
            last + ACCEL_REPEAT
        } else {
            last + REPEAT
        };
        (now >= due).then(|| {
            self.held = Some((direction, pressed, now));
            direction
        })
    }
}

/// The left stick's direction, given which way it was already read: the dominant axis past the
/// press threshold, held until it falls back under the release one.
pub fn stick(x: f32, y: f32, was: Option<Direction>) -> Option<Direction> {
    let lean = x.abs().max(y.abs());
    let threshold = if was.is_some() { STICK_RELEASE } else { STICK_PRESS };
    if lean < threshold {
        return None;
    }
    // gilrs reads up as positive Y.
    Some(if x.abs() > y.abs() {
        if x > 0.0 { Direction::Right } else { Direction::Left }
    } else if y > 0.0 {
        Direction::Up
    } else {
        Direction::Down
    })
}

fn button(button: Button) -> Option<Pad> {
    Some(match button {
        Button::South => Pad::Confirm,
        Button::East => Pad::Back,
        Button::North => Pad::Search,
        Button::West => Pad::Erase,
        Button::LeftTrigger => Pad::PrevTab,
        Button::RightTrigger => Pad::NextTab,
        Button::LeftTrigger2 => Pad::PrevSystem,
        Button::RightTrigger2 => Pad::NextSystem,
        Button::Start => Pad::Start,
        Button::Select => Pad::Select,
        _ => return None,
    })
}

fn dpad(button: Button) -> Option<Direction> {
    Some(match button {
        Button::DPadUp => Direction::Up,
        Button::DPadDown => Direction::Down,
        Button::DPadLeft => Direction::Left,
        Button::DPadRight => Direction::Right,
        _ => return None,
    })
}

/// Every pad's input, for as long as the app runs. Ends at once, quietly, where pads cannot be read
/// at all — the app works the same without one.
pub fn stream() -> impl futures::Stream<Item = Pad> {
    iced::stream::channel(64, |output: Sender<Pad>| async move {
        let reader = std::thread::Builder::new().name("gamepad".into()).spawn(move || read(output));
        if let Err(e) = reader {
            eprintln!("gamepad: could not start the reader: {e}");
        }
        // The reader owns the sender; this future only has to outlive it, which it does by never
        // finishing.
        std::future::pending::<()>().await;
    })
}

fn read(mut output: Sender<Pad>) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(e) => return eprintln!("gamepad: pads cannot be read here: {e}"),
    };
    let mut send = |pad: Pad| futures::executor::block_on(output.send(pad)).is_ok();
    let mut repeat = Repeat::default();
    let mut pad_held: Option<Direction> = None;
    let mut stick_held: Option<Direction> = None;
    let (mut x, mut y) = (0.0f32, 0.0f32);

    loop {
        // While a direction is held, wake often enough that its repeat is due on time; with nothing
        // held there is nothing to be on time for, so sleep until the pad says something — a
        // handheld's battery pays for every wake-up, and this once woke 67 times a second for good.
        let held = pad_held.or(stick_held).is_some();
        let event = gilrs.next_event_blocking(held.then_some(Duration::from_millis(15)));
        let now = Instant::now();
        if let Some(event) = event {
            match event.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(d) = dpad(b) {
                        pad_held = Some(d);
                    } else if let Some(pad) = button(b)
                        && !send(pad)
                    {
                        return;
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if dpad(b).is_some_and(|d| pad_held == Some(d)) {
                        pad_held = None;
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, value, _) => x = value,
                EventType::AxisChanged(Axis::LeftStickY, value, _) => y = value,
                EventType::Disconnected => {
                    (pad_held, stick_held, x, y) = (None, None, 0.0, 0.0);
                }
                _ => {}
            }
            stick_held = stick(x, y, stick_held);
        }
        // The D-pad wins over the stick when both are held.
        let moved = repeat.set(pad_held.or(stick_held), now).or_else(|| repeat.tick(now));
        if let Some(d) = moved
            && !send(Pad::Nav(d))
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_direction_waits_then_repeats_then_speeds_up() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut r = Repeat::default();
        assert_eq!(r.set(Some(Direction::Down), at(0)), Some(Direction::Down), "a press moves at once");
        assert_eq!(r.set(Some(Direction::Down), at(10)), None, "still held is not a new press");
        assert_eq!(r.tick(at(300)), None, "a tap never moves two");
        assert_eq!(r.tick(at(350)), Some(Direction::Down));
        assert_eq!(r.tick(at(400)), None);
        assert_eq!(r.tick(at(440)), Some(Direction::Down), "then every 90 ms");
        assert_eq!(r.tick(at(1000)), Some(Direction::Down));
        assert_eq!(r.tick(at(1030)), None);
        assert_eq!(r.tick(at(1045)), Some(Direction::Down), "and faster once held a while");
        assert_eq!(r.set(Some(Direction::Left), at(1050)), Some(Direction::Left), "a new direction moves at once");
        assert_eq!(r.set(None, at(1060)), None);
        assert_eq!(r.tick(at(2000)), None, "released is released");
    }

    #[test]
    fn the_stick_has_to_lean_to_press_and_come_back_to_release() {
        assert_eq!(stick(0.5, 0.0, None), None);
        assert_eq!(stick(0.7, 0.2, None), Some(Direction::Right));
        assert_eq!(stick(0.4, 0.0, Some(Direction::Right)), Some(Direction::Right), "held through the dead band");
        assert_eq!(stick(0.3, 0.0, Some(Direction::Right)), None);
        assert_eq!(stick(0.1, -0.9, None), Some(Direction::Down), "up is positive Y");
    }
}
