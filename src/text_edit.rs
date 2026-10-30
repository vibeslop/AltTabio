//! Selection and editing for the single-line filter. Offsets are always UTF-8 boundaries.

use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TextCommand {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Left,
    Right,
    SelectLeft,
    SelectRight,
    WordLeft,
    WordRight,
    SelectWordLeft,
    SelectWordRight,
    Start,
    End,
    SelectStart,
    SelectEnd,
    Delete,
    Backspace,
    DeleteWord,
    BackspaceWord,
}

impl TextCommand {
    pub const ALL: [Self; 22] = [
        Self::SelectAll,
        Self::Copy,
        Self::Cut,
        Self::Paste,
        Self::Undo,
        Self::Redo,
        Self::Left,
        Self::Right,
        Self::SelectLeft,
        Self::SelectRight,
        Self::WordLeft,
        Self::WordRight,
        Self::SelectWordLeft,
        Self::SelectWordRight,
        Self::Start,
        Self::End,
        Self::SelectStart,
        Self::SelectEnd,
        Self::Delete,
        Self::Backspace,
        Self::DeleteWord,
        Self::BackspaceWord,
    ];

    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    #[must_use]
    pub const fn for_key(key: u32, control: bool, shift: bool, right_alt: bool) -> Option<Self> {
        // Windows represents AltGr as Right Alt plus Ctrl. Its translated text must not first
        // execute a Ctrl editing command, even while the user holds Alt to keep switching.
        if control && right_alt {
            return None;
        }
        Some(match (key, control, shift) {
            (0x41, true, _) => Self::SelectAll,
            (0x43, true, _) | (0x2D, true, false) => Self::Copy,
            (0x58, true, _) | (0x2E, false, true) => Self::Cut,
            (0x56, true, _) | (0x2D, false, true) => Self::Paste,
            (0x5A, true, false) => Self::Undo,
            (0x59, true, _) | (0x5A, true, true) => Self::Redo,
            (0x25, false, false) => Self::Left,
            (0x27, false, false) => Self::Right,
            (0x25, false, true) => Self::SelectLeft,
            (0x27, false, true) => Self::SelectRight,
            (0x25, true, false) => Self::WordLeft,
            (0x27, true, false) => Self::WordRight,
            (0x25, true, true) => Self::SelectWordLeft,
            (0x27, true, true) => Self::SelectWordRight,
            (0x24, _, false) => Self::Start,
            (0x23, _, false) => Self::End,
            (0x24, _, true) => Self::SelectStart,
            (0x23, _, true) => Self::SelectEnd,
            (0x2E, true, _) => Self::DeleteWord,
            (0x2E, false, false) => Self::Delete,
            (0x08, true, _) => Self::BackspaceWord,
            (0x08, false, _) => Self::Backspace,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Snapshot {
    text: String,
    anchor: usize,
    caret: usize,
}

#[derive(Debug, Default)]
pub struct QueryText {
    current: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl QueryText {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current: Snapshot {
                text: String::new(),
                anchor: 0,
                caret: 0,
            },
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    #[must_use]
    pub fn text(&self) -> &str {
        &self.current.text
    }
    #[must_use]
    pub const fn caret(&self) -> usize {
        self.current.caret
    }
    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.current.anchor.min(self.current.caret)..self.current.anchor.max(self.current.caret)
    }
    #[must_use]
    pub fn selected_text(&self) -> &str {
        &self.text()[self.selection()]
    }

    pub fn reset(&mut self, text: &str) {
        self.current = Snapshot {
            text: text.to_owned(),
            anchor: text.len(),
            caret: text.len(),
        };
        self.undo.clear();
        self.redo.clear();
    }

    pub fn insert(&mut self, text: &str) {
        let text = text
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>();
        let range = self.selection();
        if range.is_empty() && text.is_empty() {
            return;
        }
        self.remember();
        self.current.text.replace_range(range.clone(), &text);
        self.move_to(range.start + text.len(), false);
    }

    pub fn command(&mut self, command: TextCommand) {
        use TextCommand as C;
        match command {
            C::SelectAll => {
                self.current.anchor = 0;
                self.current.caret = self.text().len();
            }
            C::Copy | C::Cut | C::Paste => {}
            C::Undo => Self::restore(&mut self.current, &mut self.undo, &mut self.redo),
            C::Redo => Self::restore(&mut self.current, &mut self.redo, &mut self.undo),
            C::Delete | C::Backspace | C::DeleteWord | C::BackspaceWord => self.delete(command),
            _ => self.navigate(command),
        }
    }

    pub fn select_at(&mut self, offset: usize, extend: bool) {
        let offset = offset.min(self.text().len());
        let boundary = self
            .text()
            .char_indices()
            .map(|(index, _)| index)
            .chain([self.text().len()])
            .take_while(|index| *index <= offset)
            .last()
            .unwrap_or_default();
        self.move_to(boundary, extend);
    }

    fn navigate(&mut self, command: TextCommand) {
        use TextCommand as C;
        let selection = self.selection();
        let (target, extend) = match command {
            C::Left if !selection.is_empty() => (selection.start, false),
            C::Right if !selection.is_empty() => (selection.end, false),
            C::Left | C::SelectLeft => (self.previous(), command == C::SelectLeft),
            C::Right | C::SelectRight => (self.next(), command == C::SelectRight),
            C::WordLeft | C::SelectWordLeft => (self.word_left(), command == C::SelectWordLeft),
            C::WordRight | C::SelectWordRight => (self.word_right(), command == C::SelectWordRight),
            C::Start | C::SelectStart => (0, command == C::SelectStart),
            C::End | C::SelectEnd => (self.text().len(), command == C::SelectEnd),
            _ => return,
        };
        self.move_to(target, extend);
    }

    fn delete(&mut self, command: TextCommand) {
        use TextCommand as C;
        if self.selection().is_empty() {
            self.current.anchor = match command {
                C::Backspace => self.previous(),
                C::BackspaceWord => self.word_left(),
                C::DeleteWord => self.word_right(),
                _ => self.next(),
            };
        }
        self.insert("");
    }

    fn previous(&self) -> usize {
        self.text()[..self.caret()]
            .char_indices()
            .last()
            .map_or(0, |(index, _)| index)
    }
    fn next(&self) -> usize {
        self.text()[self.caret()..]
            .chars()
            .next()
            .map_or(self.caret(), |character| {
                self.caret() + character.len_utf8()
            })
    }
    fn word_left(&self) -> usize {
        let mut target = self.caret();
        let mut seen_word = false;
        for (index, character) in self.text()[..self.caret()].char_indices().rev() {
            if seen_word && !character.is_alphanumeric() && character != '_' {
                break;
            }
            seen_word |= character.is_alphanumeric() || character == '_';
            target = index;
        }
        target
    }
    fn word_right(&self) -> usize {
        let mut target = self.caret();
        let mut seen_separator = false;
        for (index, character) in self.text()[self.caret()..].char_indices() {
            let word = character.is_alphanumeric() || character == '_';
            if seen_separator && word {
                break;
            }
            seen_separator |= !word;
            target = self.caret() + index + character.len_utf8();
        }
        target
    }
    fn move_to(&mut self, target: usize, extend: bool) {
        self.current.caret = target;
        if !extend {
            self.current.anchor = target;
        }
    }
    fn remember(&mut self) {
        if self.undo.len() == 64 {
            self.undo.remove(0);
        }
        self.undo.push(self.current.clone());
        self.redo.clear();
    }
    fn restore(current: &mut Snapshot, from: &mut Vec<Snapshot>, to: &mut Vec<Snapshot>) {
        if let Some(previous) = from.pop() {
            to.push(std::mem::replace(current, previous));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_replace_delete_and_undo_preserve_unicode_boundaries() {
        let mut text = QueryText::default();
        text.insert("Документы 🦀");
        text.command(TextCommand::SelectLeft);
        assert_eq!(text.selected_text(), "🦀");
        text.insert("test");
        assert_eq!(text.text(), "Документы test");
        text.command(TextCommand::SelectAll);
        text.insert("browser");
        text.command(TextCommand::Undo);
        assert_eq!(text.text(), "Документы test");
        text.command(TextCommand::Redo);
        assert_eq!(text.text(), "browser");
        text.command(TextCommand::BackspaceWord);
        assert_eq!(text.text(), "");
    }
    #[test]
    fn selection_and_word_motion_support_replacing_part_of_a_query() {
        let mut text = QueryText::default();
        text.insert("one two three");
        text.command(TextCommand::WordLeft);
        text.command(TextCommand::SelectEnd);
        assert_eq!(text.selected_text(), "three");
        text.insert("four");
        assert_eq!(text.text(), "one two four");
        text.command(TextCommand::Start);
        text.command(TextCommand::DeleteWord);
        assert_eq!(text.text(), "two four");
    }
    #[test]
    fn command_codes_round_trip_and_unknown_codes_are_ignored() {
        for command in TextCommand::ALL {
            assert_eq!(TextCommand::from_code(command as u8), Some(command));
        }
        assert_eq!(TextCommand::from_code(255), None);
    }
}
