//! Bounds for native metadata and safe rendering of foreign text in diagnostics.

use std::fmt::{self, Write};

pub const MAX_WINDOWS: usize = 512;
pub const MAX_WINDOWS_PER_APP: usize = 128;
pub const MAX_METADATA_CHARS: usize = 1024;
pub const MAX_FILTER_CHARS: usize = 256;

#[must_use]
pub fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_METADATA_CHARS).collect()
}

/// Keeps native titles from introducing terminal commands or forged log lines.
pub struct TerminalText<'a>(pub &'a str);

impl fmt::Display for TerminalText<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.0.chars() {
            if character.is_control() {
                for escaped in character.escape_default() {
                    output.write_char(escaped)?;
                }
            } else {
                output.write_char(character)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_text_cannot_execute_terminal_sequences_or_forge_lines() {
        let text = "Title\n\r\u{1b}[2J\u{1b}]52;c;payload\u{7}\u{9b}你好";
        let rendered = TerminalText(text).to_string();
        assert!(!rendered.chars().any(char::is_control));
        assert!(rendered.contains("\\n\\r\\u{1b}[2J"));
        assert!(rendered.ends_with("你好"));
    }

    #[test]
    fn metadata_is_bounded_without_splitting_utf8() {
        let text = "窗".repeat(MAX_METADATA_CHARS + 1);
        assert_eq!(bounded_text(&text).chars().count(), MAX_METADATA_CHARS);
    }
}
