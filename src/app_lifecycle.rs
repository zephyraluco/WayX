//! Rootless application presence, independent of native focus and process lifetime.
use std::time::{Duration, Instant};

pub fn is_application_window(id: u32, mapped: bool, flags: u32) -> bool {
    use crate::window::{MINIMIZED, OVERRIDE_REDIRECT, ROOTLESS};
    id != 1
        && flags & ROOTLESS != 0
        && flags & OVERRIDE_REDIRECT == 0
        && (mapped || flags & MINIMIZED != 0)
}

#[derive(Default)]
pub struct Presentation {
    visible: bool,
    hide_at: Option<Instant>,
}

impl Presentation {
    pub fn update(&mut self, has_windows: bool, now: Instant) -> Option<bool> {
        if has_windows {
            self.hide_at = None;
            if !self.visible {
                self.visible = true;
                return Some(true);
            }
        } else if self.visible {
            let deadline = *self.hide_at.get_or_insert(now + Duration::from_millis(200));
            if now >= deadline {
                self.visible = false;
                self.hide_at = None;
                return Some(false);
            }
        }
        None
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.hide_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::{MINIMIZED, OVERRIDE_REDIRECT, ROOTLESS};

    #[test]
    fn ignores_root_helpers_popups_and_withdrawn_windows_but_keeps_minimized_apps() {
        assert!(!is_application_window(1, true, ROOTLESS));
        assert!(!is_application_window(2, true, 0));
        assert!(!is_application_window(
            2,
            true,
            ROOTLESS | OVERRIDE_REDIRECT
        ));
        assert!(!is_application_window(2, false, ROOTLESS));
        assert!(is_application_window(2, true, ROOTLESS));
        assert!(is_application_window(2, false, ROOTLESS | MINIMIZED));
    }

    #[test]
    fn starts_hidden_and_hides_only_after_the_last_window_grace_period() {
        let mut state = Presentation::default();
        let now = Instant::now();
        assert_eq!(state.update(false, now), None);
        assert_eq!(state.deadline(), None);
        assert_eq!(state.update(true, now), Some(true));
        assert_eq!(state.update(true, now), None); // Another window does not toggle presence.
        assert_eq!(state.update(false, now), None);
        assert_eq!(state.deadline(), Some(now + Duration::from_millis(200)));
        assert_eq!(state.update(false, now + Duration::from_millis(199)), None);
        assert_eq!(
            state.update(false, now + Duration::from_millis(200)),
            Some(false)
        );
        assert_eq!(state.deadline(), None);
        assert_eq!(state.update(false, now + Duration::from_secs(1)), None);
        assert_eq!(state.update(true, now + Duration::from_secs(2)), Some(true));
    }

    #[test]
    fn a_replacement_window_cancels_pending_hide_without_flicker() {
        let mut state = Presentation::default();
        let now = Instant::now();
        assert_eq!(state.update(true, now), Some(true));
        assert_eq!(state.update(false, now), None);
        assert_eq!(state.update(true, now + Duration::from_millis(100)), None);
        assert_eq!(state.deadline(), None);
        assert_eq!(state.update(true, now + Duration::from_secs(1)), None);
    }
}
