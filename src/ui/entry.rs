//! Typing a setting in — a token, a command line — with the pad as well as a keyboard. The whole
//! window, like the picker: the field over an on-screen keyboard that has letters, capitals, and the
//! symbols paths and command lines are written with.
//!
//! A real keyboard types straight into the field, and Ctrl+V pastes. On the pad, the D-pad walks the
//! keys and A presses one; X erases, Y switches to capitals, Start saves and B cancels.

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length};

use crate::app::{Direction, Message};
use crate::theme;

const LETTERS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl-", "zxcvbnm_./"];
const CAPITALS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL-", "ZXCVBNM_./"];
const SYMBOLS: [&str; 4] = ["1234567890", "!@#$%^&*()", "[]{}<>|\\~`", "'\":;,=+?./"];

/// The bottom row, and how wide each of its keys is against a character key's 1.
const CONTROLS: [(Control, f32); 6] = [
    (Control::Shift, 1.5),
    (Control::Symbols, 1.5),
    (Control::Space, 3.0),
    (Control::Erase, 1.5),
    (Control::Paste, 1.5),
    (Control::Done, 1.0),
];
const ROWS: usize = 5;
const KEY_H: f32 = 44.0;
const KEY_GAP: f32 = 8.0;
const KEYBOARD_W: f32 = 10.0 * 56.0 + 9.0 * KEY_GAP;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    #[default]
    Letters,
    Capitals,
    Symbols,
}

/// A key that is not a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Capitals on and off.
    Shift,
    /// Symbols on and off.
    Symbols,
    Space,
    Erase,
    Paste,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Control(Control),
}

/// The field, its question, and where the cursor is on the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    /// What goes in, and what an empty field means.
    pub note: String,
    pub text: String,
    /// Why the last save was turned down, under the field.
    pub error: Option<String>,
    pub layer: Layer,
    row: usize,
    col: usize,
}

impl Entry {
    pub fn new(title: impl Into<String>, note: impl Into<String>, text: impl Into<String>) -> Self {
        // The cursor starts on Done: the field arrives filled with what is set now, and leaving it
        // as it is should be one press.
        let done = CONTROLS.iter().position(|(c, _)| *c == Control::Done).unwrap_or(0);
        Self {
            title: title.into(),
            note: note.into(),
            text: text.into(),
            error: None,
            layer: Layer::Letters,
            row: ROWS - 1,
            col: done,
        }
    }

    fn characters(&self) -> [&'static str; 4] {
        match self.layer {
            Layer::Letters => LETTERS,
            Layer::Capitals => CAPITALS,
            Layer::Symbols => SYMBOLS,
        }
    }

    /// Each key of `row` as the span of the keyboard's width it covers, 0.0 to 1.0.
    fn spans(&self, row: usize) -> Vec<(f32, f32)> {
        let weights: Vec<f32> = if row < ROWS - 1 {
            vec![1.0; self.characters()[row].chars().count()]
        } else {
            CONTROLS.iter().map(|(_, w)| *w).collect()
        };
        let total: f32 = weights.iter().sum();
        let mut at = 0.0;
        weights
            .iter()
            .map(|w| {
                let span = (at / total, (at + w) / total);
                at += w;
                span
            })
            .collect()
    }

    pub fn key_at(&self, row: usize, col: usize) -> Option<Key> {
        if row < ROWS - 1 {
            self.characters()[row].chars().nth(col).map(Key::Char)
        } else {
            CONTROLS.get(col).map(|(c, _)| Key::Control(*c))
        }
    }

    /// The key under the cursor.
    pub fn key(&self) -> Key {
        self.key_at(self.row, self.col).unwrap_or(Key::Control(Control::Done))
    }

    /// Whether the cursor is on this key.
    pub fn is_on(&self, row: usize, col: usize) -> bool {
        (self.row, self.col) == (row, col)
    }

