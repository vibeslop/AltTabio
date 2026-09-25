//! App-first switching, as the macOS switcher presents it: running apps in a strip, most
//! recently used first, and under the selected app its windows, most recently used first.
//!
//! Tab and the side arrows step between apps and land on the app's last-used window; the up and
//! down arrows step between that app's windows. The order is fixed when the switcher opens, so a
//! window closing or an app hiding while it shows never moves the other entries.
//!
//! The app in front never starts on the window that already has focus, since switching to it
//! would do nothing; stepping back onto it lands on its next window instead.

use crate::input::WindowCommand;
use crate::switcher::ProcessIdentity;

/// One window as the adapter lists it, most recently used first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowEntry {
    pub handle: isize,
    pub process: ProcessIdentity,
    pub app_name: String,
}

/// Windows in the order they last had focus, the current one first.
///
/// Stacking order is not enough for this: clicking an app in the Dock raises all of its windows
/// while only one of them was used.
#[derive(Debug, Default)]
pub struct WindowHistory {
    windows: Vec<isize>,
}

impl WindowHistory {
    /// Records `focused` as the window in use and forgets windows no longer `listed`. An empty
    /// history starts from `listed` as given, front to back, the best guess at the order of use
    /// from before recording began.
    pub fn note(&mut self, focused: Option<isize>, listed: &[isize]) {
        if self.windows.is_empty() {
            listed.clone_into(&mut self.windows);
        }
        self.windows.retain(|window| listed.contains(window));
        if let Some(focused) = focused {
            self.windows.retain(|window| *window != focused);
            self.windows.insert(0, focused);
        }
    }

    /// How recently `window` had focus, 0 being now; windows never seen rank last.
    #[must_use]
    pub fn rank(&self, window: isize) -> usize {
        self.windows
            .iter()
            .position(|known| *known == window)
            .unwrap_or(usize::MAX)
    }
}

/// A running app with the windows it has open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntry {
    pub process: ProcessIdentity,
    pub name: String,
    /// Window handles, most recently used first; empty for an app with no open window.
    pub windows: Vec<isize>,
}

/// Groups `windows` by app and adds the `windowless` apps. Apps follow `recent_apps`, the process
/// ids in the order they were last activated; apps it does not name were last used before it
/// started recording, so they follow in the order of their first window, and windowless ones
/// after those.
#[must_use]
pub fn group_by_app(
    windows: &[WindowEntry],
    recent_apps: &[u32],
    windowless: &[(ProcessIdentity, String)],
) -> Vec<AppEntry> {
    let mut apps: Vec<AppEntry> = Vec::new();
    for window in windows {
        match apps.iter_mut().find(|app| app.process == window.process) {
            Some(app) => app.windows.push(window.handle),
            None => apps.push(AppEntry {
                process: window.process,
                name: window.app_name.clone(),
                windows: vec![window.handle],
            }),
        }
    }
    for (process, name) in windowless {
        if !apps.iter().any(|app| app.process == *process) {
            apps.push(AppEntry {
                process: *process,
                name: name.clone(),
                windows: Vec::new(),
            });
        }
    }
    // A stable sort keeps that order among the apps `recent_apps` does not name.
    apps.sort_by_key(|app| {
        recent_apps
            .iter()
            .position(|id| *id == app.process.id)
            .unwrap_or(usize::MAX)
    });
    apps
}

/// What switching to the selection or running a command on it acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Window {
        handle: isize,
        process: ProcessIdentity,
    },
    /// An app with no open window, or an app-wide command such as Hide or Quit.
    App(ProcessIdentity),
}

