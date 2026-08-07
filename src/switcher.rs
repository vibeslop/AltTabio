//! Task filtering and selection behavior shared by every presentation adapter.

use crate::input::{InputAction, WindowCommand};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub id: u32,
    pub started_at: u64,
}

impl ProcessIdentity {
    #[must_use]
    pub const fn new(id: u32, started_at: u64) -> Self {
        Self { id, started_at }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwitchTask {
    pub number: usize,
    pub window_handle: isize,
    pub process_identity: ProcessIdentity,
    pub icon_handle: isize,
    pub title: String,
    pub process_name: String,
}

impl SwitchTask {
    #[must_use]
    pub fn new(number: usize, window_handle: isize, title: &str, process_name: &str) -> Self {
        Self {
            number,
            window_handle,
            process_identity: ProcessIdentity::default(),
            icon_handle: 0,
            title: title.to_owned(),
            process_name: process_name.to_owned(),
        }
    }

    #[must_use]
    pub const fn with_icon_handle(mut self, icon_handle: isize) -> Self {
        self.icon_handle = icon_handle;
        self
    }

    #[must_use]
    pub const fn with_process_identity(mut self, process_identity: ProcessIdentity) -> Self {
        self.process_identity = process_identity;
        self
    }
}

#[derive(Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields are an immutable snapshot of independent Win32 eligibility flags"
)]
pub struct WindowEligibility<'a> {
    pub title: &'a str,
    pub class_name: &'a str,
    pub is_visible: bool,
    pub is_current_process: bool,
    pub is_cloaked: bool,
    pub is_tool_window: bool,
    pub has_owner: bool,
    pub is_app_window: bool,
    pub matches_monitor_filter: bool,
}

#[must_use]
pub fn is_switchable_window(window: &WindowEligibility<'_>) -> bool {
    if !window.is_visible
        || window.is_current_process
        || window.is_cloaked
        || window.is_tool_window
        || !window.matches_monitor_filter
        || window.title.trim().is_empty()
        || matches!(
            window.class_name,
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Windows.UI.Core.CoreWindow"
        )
    {
        return false;
    }

    !window.has_owner || window.is_app_window
}

