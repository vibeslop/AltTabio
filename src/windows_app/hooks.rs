use super::App;
use super::passthrough::foreground_passthrough_policy;
use crate::hook::HookThread;
use alttabio::input::HookSettings;

impl App {
    pub(super) fn start_input_hooks(
        &mut self,
        settings: HookSettings,
    ) -> std::result::Result<(), String> {
        let policy = foreground_passthrough_policy(self.hwnd, &mut self.foreground_bounds_failures);
        if policy.bypasses_local_switching() && self.is_visible() {
            self.hide_overlay();
        }
        store_started_hook_then_sync(
            &mut self.hooks,
            HookThread::start(self.hwnd, settings),
            |hooks| {
                if let Err(error) = hooks.set_remote_desktop_passthrough(policy) {
                    eprintln!("{error}");
                }
            },
        )
    }

    pub(super) fn sync_hook_interception(&self) {
        let Some(hooks) = self.hooks.as_ref() else {
            return;
        };
        let suspended = self.modal_state().any_open();
        if suspended {
            hooks.set_interception_suspended(true);
        }
        hooks.set_search_active(!suspended && self.session.search_active());
        hooks.set_overlay_active(!suspended && self.session.is_visible());
        if !suspended {
            hooks.set_interception_suspended(false);
        }
    }

    pub(super) fn set_hook_search_active(&self, overlay_visible: bool) {
        if let Some(hooks) = &self.hooks {
            hooks.set_search_active(overlay_visible && self.settings.general.typed_search);
        }
    }

    pub(super) fn set_hook_overlay_active(&self, active: bool) {
        if let Some(hooks) = &self.hooks {
            hooks.set_overlay_active(active);
        }
    }

    pub(super) fn reset_hook_gestures(&self) {
        if let Some(hooks) = &self.hooks
            && let Err(error) = hooks.reset_gestures()
        {
            eprintln!("{error}");
        }
    }
}

fn store_started_hook_then_sync<H, E>(
    slot: &mut Option<H>,
    started: std::result::Result<H, E>,
    sync: impl FnOnce(&H),
) -> std::result::Result<(), E> {
    *slot = Some(started?);
    if let Some(hook) = slot.as_ref() {
        sync(hook);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_hook_starts_synchronize_passthrough_immediately() {
        let mut hooks = None;
        let mut synchronized = None;

        assert_eq!(
            store_started_hook_then_sync(&mut hooks, Ok::<_, ()>(42), |hook| {
                synchronized = Some(*hook);
            }),
            Ok(())
        );

        assert_eq!(hooks, Some(42));
        assert_eq!(synchronized, Some(42));

        let mut failed_hooks = None;
        let mut failure_synchronized = false;
        assert_eq!(
            store_started_hook_then_sync(&mut failed_hooks, Err::<i32, _>("start failed"), |_| {
                failure_synchronized = true;
            },),
            Err("start failed")
        );
        assert_eq!(failed_hooks, None);
        assert!(!failure_synchronized);
    }
}
