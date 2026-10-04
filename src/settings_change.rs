//! Applying a confirmed Settings dialog to the subsystems that read it.

use crate::input::HookSettings;
use crate::settings::Settings;
use crate::switcher::SwitcherSessionSettings;

/// Search becomes active only while the overlay is open, so hooks always start without it.
#[must_use]
pub fn hook_settings(settings: &Settings) -> HookSettings {
    HookSettings {
        replace_alt_tab: settings.general.replace_alt_tab,
        replace_win_tab: settings.general.replace_win_tab,
        right_button_wheel_switching: settings.general.right_button_wheel_switching,
        typed_search: settings.general.typed_search,
        search_active: false,
    }
}

#[must_use]
pub const fn switcher_session_settings(settings: &Settings) -> SwitcherSessionSettings {
    SwitcherSessionSettings {
        typed_search: settings.general.typed_search,
        release_alt_switches: settings.general.release_alt_switches,
        release_right_button_switches: settings.general.release_right_button_switches,
    }
}

/// Autostart as the system reported it when the dialog opened. It can disagree with the stored
/// setting when the scheduled task was changed outside `AltTabio`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutostartState {
    pub enabled: bool,
    pub task_exists: bool,
}

/// The side effects of a settings change, injected so their order and rollback can be tested.
pub trait SettingsEffects {
    /// # Errors
    /// Returns a message for the user when the autostart task could not be changed.
    fn set_autostart(&mut self, enabled: bool) -> Result<(), String>;

    /// # Errors
    /// Returns a message for the user when the settings file could not be written.
    fn save(&mut self, settings: &Settings) -> Result<(), String>;

    /// Stops any running input hooks and starts new ones with `settings`.
    ///
    /// # Errors
    /// Returns a message for the user when the hooks could not start; none are running then.
    fn restart_hooks(&mut self, settings: HookSettings) -> Result<(), String>;
}

pub struct SettingsChange<'a> {
    pub previous: &'a Settings,
    pub next: &'a Settings,
    pub autostart: AutostartState,
    pub hooks_running: bool,
}