#[derive(Debug, Default)]
pub struct Switcher {
    all_tasks: Vec<SwitchTask>,
    visible_indices: Vec<usize>,
    selected_visible_index: Option<usize>,
    // A mouse hit pins the rendered range so selecting that hit cannot move it under the cursor.
    pinned_visible_start: Option<usize>,
    filter: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SwitcherSessionSettings {
    pub typed_search: bool,
    pub release_alt_switches: bool,
    pub release_right_button_switches: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowCommandRequest {
    pub command: WindowCommand,
    pub window_handle: isize,
    pub process_identity: ProcessIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitcherEffect {
    None,
    Open { selection_delta: Option<i32> },
    Hide,
    Redraw,
    Activate(isize),
    Execute(WindowCommandRequest),
}

#[derive(Debug)]
pub struct SwitcherSession {
    switcher: Switcher,
    settings: SwitcherSessionSettings,
    visible: bool,
    context_menu_open: bool,
}

impl SwitcherSession {
    #[must_use]
    pub const fn new(settings: SwitcherSessionSettings) -> Self {
        Self {
            switcher: Switcher {
                all_tasks: Vec::new(),
                visible_indices: Vec::new(),
                selected_visible_index: None,
                pinned_visible_start: None,
                filter: String::new(),
            },
            settings,
            visible: false,
            context_menu_open: false,
        }
    }

    pub fn open(
        &mut self,
        tasks: impl IntoIterator<Item = SwitchTask>,
        selection_delta: Option<i32>,
    ) {
        self.switcher.clear_filter();
        self.switcher.set_tasks(tasks);
        if let Some(delta) = selection_delta {
            self.switcher.select_next(delta);
        }
        self.visible = self.switcher.visible_task_count() != 0;
    }

    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    #[must_use]
    pub const fn switcher(&self) -> &Switcher {
        &self.switcher
    }

    #[must_use]
    pub const fn switcher_mut(&mut self) -> &mut Switcher {
        &mut self.switcher
    }

    pub fn update_settings(&mut self, settings: SwitcherSessionSettings) {
        self.settings = settings;
    }

    pub fn set_context_menu_open(&mut self, open: bool) {
        self.context_menu_open = open;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn restore_visible(&mut self) {
        self.visible = true;
    }

    #[must_use]
    pub const fn search_active(&self) -> bool {
        self.visible && self.settings.typed_search
    }

    #[must_use]
    pub fn handle_input(&mut self, action: InputAction) -> SwitcherEffect {
        if !self.visible {
            return match action {
                InputAction::Switch(delta) => SwitcherEffect::Open {
                    selection_delta: Some(delta),
                },
                InputAction::RightButtonPressed => SwitcherEffect::Open {
                    selection_delta: None,
                },
                _ => SwitcherEffect::None,
            };
        }

        match action {
            InputAction::Switch(delta) => {
                self.switcher.select_next(delta);
                SwitcherEffect::Redraw
            }
            InputAction::Navigate(delta) => {
                self.switcher.select_bounded(delta);
                SwitcherEffect::Redraw
            }
            InputAction::SelectFirst => {
                self.switcher.select_first();
                SwitcherEffect::Redraw
            }
            InputAction::SelectLast => {
                self.switcher.select_last();
                SwitcherEffect::Redraw
            }
            InputAction::DismissOverlay => {
                self.visible = false;
                SwitcherEffect::Hide
            }
            InputAction::CloseSelected => self.command(WindowCommand::Close),
            InputAction::WindowCommand(command) => self.command(command),
            InputAction::ActivateVisiblePosition(position) => {
                if self.switcher.select_visible_position(position) {
                    self.activate_selected()
                } else {
                    SwitcherEffect::None
                }
            }
            InputAction::ActivateSelected => self.activate_selected(),
            InputAction::AltReleased if self.settings.release_alt_switches => {
                self.activate_from_release()
            }
            InputAction::RightButtonReleased if self.settings.release_right_button_switches => {
                self.activate_from_release()
            }
            InputAction::RightButtonPressed => SwitcherEffect::Open {
                selection_delta: None,
            },
            InputAction::MouseWheel(delta) => {
                self.switcher.select_next(if delta > 0 { -1 } else { 1 });
                SwitcherEffect::Redraw
            }
            InputAction::AppendSearchCharacter(character) if self.settings.typed_search => {
                self.switcher.append_filter_character(character);
                SwitcherEffect::Redraw
            }
            InputAction::BackspaceSearch if self.settings.typed_search => {
                self.switcher.backspace_filter();
                SwitcherEffect::Redraw
            }
            InputAction::AltReleased
            | InputAction::RightButtonReleased
            | InputAction::AppendSearchCharacter(_)
            | InputAction::BackspaceSearch => SwitcherEffect::None,
        }
    }

    fn activate_from_release(&mut self) -> SwitcherEffect {
        if self.context_menu_open {
            SwitcherEffect::None
        } else {
            self.activate_selected()
        }
    }

    fn activate_selected(&mut self) -> SwitcherEffect {
        let Some(target) = self.switcher.selected_task().map(|task| task.window_handle) else {
            return SwitcherEffect::None;
        };
        self.visible = false;
        SwitcherEffect::Activate(target)
    }

    fn command(&self, command: WindowCommand) -> SwitcherEffect {
        self.switcher
            .selected_task()
            .map_or(SwitcherEffect::None, |task| {
                SwitcherEffect::Execute(WindowCommandRequest {
                    command,
                    window_handle: task.window_handle,
                    process_identity: task.process_identity,
                })
            })
    }
}

impl Switcher {
    pub fn set_tasks(&mut self, tasks: impl IntoIterator<Item = SwitchTask>) {
        self.all_tasks.clear();
        self.all_tasks.extend(tasks);
        self.apply_filter();
    }

    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: &str) {
        if self.filter != filter {
            filter.clone_into(&mut self.filter);
            self.apply_filter();
        }
    }

    pub fn clear_filter(&mut self) {
        self.set_filter("");
    }

    pub fn append_filter_character(&mut self, value: char) {
        self.filter.push(value);
        self.apply_filter();
    }

    pub fn backspace_filter(&mut self) {
        if self.filter.pop().is_some() {
            self.apply_filter();
        }
    }

    pub fn visible_tasks(&self) -> impl Iterator<Item = &SwitchTask> {
        self.visible_indices
            .iter()
            .filter_map(|index| self.all_tasks.get(*index))
    }

    pub fn positioned_visible_tasks(&self) -> impl Iterator<Item = (usize, &SwitchTask)> {
        self.visible_tasks()
            .enumerate()
            .map(|(index, task)| (index.saturating_add(1), task))
    }

    #[must_use]
    pub fn visible_task_count(&self) -> usize {
        self.visible_indices.len()
    }

    #[must_use]
    pub fn selected_task(&self) -> Option<&SwitchTask> {
        let task_index = *self.visible_indices.get(self.selected_visible_index?)?;
        self.all_tasks.get(task_index)
    }

    #[must_use]
    pub const fn selected_visible_index(&self) -> Option<usize> {
        self.selected_visible_index
    }

    pub fn select_next(&mut self, delta: i32) {
        self.pinned_visible_start = None;
        let count = self.visible_indices.len();
        if count == 0 {
            self.selected_visible_index = None;
            return;
        }

        let current = self.selected_visible_index.unwrap_or_else(|| {
            if delta >= 0 {
                count.saturating_sub(1)
            } else {
                0
            }
        });
        let count_signed = isize::try_from(count).unwrap_or(isize::MAX);
        let current_signed = isize::try_from(current).unwrap_or_default();
        let delta_signed = isize::try_from(delta).unwrap_or_default();
        let next = (current_signed + delta_signed).rem_euclid(count_signed);
        self.selected_visible_index = usize::try_from(next).ok();
    }

    pub fn select_bounded(&mut self, delta: i32) {
        self.pinned_visible_start = None;
        let count = self.visible_indices.len();
        if count == 0 {
            self.selected_visible_index = None;
            return;
        }

        let last = count.saturating_sub(1);
        let current = self
            .selected_visible_index
            .unwrap_or(if delta >= 0 { 0 } else { last });
        let current_signed = isize::try_from(current).unwrap_or_default();
        let last_signed = isize::try_from(last).unwrap_or(isize::MAX);
        let delta_signed = isize::try_from(delta).unwrap_or_default();
        let next = current_signed
            .saturating_add(delta_signed)
            .clamp(0, last_signed);
        self.selected_visible_index = usize::try_from(next).ok();
    }

    #[must_use]
    pub fn visible_range(&self, visible_rows: usize) -> std::ops::Range<usize> {
        if visible_rows == 0 {
            return 0..0;
        }
        let count = self.visible_indices.len();
        let selected = self.selected_visible_index.unwrap_or_default();
        let max_start = count.saturating_sub(visible_rows);
        let centered_start = selected.saturating_sub(visible_rows / 2).min(max_start);
        let start = self
            .pinned_visible_start
            .map(|start| start.min(max_start))
            .filter(|start| selected >= *start && selected < start.saturating_add(visible_rows))
            .unwrap_or(centered_start);
        start..start.saturating_add(visible_rows).min(count)
    }

    pub fn pin_visible_range(&mut self, visible_rows: usize) {
        self.pinned_visible_start = Some(self.visible_range(visible_rows).start);
    }

    pub fn select_visible_position(&mut self, one_based_position: usize) -> bool {
        let Some(index) = one_based_position.checked_sub(1) else {
            return false;
        };
        if index >= self.visible_indices.len() {
            return false;
        }

        self.selected_visible_index = Some(index);
        true
    }

    pub fn select_first(&mut self) {
        self.pinned_visible_start = None;
        self.selected_visible_index = (!self.visible_indices.is_empty()).then_some(0);
    }

    pub fn select_last(&mut self) {
        self.pinned_visible_start = None;
        self.selected_visible_index = self.visible_indices.len().checked_sub(1);
    }

    fn apply_filter(&mut self) {
        self.rebuild_visible_indices();
        self.select_first();
    }

    fn rebuild_visible_indices(&mut self) {
        let normalized = self.filter.trim().to_lowercase();
        self.visible_indices.clear();
        for (index, task) in self.all_tasks.iter().enumerate() {
            if normalized.is_empty()
                || task.number.to_string().contains(&normalized)
                || task.title.to_lowercase().contains(&normalized)
                || task.process_name.to_lowercase().contains(&normalized)
            {
                self.visible_indices.push(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks() -> Vec<SwitchTask> {
        vec![
            SwitchTask::new(1, 10, "Project - Zed", "zed"),
            SwitchTask::new(2, 20, "Documentation", "firefox"),
        ]
    }

    #[test]
    fn session_open_switch_activate_sequence_is_explicit() {
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });

        assert_eq!(
            session.handle_input(InputAction::Switch(1)),
            SwitcherEffect::Open {
                selection_delta: Some(1)
            }
        );
        session.open(tasks(), Some(1));
        assert!(session.is_visible());
        assert_eq!(
            session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(20)
        );
        assert_eq!(
            session.handle_input(InputAction::ActivateSelected),
            SwitcherEffect::Activate(20)
        );
        assert!(!session.is_visible());
    }

    #[test]
    fn empty_open_dismisses_and_allows_a_later_successful_invocation() {
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });

        assert_eq!(
            session.handle_input(InputAction::Switch(1)),
            SwitcherEffect::Open {
                selection_delta: Some(1)
            }
        );
        session.open([], Some(1));

        assert!(!session.is_visible());
        assert_eq!(
            session.handle_input(InputAction::AltReleased),
            SwitcherEffect::None
        );
        assert_eq!(
            session.handle_input(InputAction::Switch(1)),
            SwitcherEffect::Open {
                selection_delta: Some(1)
            }
        );

        session.open(tasks(), Some(1));
        assert!(session.is_visible());
        assert_eq!(
            session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(20)
        );
        assert_eq!(
            session.handle_input(InputAction::AltReleased),
            SwitcherEffect::Activate(20)
        );
        assert!(!session.is_visible());
    }

    #[test]
    fn session_release_activation_requires_setting_and_clear_context_menu() {
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: false,
        });
        session.open(tasks(), None);

        assert_eq!(
            session.handle_input(InputAction::RightButtonReleased),
            SwitcherEffect::None
        );
        session.set_context_menu_open(true);
        assert_eq!(
            session.handle_input(InputAction::AltReleased),
            SwitcherEffect::None
        );
        assert!(session.is_visible());

        session.set_context_menu_open(false);
        assert_eq!(
            session.handle_input(InputAction::AltReleased),
            SwitcherEffect::Activate(10)
        );
        assert!(!session.is_visible());
    }

    #[test]
    fn session_commands_capture_the_selected_process_identity() {
        let identity = ProcessIdentity::new(42, 9001);
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });
        session.open(
            [SwitchTask::new(1, 20, "Selected", "editor").with_process_identity(identity)],
            None,
        );

