//! Pointer interaction with the overlay's task list: hover selection and the close button.

use crate::close_button::CloseButtonVisualState;
use crate::switcher::Switcher;
use crate::window_command::WindowCommand;

/// Targets are window handles, so a press stays bound to the window it started on even if the
/// selection moves under the pointer.
#[derive(Default)]
pub struct CloseButtonInteraction {
    hovered_target: Option<isize>,
    pressed_target: Option<isize>,
}

impl CloseButtonInteraction {
    #[must_use]
    pub fn visual_state(&self, selected_target: Option<isize>) -> CloseButtonVisualState {
        let Some(selected_target) = selected_target else {
            return CloseButtonVisualState::Normal;
        };
        if self.pressed_target == Some(selected_target)
            && self.hovered_target == Some(selected_target)
        {
            CloseButtonVisualState::Pressed
        } else if self.hovered_target == Some(selected_target) {
            CloseButtonVisualState::Hovered
        } else {
            CloseButtonVisualState::Normal
        }
    }

    /// Returns whether the hover changed and the button needs a redraw.
    pub fn update_hover(&mut self, target: Option<isize>) -> bool {
        if self.hovered_target == target {
            return false;
        }
        self.hovered_target = target;
        true
    }

    pub fn press(&mut self, target: isize) {
        self.hovered_target = Some(target);
        self.pressed_target = Some(target);
    }

    pub fn release(&mut self, target: Option<isize>) -> Option<WindowCommand> {
        let pressed_target = self.pressed_target.take();
        self.hovered_target = target;
        (pressed_target.is_some() && pressed_target == target).then_some(WindowCommand::Close)
    }

    pub fn cancel_press(&mut self) -> bool {
        self.pressed_target.take().is_some()
    }

    #[must_use]
    pub const fn is_pressed(&self) -> bool {
        self.pressed_target.is_some()
    }

    pub fn reset(&mut self) {
        self.hovered_target = None;
        self.pressed_target = None;
    }
}

/// Returns whether hovering moved the selection, so resting on one row does not redraw.
pub fn select_hovered_position(switcher: &mut Switcher, position: usize) -> bool {
    let previous = switcher.selected_visible_index();
    switcher.select_visible_position(position) && switcher.selected_visible_index() != previous
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switcher::SwitchTask;

    #[test]
    fn hover_selection_requests_redraw_only_when_the_item_changes() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(vec![
            SwitchTask::new(1, 10, "First", "first"),
            SwitchTask::new(2, 20, "Second", "second"),
        ]);

        for _ in 0..1_000 {
            assert!(!select_hovered_position(&mut switcher, 1));
        }
        assert!(select_hovered_position(&mut switcher, 2));
        for _ in 0..1_000 {
            assert!(!select_hovered_position(&mut switcher, 2));
        }
        assert!(!select_hovered_position(&mut switcher, 3));
    }

    #[test]
    fn close_button_visual_state_tracks_hover_press_and_leave() {
        let mut interaction = CloseButtonInteraction::default();

        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Normal
        );
        assert!(interaction.update_hover(Some(10)));
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Hovered
        );
        interaction.press(10);
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Pressed
        );
        assert!(interaction.update_hover(None));
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Normal
        );
        assert!(interaction.cancel_press());
    }

    #[test]
    fn close_button_release_emits_only_the_existing_safe_close_command() {
        let mut interaction = CloseButtonInteraction::default();
        interaction.press(10);

        assert_eq!(interaction.release(Some(10)), Some(WindowCommand::Close));
        assert!(!interaction.is_pressed());

        interaction.press(10);
        assert_eq!(interaction.release(None), None);

        interaction.press(10);
        assert_eq!(interaction.release(Some(20)), None);
    }
}