impl Target {
    #[must_use]
    pub const fn process(self) -> ProcessIdentity {
        match self {
            Self::Window { process, .. } | Self::App(process) => process,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Tab, the side arrows, and the backtick: the next (positive) or previous app, wrapping.
    StepApp(i32),
    /// The up and down arrows: the selected app's next or previous window, stopping at the ends.
    StepWindow(i32),
    /// Return, a click, or letting go of the switch modifier.
    Activate,
    /// A number key: the selected app's window at that position, counted from 1, at once.
    ChooseWindow(usize),
    Dismiss,
    Command(WindowCommand),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    None,
    /// The gesture started; the adapter lists the apps and calls `open` with this step.
    Open {
        step: i32,
    },
    Hide,
    Redraw,
    Activate(Target),
    Execute {
        command: WindowCommand,
        target: Target,
    },
}

/// Commands that concern the whole app rather than one of its windows.
const fn acts_on_app(command: WindowCommand) -> bool {
    matches!(
        command,
        WindowCommand::Hide | WindowCommand::Quit | WindowCommand::Terminate | WindowCommand::Run
    )
}

#[derive(Debug, Default)]
pub struct AppSwitcher {
    apps: Vec<AppEntry>,
    app: usize,
    window: usize,
    active: bool,
    /// The process id of the app in front when the session opened.
    frontmost: Option<u32>,
    // A command menu acts on the entry it opened for, even if the list changes underneath.
    menu_target: Option<Target>,
}

impl AppSwitcher {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    /// Starts a session on `apps` and moves `step` apps from the frontmost one. When the
    /// frontmost app is not first in the list (it has nothing to list, or it is the switcher's
    /// own settings), the first app is already one step away from it.
    pub fn open(&mut self, apps: Vec<AppEntry>, step: Option<i32>, frontmost: Option<u32>) {
        let frontmost_listed =
            frontmost.is_none_or(|id| apps.first().is_some_and(|app| app.process.id == id));
        self.apps = apps;
        self.frontmost = frontmost;
        self.app = 0;
        self.window = 0;
        self.menu_target = None;
        self.active = !self.apps.is_empty();
        match step {
            Some(step) if step > 0 && !frontmost_listed => self.step_app(step - 1),
            Some(step) => self.step_app(step),
            None => self.window = self.landing(0),
        }
    }

    /// Takes a fresh listing while the session shows. Apps and windows keep the places they had
    /// when it opened; gone ones drop out, new ones join the end.
    pub fn refresh(&mut self, apps: Vec<AppEntry>) {
        let selected_process = self.selected_app().map(|app| app.process);
        let selected_window = self.selected_window();
        let mut fresh = apps;
        let mut merged = Vec::with_capacity(fresh.len());
        for previous in &self.apps {
            let Some(index) = fresh.iter().position(|app| app.process == previous.process) else {
                continue;
            };
            let mut app = fresh.remove(index);
            app.windows.sort_by_key(|handle| {
                previous
                    .windows
                    .iter()
                    .position(|known| known == handle)
                    .unwrap_or(usize::MAX)
            });
            merged.push(app);
        }
        merged.append(&mut fresh);
        self.apps = merged;
        if self.apps.is_empty() {
            self.active = false;
            return;
        }
        if let Some(index) = selected_process
            .and_then(|process| self.apps.iter().position(|app| app.process == process))
        {
            self.app = index;
            let windows = self.apps.get(index).map_or(&[][..], |app| &app.windows[..]);
            self.window = selected_window
                .and_then(|handle| windows.iter().position(|known| *known == handle))
                .unwrap_or(self.window)
                .min(windows.len().saturating_sub(1));
        } else {
            self.app = self.app.min(self.apps.len() - 1);
            self.window = 0;
        }
    }

    pub fn hide(&mut self) {
        self.active = false;
        self.menu_target = None;
    }

    #[must_use]
    pub fn selected_app(&self) -> Option<&AppEntry> {
        self.apps.get(self.app)
    }

    #[must_use]
    pub fn selected_app_index(&self) -> Option<usize> {
        (self.app < self.apps.len()).then_some(self.app)
    }

    /// The selected window's position in its app's list; None when the app has no window.
    #[must_use]
    pub fn selected_window_index(&self) -> Option<usize> {
        let app = self.selected_app()?;
        (self.window < app.windows.len()).then_some(self.window)
    }

    #[must_use]
    pub fn selected_window(&self) -> Option<isize> {
        self.selected_app()?.windows.get(self.window).copied()
    }

    /// The selected window, or the selected app when it has none.
    #[must_use]
    pub fn selected_target(&self) -> Option<Target> {
        let app = self.selected_app()?;
        Some(
            app.windows
                .get(self.window)
                .map_or(Target::App(app.process), |handle| Target::Window {
                    handle: *handle,
                    process: app.process,
                }),
        )
    }

    /// Selects `app`, and `window` among its windows or its last-used one; true when the
    /// selection moved.
    pub fn select(&mut self, app: usize, window: Option<usize>) -> bool {
        let Some(entry) = self.apps.get(app) else {
            return false;
        };
        let window = window.unwrap_or_else(|| self.landing(app));
        if window > 0 && window >= entry.windows.len() {
            return false;
        }
        let changed = self.app != app || self.window != window;
        self.app = app;
        self.window = window;
        changed
    }

    pub fn handle(&mut self, action: Action) -> Effect {
        if !self.active {
            return match action {
                Action::StepApp(step) => Effect::Open { step },
                _ => Effect::None,
            };
        }
        if self.context_menu_open() {
            return Effect::None;
        }
        match action {
            Action::StepApp(step) => {
                self.step_app(step);
                Effect::Redraw
            }
            Action::StepWindow(step) => {
                if self.step_window(step) {
                    Effect::Redraw
                } else {
                    Effect::None
                }
            }
            Action::Activate => match self.selected_target() {
                Some(target) => {
                    self.active = false;
                    Effect::Activate(target)
                }
                None => Effect::None,
            },
            Action::ChooseWindow(position) => {
                let listed = self.selected_app().map_or(0, |app| app.windows.len());
                match position.checked_sub(1).filter(|index| *index < listed) {
                    Some(index) => {
                        self.window = index;
                        self.handle(Action::Activate)
                    }
                    None => Effect::None,
                }
            }
            Action::Dismiss => {
                self.hide();
                Effect::Hide
            }
            Action::Command(command) => self
                .selected_target()
                .and_then(|target| command_target(command, target))
                .map_or(Effect::None, |target| Effect::Execute { command, target }),
        }
    }

    /// Freezes the selection as the target of a command menu; false when none can open.
    pub fn open_context_menu(&mut self) -> bool {
        if !self.active || self.context_menu_open() {
            return false;
        }
        self.menu_target = self.selected_target();
        self.menu_target.is_some()
    }

    pub fn finish_context_menu(&mut self, command: Option<WindowCommand>) -> Effect {
        match (self.menu_target.take(), command) {
            (Some(target), Some(command)) => command_target(command, target)
                .map_or(Effect::None, |target| Effect::Execute { command, target }),
            _ => Effect::None,
        }
    }

    #[must_use]
    pub const fn context_menu_open(&self) -> bool {
        self.menu_target.is_some()
    }

    fn step_app(&mut self, step: i32) {
        let count = isize::try_from(self.apps.len()).unwrap_or(isize::MAX);
        if count == 0 {
            return;
        }
        let current = isize::try_from(self.app).unwrap_or_default();
        let step = isize::try_from(step).unwrap_or_default();
        self.app = usize::try_from((current + step).rem_euclid(count)).unwrap_or_default();
        self.window = self.landing(self.app);
    }

    /// The window an app starts on: its last-used one, or for the app in front, whose last-used
    /// window already has focus, the one before that.
    fn landing(&self, app: usize) -> usize {
        let in_front = self.apps.get(app).is_some_and(|entry| {
            Some(entry.process.id) == self.frontmost && entry.windows.len() > 1
        });
        usize::from(in_front)
    }

    fn step_window(&mut self, step: i32) -> bool {
        let Some(last) = self
            .selected_app()
            .and_then(|app| app.windows.len().checked_sub(1))
        else {
            return false;
        };
        let current = isize::try_from(self.window).unwrap_or_default();
        let step = isize::try_from(step).unwrap_or_default();
        let last = isize::try_from(last).unwrap_or(isize::MAX);
        let next = usize::try_from(current.saturating_add(step).clamp(0, last)).unwrap_or_default();
        let changed = next != self.window;
        self.window = next;
        changed
    }
}

/// Where `command` lands for a selection of `target`: app commands on the app, window commands
/// on the window. A window command on an app without windows has nothing to act on.
const fn command_target(command: WindowCommand, target: Target) -> Option<Target> {
    if acts_on_app(command) {
        return Some(Target::App(target.process()));
    }
    match target {
        Target::Window { .. } => Some(target),
        Target::App(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(id: u32) -> ProcessIdentity {
        ProcessIdentity::new(id, u64::from(id) * 10)
    }

    fn app(id: u32, windows: &[isize]) -> AppEntry {
        AppEntry {
            process: process(id),
            name: format!("App {id}"),
            windows: windows.to_vec(),
        }
    }

    fn window(handle: isize, id: u32) -> WindowEntry {
        WindowEntry {
            handle,
            process: process(id),
            app_name: format!("App {id}"),
        }
    }

    fn opened(apps: Vec<AppEntry>, step: Option<i32>) -> AppSwitcher {
        let mut switcher = AppSwitcher::default();
        switcher.open(apps, step, Some(1));
        switcher
    }

    #[test]
    fn windows_group_under_their_app_in_activation_order() {
        let windows = [window(1, 7), window(2, 8), window(3, 7), window(4, 9)];

        let apps = group_by_app(&windows, &[9, 7], &[(process(5), "Music".to_owned())]);

        let order = apps
            .iter()
            .map(|app| (app.process.id, app.windows.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![(9, vec![4]), (7, vec![1, 3]), (8, vec![2]), (5, Vec::new())]
        );
    }

    #[test]
    fn a_recently_used_app_without_windows_keeps_its_place() {
        let windows = [window(1, 7), window(2, 8)];

        let apps = group_by_app(&windows, &[5, 8], &[(process(5), "Finder".to_owned())]);

        let order = apps.iter().map(|app| app.process.id).collect::<Vec<_>>();
        assert_eq!(order, vec![5, 8, 7]);
    }

    #[test]
    fn a_windowless_app_that_has_windows_is_listed_once() {
        let apps = group_by_app(&[window(1, 7)], &[], &[(process(7), "App 7".to_owned())]);

        assert_eq!(apps, vec![app(7, &[1])]);
    }

    #[test]
    fn the_gesture_opens_on_the_previous_app_and_its_last_window() {
        let mut switcher = AppSwitcher::default();
        assert_eq!(
            switcher.handle(Action::StepApp(1)),
            Effect::Open { step: 1 }
        );
        switcher.open(vec![app(1, &[10, 11]), app(2, &[20, 21])], Some(1), Some(1));

        assert_eq!(switcher.selected_app_index(), Some(1));
        assert_eq!(switcher.selected_window(), Some(20));
        assert_eq!(
            switcher.handle(Action::Activate),
            Effect::Activate(Target::Window {
                handle: 20,
                process: process(2),
            })
        );
        assert!(!switcher.is_active());
    }

    #[test]
    fn an_unlisted_front_app_puts_the_first_app_one_step_away() {
        let apps = || vec![app(1, &[10]), app(2, &[20]), app(3, &[30])];
        let mut switcher = AppSwitcher::default();

        switcher.open(apps(), Some(1), Some(9));
        assert_eq!(switcher.selected_app_index(), Some(0));
        switcher.open(apps(), Some(-1), Some(9));
        assert_eq!(switcher.selected_app_index(), Some(2));
        switcher.open(apps(), Some(1), None);
        assert_eq!(switcher.selected_app_index(), Some(1));
    }

    #[test]
    fn apps_wrap_and_windows_stop_at_the_ends() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20, 21])], Some(-1));
        assert_eq!(switcher.selected_app_index(), Some(1));

        assert_eq!(switcher.handle(Action::StepApp(1)), Effect::Redraw);
        assert_eq!(switcher.selected_app_index(), Some(0));
        let _ = switcher.handle(Action::StepApp(1));
        assert_eq!(switcher.handle(Action::StepWindow(1)), Effect::Redraw);
        assert_eq!(switcher.selected_window(), Some(21));
        assert_eq!(switcher.handle(Action::StepWindow(1)), Effect::None);
        assert_eq!(switcher.selected_window(), Some(21));
        assert_eq!(switcher.handle(Action::StepWindow(-5)), Effect::Redraw);
        assert_eq!(switcher.selected_window(), Some(20));
    }

