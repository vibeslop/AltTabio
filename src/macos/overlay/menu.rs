//! The right-click command menu.

use alttabio::input::WindowCommand;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem, NSView};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSString};
use std::cell::Cell;

pub struct ContextMenuIvars {
    chosen: Cell<Option<WindowCommand>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - ContextMenuTarget does not implement Drop.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioContextMenuTarget"]
    #[ivars = ContextMenuIvars]
    pub struct ContextMenuTarget;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for ContextMenuTarget {}

    impl ContextMenuTarget {
        // SAFETY: the action signature matches the target/action convention.
        #[unsafe(method(chooseCommand:))]
        fn choose_command(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender.and_then(|sender| sender.downcast_ref::<NSMenuItem>()) else {
                return;
            };
            self.ivars().chosen.set(command_for_tag(item.tag()));
        }
    }
);

impl ContextMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ContextMenuIvars {
            chosen: Cell::new(None),
        });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }
}

struct MenuItem {
    command: WindowCommand,
    /// Window items read as they are; app items name the app after this.
    title: &'static str,
    key: &'static str,
    on_window: bool,
    /// A separator comes before this item unless it is the first.
    separated: bool,
}

/// The command menu from top to bottom. An item's tag is its index here plus one, so the 0 of an
/// untagged item names no command.
const MENU: [MenuItem; 5] = [
    MenuItem {
        command: WindowCommand::Close,
        title: "Close Window",
        key: "w",
        on_window: true,
        separated: false,
    },
    MenuItem {
        command: WindowCommand::Minimize,
        title: "Minimize Window",
        key: "m",
        on_window: true,
        separated: false,
    },
    MenuItem {
        command: WindowCommand::Hide,
        title: "Hide",
        key: "h",
        on_window: false,
        separated: true,
    },
    MenuItem {
        command: WindowCommand::Quit,
        title: "Quit",
        key: "q",
        on_window: false,
        separated: false,
    },
    MenuItem {
        command: WindowCommand::Terminate,
        title: "Force Quit",
        key: "",
        on_window: false,
        separated: true,
    },
];

/// The tag, title, and key of each item the menu shows, `None` standing for a separator. Window
/// commands appear only when a `window` is selected; the app commands name the app.
fn menu_entries(window: bool, app_name: &str) -> Vec<Option<(isize, String, &'static str)>> {
    let mut entries = Vec::new();
    for (index, item) in MENU.iter().enumerate() {
        if item.on_window && !window {
            continue;
        }
        if item.separated && !entries.is_empty() {
            entries.push(None);
        }
        let title = if item.on_window {
            item.title.to_owned()
        } else {
            format!("{} {app_name}", item.title)
        };
        let tag = isize::try_from(index + 1).unwrap_or_default();
        entries.push(Some((tag, title, item.key)));
    }
    entries
}

fn command_for_tag(tag: isize) -> Option<WindowCommand> {
    let index = usize::try_from(tag).ok()?.checked_sub(1)?;
    MENU.get(index).map(|item| item.command)
}

/// Runs the command menu at `point` in `view` and returns the command chosen, if any.
pub(super) fn show(
    mtm: MainThreadMarker,
    view: &NSView,
    point: NSPoint,
    window: bool,
    app_name: &str,
) -> Option<WindowCommand> {
    let target = ContextMenuTarget::new(mtm);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for entry in menu_entries(window, app_name) {
        let Some((tag, title, key)) = entry else {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        };
        let item = unsafe {
            // SAFETY: the selector exists on ContextMenuTarget with a matching signature.
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&title),
                Some(sel!(chooseCommand:)),
                &NSString::from_str(key),
            )
        };
        item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        item.setTag(tag);
        unsafe {
            // SAFETY: the target outlives the menu; both are dropped after the pop-up.
            item.setTarget(Some(&target));
        }
        menu.addItem(&item);
    }
    let _shown = menu.popUpMenuPositioningItem_atLocation_inView(None, point, Some(view));
    target.ivars().chosen.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_tags_name_the_commands() {
        assert_eq!(command_for_tag(1), Some(WindowCommand::Close));
        assert_eq!(command_for_tag(5), Some(WindowCommand::Terminate));
        assert_eq!(command_for_tag(0), None);
        assert_eq!(command_for_tag(6), None);
        assert_eq!(command_for_tag(-1), None);
        let chosen = menu_entries(true, "Notes")
            .into_iter()
            .flatten()
            .map(|(tag, _, _)| command_for_tag(tag))
            .collect::<Vec<_>>();
        let listed = MENU
            .iter()
            .map(|item| Some(item.command))
            .collect::<Vec<_>>();
        assert_eq!(chosen, listed);
    }

    #[test]
    fn the_menu_offers_window_commands_only_for_a_window() {
        let titles = |window| {
            menu_entries(window, "Notes")
                .into_iter()
                .map(|entry| entry.map(|(_, title, _)| title))
                .collect::<Vec<_>>()
        };
        let expected = |titles: &[Option<&str>]| {
            titles
                .iter()
                .map(|title| title.map(str::to_owned))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            titles(true),
            expected(&[
                Some("Close Window"),
                Some("Minimize Window"),
                None,
                Some("Hide Notes"),
                Some("Quit Notes"),
                None,
                Some("Force Quit Notes"),
            ])
        );
        assert_eq!(
            titles(false),
            expected(&[
                Some("Hide Notes"),
                Some("Quit Notes"),
                None,
                Some("Force Quit Notes"),
            ])
        );
    }

    #[test]
    fn the_menu_shows_the_keys_the_panel_answers() {
        let keys = menu_entries(true, "Notes")
            .into_iter()
            .flatten()
            .map(|(_, _, key)| key)
            .collect::<Vec<_>>();
        assert_eq!(keys, ["w", "m", "h", "q", ""]);
    }
}
