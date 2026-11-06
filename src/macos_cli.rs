//! Compile-time gate for macOS window diagnostics, independent of native TCC attribution.

use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
pub enum DiagnosticCommand {
    #[cfg(not(debug_assertions))]
    Unavailable,
    #[cfg(debug_assertions)]
    List,
    #[cfg(debug_assertions)]
    Activate(Option<OsString>),
}

/// Recognizes diagnostic flags in argv, including when mixed with normal startup flags.
/// Production builds reject the entire request instead of falling through to app startup.
#[must_use]
pub fn diagnostic_command(arguments: &[OsString]) -> Option<DiagnosticCommand> {
    if arguments.iter().any(|argument| argument == "--list") {
        #[cfg(debug_assertions)]
        return Some(DiagnosticCommand::List);
        #[cfg(not(debug_assertions))]
        return Some(DiagnosticCommand::Unavailable);
    }
    let index = arguments
        .iter()
        .position(|argument| argument == "--activate")?;
    #[cfg(debug_assertions)]
    return Some(DiagnosticCommand::Activate(
        arguments.get(index + 1).cloned(),
    ));
    #[cfg(not(debug_assertions))]
    {
        let _ = index;
        Some(DiagnosticCommand::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(arguments: &[&str]) -> Option<DiagnosticCommand> {
        diagnostic_command(&arguments.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn normal_startup_does_not_request_diagnostics() {
        for arguments in [
            vec!["AltTabio"],
            vec!["AltTabio", "--preview"],
            vec!["AltTabio", "--settings"],
            // An executable name is not normally a flag, but is included in argv.
            vec!["/Applications/AltTabio.app/Contents/MacOS/AltTabio"],
        ] {
            assert_eq!(command(&arguments), None);
        }
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn production_rejects_diagnostics_including_mixed_and_malformed_requests() {
        for arguments in [
            vec!["AltTabio", "--list"],
            vec!["AltTabio", "--activate", "42"],
            vec!["AltTabio", "--activate"],
            vec!["AltTabio", "--activate", "invalid"],
            vec!["AltTabio", "--settings", "--list"],
            vec!["AltTabio", "--preview", "--activate", "42"],
            vec!["AltTabio", "--activate", "42", "--list"],
        ] {
            assert_eq!(command(&arguments), Some(DiagnosticCommand::Unavailable));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_builds_keep_window_diagnostics() {
        assert_eq!(
            command(&["AltTabio", "--list"]),
            Some(DiagnosticCommand::List)
        );
        assert_eq!(
            command(&["AltTabio", "--activate", "42"]),
            Some(DiagnosticCommand::Activate(Some(OsString::from("42"))))
        );
        assert_eq!(
            command(&["AltTabio", "--activate"]),
            Some(DiagnosticCommand::Activate(None))
        );
        assert_eq!(
            command(&["AltTabio", "--activate", "42", "--list"]),
            Some(DiagnosticCommand::List)
        );
    }
}
