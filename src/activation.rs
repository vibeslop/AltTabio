//! Which window takes activation when the user picks a task.

/// The owned popup the user last worked in, such as a modal dialog, takes activation in place of
/// its owner. `W::default()` is the null window handle.
#[must_use]
pub fn activation_target<W: Copy + Default + PartialEq>(
    owner: W,
    popup: W,
    popup_is_visible: bool,
) -> W {
    if popup != W::default() && popup != owner && popup_is_visible {
        popup
    } else {
        owner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_targets_the_visible_last_active_owned_popup() {
        let owner = 100_isize;
        let popup = 200_isize;

        assert_eq!(activation_target(owner, popup, true), popup);
    }

    #[test]
    fn activation_keeps_the_owner_for_an_unusable_popup() {
        let owner = 100_isize;
        let popup = 200_isize;

        assert_eq!(activation_target(owner, popup, false), owner);
        assert_eq!(activation_target(owner, 0, true), owner);
        assert_eq!(activation_target(owner, owner, true), owner);
    }
}