    /// Move the cursor. Up and Down land on the key under the one it leaves, keys being of different
    /// widths on the bottom row; Left and Right wrap within a row. Answers whether it moved.
    pub fn step(&mut self, direction: Direction) -> bool {
        let before = (self.row, self.col);
        let width = self.spans(self.row).len();
        match direction {
            Direction::Left => self.col = (self.col + width - 1) % width,
            Direction::Right => self.col = (self.col + 1) % width,
            Direction::Up | Direction::Down => {
                let target = match direction {
                    Direction::Up if self.row > 0 => self.row - 1,
                    Direction::Down if self.row + 1 < ROWS => self.row + 1,
                    _ => return false,
                };
                let (from, to) = self.spans(self.row)[self.col];
                let middle = (from + to) / 2.0;
                let spans = self.spans(target);
                self.col = spans.iter().position(|(a, b)| middle >= *a && middle < *b).unwrap_or(spans.len() - 1);
                self.row = target;
            }
        }
        before != (self.row, self.col)
    }

    /// Put the cursor on a key — where a click landed.
    pub fn focus(&mut self, key: Key) {
        for row in 0..ROWS {
            for col in 0..self.spans(row).len() {
                if self.key_at(row, col) == Some(key) {
                    (self.row, self.col) = (row, col);
                    return;
                }
            }
        }
    }

    /// Type what a keyboard or the clipboard gave, leaving out line breaks and other control
    /// characters: every field here is one line.
    pub fn type_str(&mut self, typed: &str) {
        self.text.extend(typed.chars().filter(|c| !c.is_control()));
        self.error = None;
    }

    pub fn erase(&mut self) {
        self.text.pop();
        self.error = None;
    }

    /// Press `key`. Typing and the layer keys are done here; Paste and Done are the caller's, and
    /// are handed back.
    pub fn press(&mut self, key: Key) -> Option<Control> {
        match key {
            Key::Char(c) => self.type_str(&c.to_string()),
            Key::Control(Control::Space) => self.type_str(" "),
            Key::Control(Control::Erase) => self.erase(),
            Key::Control(Control::Shift) => {
                self.layer = if self.layer == Layer::Capitals { Layer::Letters } else { Layer::Capitals };
            }
            Key::Control(Control::Symbols) => {
                self.layer = if self.layer == Layer::Symbols { Layer::Letters } else { Layer::Symbols };
            }
            Key::Control(other @ (Control::Paste | Control::Done)) => return Some(other),
        }
        None
    }
}

pub fn view<'a>(entry: &Entry) -> Element<'a, Message> {
    let field = container(
        text(format!("{}▌", entry.text))
            .size(theme::text_size::TITLE_MEDIUM)
            .color(theme::TEXT_PRIMARY)
            .wrapping(Wrapping::Glyph),
    )
    .width(Length::Fill)
    .padding([12, 16])
    .style(|_| container::Style {
        background: Some(Background::Color(theme::SURFACE)),
        border: Border { color: theme::FOCUS_RING, width: 2.0, radius: 12.0.into() },
        ..Default::default()
    });

    let mut card = column![
        text(entry.title.clone()).size(theme::text_size::HEADLINE_SMALL).font(theme::MEDIUM).color(theme::TEXT_PRIMARY),
        text(entry.note.clone()).size(theme::text_size::BODY_MEDIUM).color(theme::TEXT_SECONDARY),
        Space::new().height(Length::Fixed(4.0)),
        field,
    ]
    .spacing(12)
    .max_width(KEYBOARD_W + 120.0);
    if let Some(error) = &entry.error {
        card = card.push(text(error.clone()).size(theme::text_size::BODY_MEDIUM).color(theme::ERROR));
    }
    card = card
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(container(keyboard(entry)).center_x(Length::Fill))
        .push(
            text("Type, or walk the keys and press A  ·  X erases  ·  Y capitals  ·  Enter or Start saves  ·  Esc or B cancels")
                .size(theme::text_size::LABEL_MEDIUM)
                .color(theme::TEXT_MUTED),
        );

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([40, 28])
        .center_x(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(theme::BACKGROUND)), ..Default::default() })
        .into()
}

