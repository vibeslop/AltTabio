//! Visible, single-line encoding for foreign text written to a terminal.
//!
//! Apply this at output boundaries, not to stored names or window identities.

/// Escapes terminal controls, line separators, bidi formatting and literal backslashes.
/// Ordinary Unicode, including RTL letters, combining marks and emoji joiners, is preserved.
#[must_use]
pub fn escape(value: &str) -> String {
    escape_truncated(value, usize::MAX)
}

/// Encodes text before limiting its Unicode scalar count, keeping each escape intact.
/// The limit counts encoded characters, not terminal display cells or grapheme clusters.
#[must_use]
pub fn escape_truncated(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut remaining = max_chars;
    for character in value.chars() {
        if character.is_control()
            || matches!(
                character,
                '\\' | '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                    | '\u{2066}'..='\u{206f}'
            )
        {
            // C0/C1 controls include ESC, OSC/CSI/ST, BEL, CR, LF, TAB and DEL.
            // Bidi controls are escaped without suppressing ordinary RTL names.
            let escaped = character.escape_default();
            if escaped.len() > remaining {
                break;
            }
            remaining -= escaped.len();
            output.extend(escaped);
        } else {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{escape, escape_truncated};

    #[test]
    fn terminal_sequences_are_visible_text() {
        assert_eq!(
            escape("Doc\u{1b}[2J\u{1b}]52;c;YXR0YWNr\u{7}\u{1b}\\"),
            r"Doc\u{1b}[2J\u{1b}]52;c;YXR0YWNr\u{7}\u{1b}\\"
        );
        assert_eq!(
            escape("\u{9b}2J\u{9d}52;c;YXR0YWNr\u{9c}"),
            r"\u{9b}2J\u{9d}52;c;YXR0YWNr\u{9c}"
        );
    }

    #[test]
    fn foreign_text_cannot_add_rows_or_move_the_cursor() {
        assert_eq!(
            escape("Title\nforged row\r\t\u{8}\u{7f}\u{85}\u{2028}\u{2029}"),
            r"Title\nforged row\r\t\u{8}\u{7f}\u{85}\u{2028}\u{2029}"
        );
    }

    #[test]
    fn all_c0_and_c1_controls_are_encoded() {
        for character in ('\0'..='\u{1f}').chain('\u{7f}'..='\u{9f}') {
            let encoded = escape(&character.to_string());
            assert!(encoded.starts_with('\\'), "{character:?}");
            assert!(encoded.is_ascii(), "{character:?}");
            assert!(!encoded.chars().any(char::is_control), "{character:?}");
        }
    }

    #[test]
    fn bidi_marks_embeddings_overrides_and_isolates_are_encoded() {
        assert_eq!(
            escape(
                "\u{61c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}"
            ),
            r"\u{61c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}"
        );
        assert_eq!(escape("\u{206a}\u{206f}"), r"\u{206a}\u{206f}");
    }

    #[test]
    fn ordinary_unicode_names_are_preserved() {
        let name = "Café e\u{301} — Окно 中文 العربية עברית 👩\u{200d}💻 ☀\u{fe0f}";
        assert_eq!(escape(name), name);
        assert_eq!(escape_truncated("中文 العربية", 4), "中文 ا");
    }

    #[test]
    fn encoded_controls_are_distinguishable_from_literal_escapes() {
        assert_eq!(escape("\n"), r"\n");
        assert_eq!(escape(r"\n"), r"\\n");
    }

    #[test]
    fn truncation_counts_encoded_characters_and_keeps_escapes_whole() {
        let name = "12345678901234567890\u{1b}Z";
        assert_eq!(escape_truncated(name, 24), "12345678901234567890");
        assert_eq!(escape_truncated(name, 26), r"12345678901234567890\u{1b}");
        assert_eq!(escape_truncated("abc\nz", 4), "abc");
        assert_eq!(escape_truncated("abc\nz", 5), r"abc\n");
        assert_eq!(escape_truncated("\u{202e}abc", 7), "");
        assert_eq!(escape_truncated("\u{202e}abc", 8), r"\u{202e}");
        assert_eq!(escape_truncated("\\abc", 1), "");
        assert_eq!(escape_truncated("\\abc", 2), r"\\");
        assert_eq!(escape_truncated("中文", 0), "");
        assert_eq!(escape(""), "");
    }
}