impl SettingsChange<'_> {
    /// Applies autostart, then the settings file, then the input hooks. When a step fails, the
    /// earlier steps are undone, so the caller keeps its previous settings.
    ///
    /// # Errors
    /// Returns the message to show the user, including any undo that also failed.
    pub fn apply(&self, effects: &mut impl SettingsEffects) -> Result<(), String> {
        if self.autostart_changed() {
            effects.set_autostart(self.next.general.autostart)?;
        }
        if let Err(mut message) = effects.save(self.next) {
            if let Err(rollback_error) = self.restore_autostart(effects) {
                message.push_str("\n\nAutostart rollback also failed: ");
                message.push_str(&rollback_error);
            }
            return Err(message);
        }

        let previous_hooks = hook_settings(self.previous);
        let next_hooks = hook_settings(self.next);
        if previous_hooks == next_hooks && self.hooks_running {
            return Ok(());
        }
        let Err(error) = effects.restart_hooks(next_hooks) else {
            return Ok(());
        };
        let hooks_rollback = effects.restart_hooks(previous_hooks);
        let settings_rollback = effects.save(self.previous);
        let autostart_rollback = self.restore_autostart(effects);
        let mut message = format!("The new input-hook settings could not be activated. {error}");
        if let Err(rollback_error) = hooks_rollback {
            message.push_str("\n\nThe previous input hooks could not be restored. ");
            message.push_str(&rollback_error);
        }
        if let Err(rollback_error) = settings_rollback {
            message.push_str("\n\nSettings rollback also failed: ");
            message.push_str(&rollback_error);
        }
        if let Err(rollback_error) = autostart_rollback {
            message.push_str("\n\nAutostart rollback also failed: ");
            message.push_str(&rollback_error);
        }
        Err(message)
    }

    /// A disabled setting with a leftover task still needs the task removed.
    const fn autostart_changed(&self) -> bool {
        self.next.general.autostart != self.autostart.enabled
            || (!self.next.general.autostart && self.autostart.task_exists)
    }

    fn restore_autostart(&self, effects: &mut impl SettingsEffects) -> Result<(), String> {
        if self.autostart_changed() {
            effects.set_autostart(self.autostart.enabled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Version {
        Previous,
        Next,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Effect {
        Autostart(bool),
        Save(Version),
        Hooks(Version),
    }

    /// Records each effect and fails the ones listed in `failing`, naming them in the error.
    struct Recorder {
        previous: Settings,
        next: Settings,
        failing: Vec<Effect>,
        applied: Vec<Effect>,
    }

    impl Recorder {
        fn new(previous: &Settings, next: &Settings, failing: &[Effect]) -> Self {
            Self {
                previous: previous.clone(),
                next: next.clone(),
                failing: failing.to_vec(),
                applied: Vec::new(),
            }
        }

        fn record(&mut self, effect: Effect) -> Result<(), String> {
            self.applied.push(effect);
            if self.failing.contains(&effect) {
                Err(format!("{effect:?} failed"))
            } else {
                Ok(())
            }
        }
    }

    impl SettingsEffects for Recorder {
        fn set_autostart(&mut self, enabled: bool) -> Result<(), String> {
            self.record(Effect::Autostart(enabled))
        }

        fn save(&mut self, settings: &Settings) -> Result<(), String> {
            let version = if *settings == self.next {
                Version::Next
            } else {
                assert_eq!(*settings, self.previous);
                Version::Previous
            };
            self.record(Effect::Save(version))
        }

        fn restart_hooks(&mut self, settings: HookSettings) -> Result<(), String> {
            let version = if settings == hook_settings(&self.next) {
                Version::Next
            } else {
                assert_eq!(settings, hook_settings(&self.previous));
                Version::Previous
            };
            self.record(Effect::Hooks(version))
        }
    }

    const AUTOSTART_ON: AutostartState = AutostartState {
        enabled: true,
        task_exists: true,
    };

    /// Turns autostart off and changes the hooks, so every step has something to apply.
    fn full_change() -> (Settings, Settings) {
        let previous = Settings::default();
        let mut next = previous.clone();
        next.general.autostart = false;
        next.general.replace_alt_tab = false;
        (previous, next)
    }

    /// Changes only an appearance setting, which neither autostart nor the hooks read.
    fn appearance_change() -> (Settings, Settings) {
        let previous = Settings::default();
        let mut next = previous.clone();
        next.appearance.compact_list = !previous.appearance.compact_list;
        (previous, next)
    }

    fn apply(
        previous: &Settings,
        next: &Settings,
        autostart: AutostartState,
        hooks_running: bool,
        failing: &[Effect],
    ) -> (Result<(), String>, Vec<Effect>) {
        let mut recorder = Recorder::new(previous, next, failing);
        let result = SettingsChange {
            previous,
            next,
            autostart,
            hooks_running,
        }
        .apply(&mut recorder);
        (result, recorder.applied)
    }

    #[test]
    fn a_full_change_applies_autostart_then_the_file_then_the_hooks() {
        let (previous, next) = full_change();

        assert_eq!(
            apply(&previous, &next, AUTOSTART_ON, true, &[]),
            (
                Ok(()),
                vec![
                    Effect::Autostart(false),
                    Effect::Save(Version::Next),
                    Effect::Hooks(Version::Next),
                ]
            )
        );
    }

    #[test]
    fn running_hooks_with_unchanged_settings_are_left_alone() {
        let (previous, next) = appearance_change();

        assert_eq!(
            apply(&previous, &next, AUTOSTART_ON, true, &[]),
            (Ok(()), vec![Effect::Save(Version::Next)])
        );
    }

    #[test]
    fn stopped_hooks_restart_even_with_unchanged_settings() {
        let (previous, next) = appearance_change();

        assert_eq!(
            apply(&previous, &next, AUTOSTART_ON, false, &[]),
            (
                Ok(()),
                vec![Effect::Save(Version::Next), Effect::Hooks(Version::Next)]
            )
        );
    }

    #[test]
    fn autostart_is_written_only_when_it_disagrees_with_the_system() {
        for (requested, enabled, task_exists, written) in [
            (true, false, false, true),
            (true, false, true, true),
            (false, true, true, true),
            (false, false, true, true),
            (true, true, true, false),
            (false, false, false, false),
        ] {
            let previous = Settings::default();
            let mut next = previous.clone();
            next.general.autostart = requested;
            let autostart = AutostartState {
                enabled,
                task_exists,
            };

            let (result, applied) = apply(&previous, &next, autostart, true, &[]);

            assert_eq!(result, Ok(()));
            assert_eq!(
                applied.contains(&Effect::Autostart(requested)),
                written,
                "requested {requested}, enabled {enabled}, task exists {task_exists}"
            );
        }
    }

    #[test]
    fn an_autostart_failure_stops_before_anything_else_changes() {
        let (previous, next) = full_change();

        assert_eq!(
            apply(
                &previous,
                &next,
                AUTOSTART_ON,
                true,
                &[Effect::Autostart(false)]
            ),
            (
                Err("Autostart(false) failed".to_owned()),
                vec![Effect::Autostart(false)]
            )
        );
    }

    #[test]
    fn a_save_failure_restores_autostart() {
        let (previous, next) = full_change();

        assert_eq!(
            apply(
                &previous,
                &next,
                AUTOSTART_ON,
                true,
                &[Effect::Save(Version::Next)]
            ),
            (
                Err("Save(Next) failed".to_owned()),
                vec![
                    Effect::Autostart(false),
                    Effect::Save(Version::Next),
                    Effect::Autostart(true),
                ]
            )
        );
    }

    #[test]
    fn a_save_failure_reports_a_failed_autostart_restore() {
        let (previous, next) = full_change();

        let (result, _) = apply(
            &previous,
            &next,
            AUTOSTART_ON,
            true,
            &[Effect::Save(Version::Next), Effect::Autostart(true)],
        );

        assert_eq!(
            result,
            Err(
                "Save(Next) failed\n\nAutostart rollback also failed: Autostart(true) failed"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_save_failure_leaves_unchanged_autostart_alone() {
        let (previous, next) = appearance_change();

        assert_eq!(
            apply(
                &previous,
                &next,
                AUTOSTART_ON,
                true,
                &[Effect::Save(Version::Next)]
            ),
            (
                Err("Save(Next) failed".to_owned()),
                vec![Effect::Save(Version::Next)]
            )
        );
    }

    #[test]
    fn a_hook_failure_restores_the_hooks_the_file_and_autostart() {
        let (previous, next) = full_change();

        assert_eq!(
            apply(
                &previous,
                &next,
                AUTOSTART_ON,
                true,
                &[Effect::Hooks(Version::Next)]
            ),
            (
                Err(
                    "The new input-hook settings could not be activated. Hooks(Next) failed"
                        .to_owned()
                ),
                vec![
                    Effect::Autostart(false),
                    Effect::Save(Version::Next),
                    Effect::Hooks(Version::Next),
                    Effect::Hooks(Version::Previous),
                    Effect::Save(Version::Previous),
                    Effect::Autostart(true),
                ]
            )
        );
    }

    #[test]
    fn a_hook_failure_leaves_unchanged_autostart_alone() {
        let (previous, mut next) = appearance_change();
        next.general.replace_alt_tab = false;

        let (_, applied) = apply(
            &previous,
            &next,
            AUTOSTART_ON,
            true,
            &[Effect::Hooks(Version::Next)],
        );

        assert_eq!(
            applied,
            vec![
                Effect::Save(Version::Next),
                Effect::Hooks(Version::Next),
                Effect::Hooks(Version::Previous),
                Effect::Save(Version::Previous),
            ]
        );
    }

    #[test]
    fn a_hook_failure_reports_each_failed_rollback_and_still_tries_the_rest() {
        let (previous, next) = full_change();
        let every_step = vec![
            Effect::Autostart(false),
            Effect::Save(Version::Next),
            Effect::Hooks(Version::Next),
            Effect::Hooks(Version::Previous),
            Effect::Save(Version::Previous),
            Effect::Autostart(true),
        ];

        for (rollback, report) in [
            (
                Effect::Hooks(Version::Previous),
                "The previous input hooks could not be restored. Hooks(Previous) failed",
            ),
            (
                Effect::Save(Version::Previous),
                "Settings rollback also failed: Save(Previous) failed",
            ),
            (
                Effect::Autostart(true),
                "Autostart rollback also failed: Autostart(true) failed",
            ),
        ] {
            let (result, applied) = apply(
                &previous,
                &next,
                AUTOSTART_ON,
                true,
                &[Effect::Hooks(Version::Next), rollback],
            );

            assert_eq!(
                result,
                Err(format!(
                    "The new input-hook settings could not be activated. Hooks(Next) failed\n\n{report}"
                ))
            );
            assert_eq!(applied, every_step);
        }
    }

    #[test]
    fn a_hook_failure_reports_every_failed_rollback_in_order() {
        let (previous, next) = full_change();

        let (result, _) = apply(
            &previous,
            &next,
            AUTOSTART_ON,
            true,
            &[
                Effect::Hooks(Version::Next),
                Effect::Hooks(Version::Previous),
                Effect::Save(Version::Previous),
                Effect::Autostart(true),
            ],
        );

        assert_eq!(
            result,
            Err(
                "The new input-hook settings could not be activated. Hooks(Next) failed\
                 \n\nThe previous input hooks could not be restored. Hooks(Previous) failed\
                 \n\nSettings rollback also failed: Save(Previous) failed\
                 \n\nAutostart rollback also failed: Autostart(true) failed"
                    .to_owned()
            )
        );
    }

    #[test]
    fn hook_settings_copy_the_switches_and_start_without_search() {
        let mut settings = Settings::default();
        settings.general.replace_alt_tab = false;
        settings.general.replace_win_tab = true;
        settings.general.right_button_wheel_switching = true;
        settings.general.typed_search = false;

        assert_eq!(
            hook_settings(&settings),
            HookSettings {
                replace_alt_tab: false,
                replace_win_tab: true,
                right_button_wheel_switching: true,
                typed_search: false,
                search_active: false,
            }
        );
    }

    #[test]
    fn session_settings_copy_search_and_release_switches() {
        let mut settings = Settings::default();
        settings.general.typed_search = true;
        settings.general.release_alt_switches = false;
        settings.general.release_right_button_switches = true;

        assert_eq!(
            switcher_session_settings(&settings),
            SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: false,
                release_right_button_switches: true,
            }
        );
    }
}