fn keyboard<'a>(entry: &Entry) -> Element<'a, Message> {
    let mut rows = column![].spacing(KEY_GAP).width(Length::Fixed(KEYBOARD_W));
    for r in 0..ROWS {
        let mut line = row![].spacing(KEY_GAP).align_y(Alignment::Center);
        let keys: Vec<(Key, f32)> = if r < ROWS - 1 {
            entry.characters()[r].chars().map(|c| (Key::Char(c), 1.0)).collect()
        } else {
            CONTROLS.iter().map(|(c, w)| (Key::Control(*c), *w)).collect()
        };
        for (c, (key, weight)) in keys.into_iter().enumerate() {
            let lit = entry.is_on(r, c);
            // The layer keys stay marked while their layer is up.
            let on = matches!(
                (key, entry.layer),
                (Key::Control(Control::Shift), Layer::Capitals) | (Key::Control(Control::Symbols), Layer::Symbols)
            );
            let label = match key {
                Key::Char(c) => c.to_string(),
                Key::Control(Control::Shift) => "⇧ ABC".into(),
                Key::Control(Control::Symbols) => "#+=".into(),
                Key::Control(Control::Space) => "space".into(),
                Key::Control(Control::Erase) => "⌫".into(),
                Key::Control(Control::Paste) => "Paste".into(),
                Key::Control(Control::Done) => "Save".into(),
            };
            line = line.push(
                button(
                    container(text(label).size(theme::text_size::BODY_LARGE).wrapping(Wrapping::None))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .width(Length::FillPortion((weight * 2.0) as u16))
                .height(Length::Fixed(KEY_H))
                .padding(0)
                .on_press(Message::EntryKey(key))
                .style(crate::ui::style::key(lit, on)),
            );
        }
        rows = rows.push(line);
    }
    rows.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_opens_on_save_and_walks_between_rows_of_different_widths() {
        let mut entry = Entry::new("Token", "", "abc");
        assert_eq!(entry.key(), Key::Control(Control::Done), "unchanged is one press");
        for _ in 0..3 {
            entry.step(Direction::Left);
        }
        assert_eq!(entry.key(), Key::Control(Control::Space));
        entry.step(Direction::Up);
        assert_eq!(entry.key(), Key::Char('b'), "the character above the middle of the space bar");
        entry.step(Direction::Down);
        entry.step(Direction::Right);
        entry.step(Direction::Right);
        entry.step(Direction::Right);
        entry.step(Direction::Right);
        assert_eq!(entry.key(), Key::Control(Control::Shift), "Right wraps along a row");
        assert!(!entry.step(Direction::Down), "nothing under the bottom row");
        for _ in 0..4 {
            entry.step(Direction::Up);
        }
        assert_eq!(entry.key(), Key::Char('1'));
        assert!(!entry.step(Direction::Up));
    }

    #[test]
    fn keys_type_and_the_layer_keys_switch_what_the_others_are() {
        let mut entry = Entry::new("Command", "", "");
        entry.press(Key::Char('a'));
        entry.press(Key::Control(Control::Shift));
        assert_eq!(entry.key_at(1, 0), Some(Key::Char('Q')));
        entry.press(Key::Char('B'));
        entry.press(Key::Control(Control::Symbols));
        assert_eq!(entry.key_at(2, 2), Some(Key::Char('{')));
        entry.press(Key::Char('{'));
        entry.press(Key::Control(Control::Space));
        entry.press(Key::Control(Control::Erase));
        assert_eq!(entry.text, "aB{");
        assert_eq!(entry.press(Key::Control(Control::Done)), Some(Control::Done), "the caller's to do");
        entry.type_str("pasted\nline\t");
        assert_eq!(entry.text, "aB{pastedline", "one line, whatever was pasted");
        entry.focus(Key::Char('|'));
        assert_eq!(entry.key(), Key::Char('|'));
    }
}
