//! Bounded background enumeration. Native metadata and icon queries never block the UI.

use crate::task_query::{EnumeratedTasks, enumerate_switchable_windows};
use alttabio::settings::Settings;
use std::sync::{Arc, Mutex, mpsc};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::Result;

pub const WM_TASK_SNAPSHOT: u32 = WM_APP + 22;
type Request = (u64, Settings);
type Response = (u64, Result<EnumeratedTasks>);

pub struct SnapshotWorker {
    sender: mpsc::SyncSender<()>,
    pending: Arc<Mutex<Option<Request>>>,
    completed: Arc<Mutex<Option<Response>>>,
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this guard is created and dropped on the initialized worker thread.
        unsafe {
            CoUninitialize();
        }
    }
}

impl SnapshotWorker {
    pub fn spawn(target: HWND) -> std::result::Result<Self, String> {
        let target = target.0 as isize;
        Self::spawn_with(enumerate_switchable_windows, move || {
            // SAFETY: the scalar HWND is borrowed; no pointers cross the message boundary.
            unsafe {
                PostMessageW(
                    Some(HWND(target as *mut _)),
                    WM_TASK_SNAPSHOT,
                    WPARAM(0),
                    LPARAM(0),
                )
            }
        })
    }

    fn spawn_with(
        enumerate: impl Fn(&Settings) -> Result<EnumeratedTasks> + Send + 'static,
        notify: impl Fn() -> Result<()> + Send + 'static,
    ) -> std::result::Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(None::<Request>));
        let completed = Arc::new(Mutex::new(None::<Response>));
        let requests = Arc::clone(&pending);
        let responses = Arc::clone(&completed);
        std::thread::Builder::new()
            .name("alttabio-task-snapshot".to_owned())
            .spawn(move || {
                // SAFETY: COM initialization and its guard stay on this single worker thread.
                let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() };
                let _apartment = initialized.is_ok().then_some(Apartment);
                while receiver.recv().is_ok() {
                    let request = match requests.lock() {
                        Ok(mut pending) => pending.take(),
                        Err(_) => return,
                    };
                    let Some((generation, settings)) = request else {
                        continue;
                    };
                    let result = match &initialized {
                        Ok(()) => enumerate(&settings),
                        Err(error) => Err(error.clone()),
                    };
                    let Ok(mut completed) = responses.lock() else {
                        return;
                    };
                    // One completion is retained. Until it is drained its posted wakeup also
                    // serves newer results, so a stalled UI cannot accumulate messages.
                    let post = completed.is_none();
                    *completed = Some((generation, result));
                    drop(completed);
                    if post && let Err(error) = notify() {
                        eprintln!("Could not deliver the window snapshot: {error}");
                        return;
                    }
                }
            })
            .map_err(|error| format!("Could not start the window snapshot worker: {error}"))?;
        Ok(Self {
            sender,
            pending,
            completed,
        })
    }

    pub fn request(&self, generation: u64, settings: &Settings) -> std::result::Result<(), String> {
        *self
            .pending
            .lock()
            .map_err(|_| "The window snapshot request state is unavailable")? =
            Some((generation, settings.clone()));
        match self.sender.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(())) => {
                Err("The window snapshot worker stopped".to_owned())
            }
        }
    }

    pub fn take(&self) -> Option<Response> {
        self.completed.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn blocked_enumeration_keeps_only_the_latest_request_and_one_wakeup() {
        let (entered, entry) = mpsc::sync_channel(1);
        let (resume, paused) = mpsc::sync_channel(1);
        let calls = Arc::new(AtomicUsize::new(0));
        let invocations = Arc::clone(&calls);
        let notices = Arc::new(AtomicUsize::new(0));
        let notifications = Arc::clone(&notices);
        let worker = SnapshotWorker::spawn_with(
            move |_| {
                if invocations.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered
                        .send(())
                        .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
                    paused
                        .recv()
                        .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
                }
                Ok(EnumeratedTasks {
                    tasks: Vec::new(),
                    icons: crate::task_icon::TaskIcons::default(),
                })
            },
            move || {
                notifications.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
        worker
            .request(0, &Settings::default())
            .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
        entry
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
        for generation in 1..=100_000 {
            worker
                .request(generation, &Settings::default())
                .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            worker
                .pending
                .lock()
                .unwrap_or_else(|error| panic!("test fixture failed: {error}"))
                .as_ref()
                .unwrap_or_else(|| panic!("pending request missing"))
                .0,
            100_000
        );
        resume
            .send(())
            .unwrap_or_else(|error| panic!("test fixture failed: {error}"));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if worker
                .completed
                .lock()
                .unwrap_or_else(|error| panic!("test fixture failed: {error}"))
                .as_ref()
                .is_some_and(|(generation, _)| *generation == 100_000)
            {
                break;
            }
            assert!(Instant::now() < deadline, "latest snapshot never completed");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(notices.load(Ordering::SeqCst), 1);
        assert_eq!(
            worker
                .take()
                .unwrap_or_else(|| panic!("completion missing"))
                .0,
            100_000
        );
        assert!(worker.take().is_none());
    }
}
