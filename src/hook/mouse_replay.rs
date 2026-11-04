//! Replays an ordinary right click after the physical pair was withheld for a possible gesture.

use super::replay::REPLAYED_INPUT_MARKER;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};

pub(super) fn replay_right_click() -> bool {
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
