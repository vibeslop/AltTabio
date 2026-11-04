//! Replays ordinary right clicks on a worker so the hook thread can keep delivering input.

use super::replay::REPLAYED_INPUT_MARKER;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};

const QUEUED_CLICKS: usize = 16;

pub(super) struct ReplayWorker {
    sender: Option<SyncSender<()>>,
    join: Option<JoinHandle<()>>,
}

impl ReplayWorker {
    pub(super) fn start() -> Result<Self, String> {
        Self::start_with(|| {
            if !replay_right_click() {
                eprintln!("Could not replay a right click or balance a partially inserted press");
            }
        })
    }

    fn start_with(mut replay: impl FnMut() + Send + 'static) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(QUEUED_CLICKS);
        let join = thread::Builder::new()
            .name("alttabio-mouse-replay".to_owned())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    replay();
                }
            })
            .map_err(|error| format!("Could not start mouse replay worker: {error}"))?;
        Ok(Self {
            sender: Some(sender),
            join: Some(join),
        })
    }

    pub(super) fn request(&self) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(()).is_ok())
    }
}

impl Drop for ReplayWorker {
    fn drop(&mut self) {
        // The owner removes its hooks before dropping us, so joining cannot wait on a callback
        // to the joining thread. Disconnect first so the worker exits after balancing its clicks.
        drop(self.sender.take());
        if let Some(join) = self.join.take()
            && let Err(error) = join.join()
        {
            eprintln!("Mouse replay worker panicked during shutdown: {error:?}");
        }
    }
}

fn replay_right_click() -> bool {
    replay_right_click_with(|inputs, size| unsafe {
        // SAFETY: the helper provides initialized mouse INPUT records. SendInput copies them
        // synchronously and retains no references. The marker bypasses physical gesture tracking.
        SendInput(inputs, size)
    })
}

pub(super) fn replay_right_click_with(mut send: impl FnMut(&[INPUT], i32) -> u32) -> bool {
    let inputs = [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
                ..Default::default()
            },
        },
    });
    let Ok(size) = i32::try_from(core::mem::size_of::<INPUT>()) else {
        return false;
    };
    match send(&inputs, size) {
        2 => true,
        1 => {
            // A partial batch inserted the press. Complete its matching release.
            send(&inputs[1..], size) == 1
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_native_replay_never_blocks_the_requesting_hook_thread() {
        use std::time::Duration;
        let (started, observed) = mpsc::channel();
        let (release, resume) = mpsc::sync_channel(1);
        let mut first = true;
        let worker = ReplayWorker::start_with(move || {
            started
                .send(thread::current().id())
                .unwrap_or_else(|error| panic!("observe replay thread: {error}"));
            if first {
                first = false;
                resume
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|error| panic!("resume blocked replay: {error}"));
            }
        })
        .unwrap_or_else(|error| panic!("start replay: {error}"));
        assert!(worker.request());
        let replay_thread = observed
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("replay did not start: {error}"));
        assert_ne!(replay_thread, thread::current().id());
        // Native injection is still blocked. Hook dispatch must remain nonblocking and bounded.
        for _ in 0..QUEUED_CLICKS {
            assert!(worker.request());
        }
        assert!(!worker.request());
        release
            .send(())
            .unwrap_or_else(|error| panic!("release replay: {error}"));
        drop(worker);
        assert_eq!(observed.into_iter().count(), QUEUED_CLICKS);
    }

    #[test]
    fn ordinary_click_replay_is_one_tagged_balanced_pair() {
        let mut flags = Vec::new();
        assert!(replay_right_click_with(|inputs, _| {
            for input in inputs {
                assert_eq!(input.r#type, INPUT_MOUSE);
                // SAFETY: the helper initializes the mouse union member for INPUT_MOUSE.
                let mouse = unsafe { input.Anonymous.mi };
                assert_eq!(mouse.dwExtraInfo, REPLAYED_INPUT_MARKER);
                flags.push(mouse.dwFlags);
            }
            u32::try_from(inputs.len()).unwrap_or_default()
        }));
        assert_eq!(flags, [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP]);
    }

    #[test]
    fn partial_click_replay_completes_the_inserted_press() {
        let mut calls = 0;
        assert!(replay_right_click_with(|inputs, _| {
            calls += 1;
            if calls == 1 {
                return 1;
            }
            assert_eq!(inputs.len(), 1);
            // SAFETY: the helper initializes the mouse union member.
            assert_eq!(
                unsafe { inputs[0].Anonymous.mi }.dwFlags,
                MOUSEEVENTF_RIGHTUP
            );
            1
        }));
        assert_eq!(calls, 2);
    }
}
