use super::{App, CLOSE_REFRESH_TIMER_ID};
use crate::task_query::{EnumeratedTasks, enumerate_switchable_windows};
use crate::win_events::{self, LISTED_REFRESH_RETRY_DELAY_MS, LISTED_REFRESH_RETRY_TIMER_ID};
use crate::window_commands::execute as execute_window_command;
use alttabio::switcher::WindowCommandRequest;
use alttabio::task_refresh::{
    ContextMenuCommandOutcome, RefreshDecision, RetryTimer, apply_listed_refresh_batch,
};
use alttabio::window_command::WindowCommand;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::Error;

const CLOSE_REFRESH_DELAY_MS: u32 = 250;

impl App {
    pub(super) fn execute_window_command(
        &mut self,
        request: WindowCommandRequest,
    ) -> ContextMenuCommandOutcome {
        let command = request.command;
        if !execute_window_command(
            request.command,
            request.window_handle,
            request.process_identity,
        ) {
            eprintln!("Could not execute {command:?} for the selected window");
            return ContextMenuCommandOutcome::Failed;
        }
        self.close_button.reset();
        ContextMenuCommandOutcome::Succeeded {
            close_window: (command == WindowCommand::Close).then_some(request.window_handle),
        }
    }

    fn start_close_refresh_timer(&mut self) {
        let timer_id = unsafe {
            // SAFETY: the live overlay HWND owns this timer and no callback pointer is retained.
            SetTimer(
                Some(self.hwnd),
                CLOSE_REFRESH_TIMER_ID,
                CLOSE_REFRESH_DELAY_MS,
                None,
            )
        };
        if timer_id == 0 {
            self.task_refresh.cancel_retries();
            eprintln!(
                "Could not schedule a follow-up refresh after closing a window: {}",
                Error::from_thread()
            );
        }
    }

    fn stop_close_refresh_timer(&mut self) {
        let result = unsafe {
            // SAFETY: this handles the timer owned by the live overlay HWND.
            KillTimer(Some(self.hwnd), CLOSE_REFRESH_TIMER_ID)
        };
        if let Err(error) = result {
            eprintln!("Could not stop the close refresh timer: {error}");
        }
    }

    pub(super) fn handle_close_refresh_timer(&mut self) {
        if self.session.context_menu_open() {
            return;
        }
        if !self.task_refresh.has_pending_retries() {
            self.stop_close_refresh_timer();
            return;
        }
        self.refresh_switcher_tasks();
    }

    pub(super) fn handle_listed_window_refresh(&mut self) {
        self.ingest_listed_refresh_signal();
        self.run_pending_task_refresh();
    }

    pub(super) fn start_listed_refresh_retry_timer(&mut self) -> std::result::Result<(), String> {
        let timer_id = unsafe {
            // SAFETY: the live overlay HWND owns this timer and no callback pointer is retained.
            SetTimer(
                Some(self.hwnd),
                LISTED_REFRESH_RETRY_TIMER_ID,
                LISTED_REFRESH_RETRY_DELAY_MS,
                None,
            )
        };
        if timer_id == 0 {
            return Err(format!(
                "Could not start reliable live window-list updates: {}\n\nThe window event watcher has been disabled.",
                Error::from_thread()
            ));
        }
        self.listed_refresh_retry_timer_armed = true;
        Ok(())
    }

    pub(super) fn stop_listed_refresh_retry_timer(&mut self) {
        if !self.listed_refresh_retry_timer_armed {
            return;
        }
        self.listed_refresh_retry_timer_armed = false;
        let result = unsafe {
            // SAFETY: this stops the timer owned by the live overlay HWND during shutdown.
            KillTimer(Some(self.hwnd), LISTED_REFRESH_RETRY_TIMER_ID)
        };
        if let Err(error) = result {
            eprintln!("Could not stop the listed-window refresh retry timer: {error}");
        }
    }

    pub(super) fn handle_listed_refresh_retry_timer(&mut self) {
        if win_events::foreground_check_needs_retry() {
            self.handle_foreground_check();
        }
        let Some(batch) = win_events::take_listed_refresh_retry() else {
            return;
        };
        apply_listed_refresh_batch(&mut self.task_refresh, batch);
        self.run_pending_task_refresh();
    }

    pub(super) fn ingest_listed_refresh_signal(&mut self) {
        apply_listed_refresh_batch(
            &mut self.task_refresh,
            win_events::take_listed_refresh_notices(),
        );
    }

    pub(super) fn run_pending_task_refresh(&mut self) {
        match self.task_refresh.decision(
            self.session.is_visible(),
            |window_handle| {
                self.session
                    .switcher()
                    .contains_window_handle(window_handle)
            },
            self.session.context_menu_open(),
        ) {
            RefreshDecision::Ignore => {
                self.task_refresh.clear_notices();
            }
            RefreshDecision::Defer => {}
            RefreshDecision::Refresh => self.refresh_switcher_tasks(),
        }
    }

    fn refresh_switcher_tasks(&mut self) {
        let timer = match enumerate_switchable_windows(&self.settings) {
            Ok(EnumeratedTasks { tasks, icons }) => {
                let timer = self.task_refresh.complete_enumeration(Ok(&tasks));
                self.session.refresh_tasks(tasks);
                self.task_icons = icons;
                timer
            }
            Err(error) => {
                eprintln!("Could not refresh windows: {error}");
                self.task_refresh.complete_enumeration(Err(()))
            }
        };
        match timer {
            RetryTimer::Start => self.start_close_refresh_timer(),
            RetryTimer::Stop => self.stop_close_refresh_timer(),
            RetryTimer::Keep => {}
        }
        self.sync_overlay_after_task_refresh();
    }

    fn sync_overlay_after_task_refresh(&mut self) {
        if self.session.is_visible() {
            self.request_redraw();
        } else {
            self.hide_overlay();
        }
    }
}
