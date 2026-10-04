use super::ax::AppObserver;
use super::runtime::post_to_app;
use super::window_list::{self, AX_TIMEOUT_SECONDS, EnumerationOptions, Unresponsive};
use std::sync::mpsc;

/// Accessibility work for the window list thread.
pub(super) enum Job {
    List(EnumerationOptions),
    /// Follow this app's windows, replacing the app followed before.
    Watch(i32),
}

/// The thread that asks apps over Accessibility, so an app that is slow to answer never stalls
/// the main thread.
pub(super) struct RefreshWorker {
    sender: mpsc::Sender<Job>,
}

impl RefreshWorker {
    pub(super) fn spawn() -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("alttabio-window-list".to_owned())
            .spawn(move || {
                let mut unresponsive = Unresponsive::default();
                while let Ok(job) = receiver.recv() {
                    // Coalesce bursts of notifications: the latest job of each kind counts.
                    let mut list = None;
                    let mut watch = None;
                    for job in std::iter::once(job).chain(receiver.try_iter()) {
                        match job {
                            Job::List(options) => list = Some(options),
                            Job::Watch(pid) => watch = Some(pid),
                        }
                    }
                    // AppKit drains no pool on a thread it didn't start, so without this
                    // every autoreleased runningApplications snapshot lives forever.
                    objc2::rc::autoreleasepool(|_| {
                        if let Some(pid) = watch {
                            let observer = AppObserver::new(pid, AX_TIMEOUT_SECONDS);
                            post_to_app(move |app| app.observer_ready(pid, observer));
                        }
                        if let Some(options) = list {
                            let listing = window_list::enumerate(options, &mut unresponsive);
                            post_to_app(move |app| app.refresh_completed(listing));
                        }
                    });
                }
            })
            .map_or_else(
                |error| {
                    eprintln!("Could not start the window list thread: {error}");
                    Self {
                        sender: mpsc::channel().0,
                    }
                },
                |_handle| Self { sender },
            )
    }

    pub(super) fn request(&self, job: Job) {
        if self.sender.send(job).is_err() {
            eprintln!("The window list thread is gone; the switcher keeps its last list");
        }
    }
}
