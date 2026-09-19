//! One description of the switcher's keys, shared by the overlay's hint bar and the Shortcuts
//! settings tab so the two never disagree.

use super::hotkey::HeldModifier;

use alttabio::input::WindowCommand;

/// A key chip with its explanation, as drawn in the footer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hint {
    pub label: &'static str,
    pub keys: &'static str,
}

/// What the footer should say right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FooterContext {
    /// The gesture modifier is down: numbers jump, release switches.
    Held(HeldModifier),
    /// The modifier is up but the list stays open: typing searches.
    Released,
    /// Search text is showing.
    Searching { matches: usize },
}

/// The footer, laid out like a launcher's action bar: a status sentence on the left and the
/// primary action plus the actions menu on the right.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Footer {
    pub status: String,
    pub trailing: Vec<Hint>,
}

#[must_use]
pub fn footer(context: FooterContext, panel_open: bool) -> Footer {
    let status = match context {
        FooterContext::Held(modifier) => {
            let glyph = modifier.glyph();
            format!("Release {glyph} to switch  ·  1–9 jumps")
        }
        FooterContext::Released => "Type to search".to_owned(),
        FooterContext::Searching { matches: 1 } => "1 match".to_owned(),
        FooterContext::Searching { matches } => format!("{matches} matches"),
    };
    let trailing = if panel_open {
        vec![
            Hint {
                label: "Run",
                keys: "↵",
            },
            Hint {
                label: "Actions",
                keys: "⌘K",
            },
        ]
    } else {
        vec![
            Hint {
                label: "Switch",
                keys: "↵",
            },
            Hint {
                label: "Actions",
                keys: "⌘K",
            },
        ]
    };
    Footer { status, trailing }
}

/// What an entry of the ⌘K action panel does to the selected window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    Activate,
    Command(WindowCommand),
    NextWindowOfApp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    pub label: &'static str,
    pub keys: &'static str,
    pub kind: ActionKind,
}

const fn action(label: &'static str, keys: &'static str, kind: ActionKind) -> Action {
    Action { label, keys, kind }
}

/// The ⌘K action panel, primary action first, each with the shortcut that runs it directly.
pub const ACTIONS: &[Action] = &[
    action("Switch to Window", "↵", ActionKind::Activate),
    action(
        "Close Window",
        "⌘W",
        ActionKind::Command(WindowCommand::Close),
    ),
    action(
        "Minimize Window",
        "⌘M",
        ActionKind::Command(WindowCommand::Minimize),
    ),
    action(
        "Zoom Window",
        "F6",
        ActionKind::Command(WindowCommand::Maximize),
    ),
    action(
        "Restore Window",
        "F7",
        ActionKind::Command(WindowCommand::Restore),
    ),
    action("Next Window of App", "⌘`", ActionKind::NextWindowOfApp),
    action("Hide App", "⌘H", ActionKind::Command(WindowCommand::Hide)),
    action("Quit App", "⌘Q", ActionKind::Command(WindowCommand::Quit)),
    action(
        "Force Quit App",
        "F8",
        ActionKind::Command(WindowCommand::Terminate),
    ),
    action(
        "New Instance",
        "F9",
        ActionKind::Command(WindowCommand::Run),
    ),
];

/// A row of the Shortcuts settings tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutRow {
    pub keys: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutSection {
    pub title: &'static str,
    pub rows: &'static [ShortcutRow],
}

const fn row(keys: &'static str, description: &'static str) -> ShortcutRow {
    ShortcutRow { keys, description }
}

pub const SECTIONS: &[ShortcutSection] = &[
    ShortcutSection {
        title: "Open",
        rows: &[
            row("⌘ Tab", "Open the switcher and step to the next window"),
            row("⌘ ⇧ Tab", "Step backwards"),
            row("⌥ Tab", "Also opens the switcher when enabled"),
        ],
    },
    ShortcutSection {
        title: "While holding ⌘",
        rows: &[
            row("Tab  /  ⇧ Tab", "Next or previous window"),
            row("1 – 9", "Switch to that row"),
            row("Release ⌘", "Switch to the selected window"),
            row("W", "Close the selected window"),
            row("M", "Minimize the selected window"),
            row("H", "Hide the selected window's app"),
            row("Q", "Quit the selected window's app"),
            row("`", "Next window of the same app"),
            row("K", "Open the actions panel"),
            row("esc", "Close the switcher without switching"),
        ],
    },
    ShortcutSection {
        title: "After releasing ⌘ (the list stays open)",
        rows: &[
            row("↑ ↓ ← →", "Move the selection"),
            row("⏎", "Switch to the selected window"),
            row("Type", "Search titles and app names"),
            row("⌫", "Delete a search character"),
            row("⌘ 1 – 9", "Switch to that row"),
            row("⌘ W  /  ⌘ M", "Close or minimize the selected window"),
            row("⌘ H  /  ⌘ Q", "Hide or quit the selected window's app"),
            row("⌘ K", "Open the actions panel"),
            row("Home  /  End", "Select the first or last window"),
            row("esc", "Close the switcher"),
        ],
    },
    ShortcutSection {
        title: "Function keys",
        rows: &[
            row("F4", "Close"),
            row("F5", "Minimize"),
            row("F6", "Zoom"),
            row("F7", "Restore from minimized or full screen"),
            row("F8", "Force quit the app"),
            row("F9", "Launch another instance of the app"),
        ],
    },
    ShortcutSection {
        title: "Mouse and trackpad",
        rows: &[
            row("Hover", "Select a row"),
            row("Click", "Switch to the row"),
            row("Right-click", "Open the window command menu"),
            row("Scroll", "Move the selection"),
            row("Click outside", "Close the switcher"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footer_names_the_modifier_that_is_down() {
        let footer = footer(FooterContext::Held(HeldModifier::Option), false);

        assert_eq!(footer.status, "Release ⌥ to switch  ·  1–9 jumps");
        assert_eq!(
            footer
                .trailing
                .iter()
                .map(|hint| hint.keys)
                .collect::<Vec<_>>(),
            vec!["↵", "⌘K"]
        );
    }

    #[test]
    fn search_status_counts_matches_and_the_open_panel_runs_actions() {
        assert_eq!(
            footer(FooterContext::Searching { matches: 1 }, false).status,
            "1 match"
        );
        assert_eq!(
            footer(FooterContext::Searching { matches: 4 }, true).trailing[0].label,
            "Run"
        );
    }

    #[test]
    fn the_primary_action_leads_the_panel() {
        assert_eq!(ACTIONS[0].kind, ActionKind::Activate);
        assert!(ACTIONS.iter().all(|action| !action.keys.is_empty()));
    }
}
