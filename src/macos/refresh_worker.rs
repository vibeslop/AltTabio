//! Accessibility calls wait on the app they ask, up to a timeout, so one hung app would freeze
//! the switcher if the main thread made them.

use super::ax::AppObserver;
use super::runtime::post_refresh_to_app;
use super::window_list::{self, AX_TIMEOUT_SECONDS, EnumerationOptions, Unresponsive};
use alttabio::refresh_schedule::{RefreshBatch, RefreshSchedule};
use std::sync::mpsc;

/// Accessibility work for the window list thread.
#[derive(Clone, Copy)]
pub(super) enum Job {
    List(EnumerationOptions),
    /// Follow this app's windows, replacing the app followed before.
    Watch(Option<i32>),
}

type Batch = RefreshBatch<EnumerationOptions, Option<i32>>;

/// The thread that asks apps over Accessibility, so an app that is slow to answer never stalls
/// the main thread.
pub(super) struct RefreshWorker {
    sender: Option<mpsc::SyncSender<Batch>>,
    schedule: RefreshSchedule<EnumerationOptions, Option<i32>>,
}

impl RefreshWorker {
    pub(super) fn spawn() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Batch>(1);
        std::thread::Builder::new()
            .name("alttabio-window-list".to_owned())
            .spawn(move || {
                let mut unresponsive = Unresponsive::default();
                while let Ok((list, watch)) = receiver.recv() {
                    // AppKit drains no pool on a thread it didn't start, so without this
                    // every autoreleased runningApplications snapshot lives forever.
                    objc2::rc::autoreleasepool(|_| {
                        let observer = watch
                            .flatten()
                            .map(|pid| (pid, AppObserver::new(pid, AX_TIMEOUT_SECONDS)));
                        let listing =
                            list.map(|options| window_list::enumerate(options, &mut unresponsive));
                        // No next batch is sent until this completion reaches the app.
                        // Thus a stalled main thread retains at most one result block.
                        post_refresh_to_app(move |app| app.refresh_finished(listing, observer));
                    });
                }
            })
            .map_or_else(
                |error| {
                    eprintln!("Could not start the window list thread: {error}");
                    Self {
                        sender: None,
                        schedule: {
                            let mut schedule = RefreshSchedule::default();
                            schedule.stop();
                            schedule
                        },
                    }
                },
                |_handle| Self {
                    sender: Some(sender),
                    schedule: RefreshSchedule::default(),
                },
            )
    }

    pub(super) fn request(&mut self, job: Job) {
        let batch = match job {
            Job::List(options) => self.schedule.request_list(options),
            Job::Watch(pid) => self.schedule.request_watch(pid),
        };
        self.submit(batch);
    }

    pub(super) fn complete(&mut self) {
        let batch = self.schedule.complete();
        self.submit(batch);
    }

    fn submit(&mut self, batch: Option<Batch>) {
        if let (Some(sender), Some(batch)) = (&self.sender, batch)
            && let Err(error) = sender.try_send(batch)
        {
            eprintln!("The window list thread cannot accept work: {error}; keeping the last list");
            self.stop();
        }
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.schedule.is_stopped()
    }

    pub(super) fn stop(&mut self) {
        self.schedule.stop();
        // Closing the channel wakes an idle worker; ongoing AX work finishes off-thread.
        self.sender = None;
    }
}