    #[test]
    fn a_number_switches_to_that_window_of_the_selected_app() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20, 21, 22])], Some(1));

        assert_eq!(switcher.handle(Action::ChooseWindow(4)), Effect::None);
        assert_eq!(switcher.handle(Action::ChooseWindow(0)), Effect::None);
        assert!(switcher.is_active());
        assert_eq!(
            switcher.handle(Action::ChooseWindow(3)),
            Effect::Activate(Target::Window {
                handle: 22,
                process: process(2),
            })
        );
        assert!(!switcher.is_active());
    }

    #[test]
    fn the_front_app_starts_on_the_window_after_the_one_in_focus() {
        let mut switcher = opened(vec![app(1, &[10, 11]), app(2, &[20, 21])], None);
        assert_eq!(switcher.selected_window(), Some(11));

        let _ = switcher.handle(Action::StepApp(1));
        assert_eq!(switcher.selected_window(), Some(20));
        let _ = switcher.handle(Action::StepApp(-1));
        assert_eq!(switcher.selected_window(), Some(11));
        let _ = switcher.select(1, None);
        assert!(switcher.select(0, None));
        assert_eq!(switcher.selected_window(), Some(11));
        // An app in front with one window starts on it; there is nothing else.
        let single = opened(vec![app(1, &[10]), app(2, &[20])], None);
        assert_eq!(single.selected_window(), Some(10));
    }

    #[test]
    fn the_window_history_follows_focus_and_forgets_closed_windows() {
        let mut history = WindowHistory::default();
        let ranks = |history: &WindowHistory, windows: [isize; 3]| {
            windows.map(|window| history.rank(window))
        };

        history.note(None, &[3, 1, 2]);
        assert_eq!(ranks(&history, [3, 1, 2]), [0, 1, 2]);
        history.note(Some(2), &[3, 1, 2]);
        assert_eq!(ranks(&history, [2, 3, 1]), [0, 1, 2]);
        // Window 3 closed and window 4 was never seen.
        history.note(Some(2), &[2, 1, 4]);
        assert_eq!(ranks(&history, [2, 1, 3]), [0, 1, usize::MAX]);
        assert_eq!(history.rank(4), usize::MAX);
    }

    #[test]
    fn stepping_to_another_app_lands_on_its_last_window() {
        let mut switcher = opened(vec![app(1, &[10, 11]), app(2, &[20, 21])], None);
        let _ = switcher.handle(Action::StepWindow(1));

        let _ = switcher.handle(Action::StepApp(1));

        assert_eq!(switcher.selected_window(), Some(20));
    }

    #[test]
    fn an_app_without_windows_is_activated_itself_and_ignores_window_commands() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[])], Some(1));

        assert_eq!(switcher.selected_window_index(), None);
        assert_eq!(switcher.handle(Action::StepWindow(1)), Effect::None);
        assert_eq!(
            switcher.handle(Action::Command(WindowCommand::Close)),
            Effect::None
        );
        assert_eq!(
            switcher.handle(Action::Command(WindowCommand::Quit)),
            Effect::Execute {
                command: WindowCommand::Quit,
                target: Target::App(process(2)),
            }
        );
        assert_eq!(
            switcher.handle(Action::Activate),
            Effect::Activate(Target::App(process(2)))
        );
    }

    #[test]
    fn window_commands_target_the_window_and_app_commands_the_app() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20])], Some(1));

        assert_eq!(
            switcher.handle(Action::Command(WindowCommand::Minimize)),
            Effect::Execute {
                command: WindowCommand::Minimize,
                target: Target::Window {
                    handle: 20,
                    process: process(2),
                },
            }
        );
        assert_eq!(
            switcher.handle(Action::Command(WindowCommand::Hide)),
            Effect::Execute {
                command: WindowCommand::Hide,
                target: Target::App(process(2)),
            }
        );
    }

    #[test]
    fn a_refresh_keeps_the_opening_order_and_the_selected_window() {
        let mut switcher = opened(
            vec![app(1, &[10, 11]), app(2, &[20, 21, 22]), app(3, &[30])],
            Some(1),
        );
        let _ = switcher.handle(Action::StepWindow(1));

        // Hiding app 1 activated app 3 and reordered everything; window 20 closed.
        switcher.refresh(vec![app(3, &[30]), app(2, &[22, 21, 23]), app(4, &[40])]);

        let order = switcher
            .apps()
            .iter()
            .map(|app| (app.process.id, app.windows.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![(2, vec![21, 22, 23]), (3, vec![30]), (4, vec![40])]
        );
        assert_eq!(switcher.selected_window(), Some(21));
    }

    #[test]
    fn a_refresh_that_drops_the_selected_app_keeps_the_place() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20]), app(3, &[30])], Some(2));

        switcher.refresh(vec![app(1, &[10]), app(2, &[20])]);

        assert_eq!(switcher.selected_app_index(), Some(1));
        switcher.refresh(Vec::new());
        assert!(!switcher.is_active());
    }

    #[test]
    fn a_closed_window_moves_the_selection_to_its_neighbour() {
        let mut switcher = opened(vec![app(1, &[10, 11, 12])], None);
        let _ = switcher.handle(Action::StepWindow(2));

        switcher.refresh(vec![app(1, &[10, 11])]);

        assert_eq!(switcher.selected_window(), Some(11));
    }

    #[test]
    fn the_command_menu_keeps_its_target_while_input_arrives() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20])], Some(1));
        assert!(switcher.open_context_menu());
        assert!(!switcher.open_context_menu());

        for action in [
            Action::StepApp(1),
            Action::Activate,
            Action::Command(WindowCommand::Close),
        ] {
            assert_eq!(switcher.handle(action), Effect::None);
        }
        switcher.refresh(vec![app(1, &[10])]);
        assert_eq!(
            switcher.finish_context_menu(Some(WindowCommand::Close)),
            Effect::Execute {
                command: WindowCommand::Close,
                target: Target::Window {
                    handle: 20,
                    process: process(2),
                },
            }
        );
        assert!(switcher.open_context_menu());
        assert_eq!(switcher.finish_context_menu(None), Effect::None);
    }

    #[test]
    fn the_command_menu_sends_app_commands_to_the_app() {
        let mut switcher = opened(vec![app(1, &[10])], None);
        assert!(switcher.open_context_menu());

        assert_eq!(
            switcher.finish_context_menu(Some(WindowCommand::Terminate)),
            Effect::Execute {
                command: WindowCommand::Terminate,
                target: Target::App(process(1)),
            }
        );
    }

    #[test]
    fn mouse_selection_picks_an_app_and_optionally_a_window() {
        let mut switcher = opened(vec![app(1, &[10]), app(2, &[20, 21])], None);

        assert!(switcher.select(1, Some(1)));
        assert_eq!(switcher.selected_window(), Some(21));
        assert!(!switcher.select(1, Some(1)));
        assert!(switcher.select(1, None));
        assert_eq!(switcher.selected_window(), Some(20));
        assert!(!switcher.select(1, Some(5)));
        assert!(!switcher.select(4, None));
    }

    #[test]
    fn escape_hides_and_nothing_but_the_gesture_opens() {
        let mut switcher = opened(vec![app(1, &[10])], None);

        assert_eq!(switcher.handle(Action::Dismiss), Effect::Hide);
        assert!(!switcher.is_active());
        assert_eq!(switcher.handle(Action::Activate), Effect::None);
        assert_eq!(switcher.handle(Action::StepWindow(1)), Effect::None);
        assert!(!opened(Vec::new(), Some(1)).is_active());
    }
}