        assert_eq!(
            session.handle_input(InputAction::WindowCommand(WindowCommand::Terminate)),
            SwitcherEffect::Execute(WindowCommandRequest {
                command: WindowCommand::Terminate,
                window_handle: 20,
                process_identity: identity,
            })
        );
    }

    #[test]
    fn reopening_session_clears_search_and_replaces_tasks() {
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });
        session.open(tasks(), None);
        assert_eq!(
            session.handle_input(InputAction::AppendSearchCharacter('z')),
            SwitcherEffect::Redraw
        );
        assert_eq!(session.switcher().visible_task_count(), 1);

        session.open(
            [
                SwitchTask::new(1, 30, "Terminal", "terminal"),
                SwitchTask::new(2, 40, "Files", "explorer"),
            ],
            None,
        );

        assert!(session.switcher().filter().is_empty());
        assert_eq!(session.switcher().visible_task_count(), 2);
        assert_eq!(
            session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(30)
        );
    }

    #[test]
    fn setting_tasks_selects_first_task() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());

        assert_eq!(switcher.visible_task_count(), 2);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn filter_matches_number_title_and_process_case_insensitively() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());

        for (filter, expected_handle) in [("2", 20), ("DOC", 20), ("FIREFOX", 20)] {
            switcher.set_filter(filter);
            assert_eq!(
                switcher
                    .visible_tasks()
                    .next()
                    .map(|task| task.window_handle),
                Some(expected_handle)
            );
            assert_eq!(switcher.visible_task_count(), 1);
        }
    }

    #[test]
    fn filtered_tasks_are_numbered_by_their_visible_shortcut_positions() {
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 10, "Match first", "editor"),
            SwitchTask::new(2, 20, "Unrelated", "browser"),
            SwitchTask::new(3, 30, "Match last", "terminal"),
        ]);
        switcher.set_filter("match");

        assert_eq!(
            switcher
                .positioned_visible_tasks()
                .map(|(position, task)| (position, task.number, task.window_handle))
                .collect::<Vec<_>>(),
            vec![(1, 1, 10), (2, 3, 30)]
        );
        assert!(switcher.select_visible_position(2));
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(30)
        );
    }

    #[test]
    fn filter_matches_unicode_window_metadata_case_insensitively() {
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 10, "КАЛЬКУЛЯТОР", "CalculatorApp"),
            SwitchTask::new(2, 20, "Notes", "Notepad"),
        ]);

        switcher.set_filter("кальк");

        assert_eq!(switcher.visible_task_count(), 1);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());

        switcher.select_next(-1);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(20)
        );
        switcher.select_next(1);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn bounded_navigation_stops_at_first_and_last_task() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());

        switcher.select_bounded(-1);
        assert_eq!(switcher.selected_visible_index(), Some(0));

        switcher.select_bounded(1);
        assert_eq!(switcher.selected_visible_index(), Some(1));
        switcher.select_bounded(1);
        assert_eq!(switcher.selected_visible_index(), Some(1));
    }

    #[test]
    fn bounded_navigation_keeps_the_selection_in_the_visible_range() {
        let mut switcher = Switcher::default();
        switcher.set_tasks((1..=8).map(|number| {
            SwitchTask::new(
                number,
                isize::try_from(number).unwrap_or_default(),
                "Task",
                "process",
            )
        }));

        for expected in 1..8 {
            switcher.select_bounded(1);
            assert_eq!(switcher.selected_visible_index(), Some(expected));
            assert!(switcher.visible_range(3).contains(&expected));
        }
        assert_eq!(switcher.visible_range(3), 5..8);
    }

    #[test]
    fn filtering_resets_selection_and_backspace_restores_tasks() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());
        switcher.append_filter_character('z');

        assert_eq!(switcher.visible_task_count(), 1);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );

        switcher.backspace_filter();
        assert_eq!(switcher.visible_task_count(), 2);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn typed_filter_query_can_be_edited_one_character_at_a_time() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());
        switcher.append_filter_character('d');
        switcher.append_filter_character('o');
        assert_eq!(switcher.visible_task_count(), 1);
        assert_eq!(switcher.filter(), "do");

        switcher.backspace_filter();

        assert_eq!(switcher.filter(), "d");
        assert_eq!(switcher.visible_task_count(), 2);
    }

    #[test]
    fn visible_position_is_one_based_and_bounds_checked() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());

        assert!(switcher.select_visible_position(2));
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(20)
        );
        assert!(!switcher.select_visible_position(0));
        assert!(!switcher.select_visible_position(3));
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(20)
        );
    }

    #[test]
    fn pointer_pinned_range_stays_stable_until_keyboard_navigation() {
        let mut switcher = Switcher::default();
        switcher.set_tasks((1..=10).map(|number| {
            SwitchTask::new(
                number,
                isize::try_from(number).unwrap_or_default(),
                "Task",
                "process",
            )
        }));
        assert!(switcher.select_visible_position(8));
        assert_eq!(switcher.visible_range(2), 6..8);

        switcher.pin_visible_range(2);
        assert!(switcher.select_visible_position(7));
        assert_eq!(switcher.visible_range(2), 6..8);

        switcher.select_bounded(-1);
        assert_eq!(switcher.visible_range(2), 4..6);
    }

    #[test]
    fn empty_results_have_no_selection() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(tasks());
        switcher.set_filter("missing");

        assert_eq!(switcher.visible_task_count(), 0);
        assert!(switcher.selected_task().is_none());
        switcher.select_next(1);
        assert!(switcher.selected_task().is_none());
        switcher.select_first();
        assert!(switcher.selected_task().is_none());
        switcher.select_last();
        assert!(switcher.selected_task().is_none());
    }

    #[test]
    fn boundary_selection_uses_only_filtered_tasks() {
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 10, "Alpha first", "editor"),
            SwitchTask::new(2, 20, "Unrelated", "browser"),
            SwitchTask::new(3, 30, "Alpha last", "terminal"),
        ]);
        switcher.set_filter("alpha");

        switcher.select_last();
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(30)
        );
        switcher.select_first();
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn eligibility_matches_the_existing_top_level_window_policy() {
        let ordinary = WindowEligibility {
            title: "Document",
            class_name: "EditorWindow",
            is_visible: true,
            is_current_process: false,
            is_cloaked: false,
            is_tool_window: false,
            has_owner: false,
            is_app_window: false,
            matches_monitor_filter: true,
        };
        assert!(is_switchable_window(&ordinary));

        assert!(!is_switchable_window(&WindowEligibility {
            class_name: "Shell_TrayWnd",
            ..ordinary
        }));
        assert!(!is_switchable_window(&WindowEligibility {
            has_owner: true,
            ..ordinary
        }));
        assert!(is_switchable_window(&WindowEligibility {
            has_owner: true,
            is_app_window: true,
            ..ordinary
        }));
        assert!(!is_switchable_window(&WindowEligibility {
            matches_monitor_filter: false,
            ..ordinary
        }));
    }
}
