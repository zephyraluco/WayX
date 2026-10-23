//! Portable window metadata and ICCCM size constraints shared with the C DDX.
use crate::backend::{Error, MAX_DIMENSION};

/// The DDX presents this window independently instead of scaling a root desktop.
pub const ROOTLESS: u32 = 1;
/// Undecorated popup which must not steal keyboard focus.
pub const OVERRIDE_REDIRECT: u32 = 2;
/// Keep this window above ordinary windows.
pub const ABOVE: u32 = 4;
/// Requested native minimization state.
pub const MINIMIZED: u32 = 8;
/// Requested native maximization state.
pub const MAXIMIZED: u32 = 16;
/// Requested borderless fullscreen state.
pub const FULLSCREEN: u32 = 32;

/// Fixed-width, versioned C-compatible ICCCM window metadata.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    /// Outer X11 window position in root coordinates.
    pub x: i32,
    /// Outer X11 window position in root coordinates.
    pub y: i32,
    /// Bitset of ROOTLESS, OVERRIDE_REDIRECT, ABOVE, MINIMIZED, MAXIMIZED, FULLSCREEN.
    pub flags: u32,
    /// X11 transient owner, or zero. This is not an ownership/destroy relationship.
    pub transient_for: u32,
    /// Minimum client width (zero means one pixel).
    pub min_width: u32,
    /// Minimum client height (zero means one pixel).
    pub min_height: u32,
    /// Maximum client width (zero means the framebuffer dimension limit).
    pub max_width: u32,
    /// Maximum client height (zero means the framebuffer dimension limit).
    pub max_height: u32,
    /// ICCCM base width for resize increments.
    pub base_width: u32,
    /// ICCCM base height for resize increments.
    pub base_height: u32,
    /// Width increment (zero means one pixel).
    pub width_inc: u32,
    /// Height increment (zero means one pixel).
    pub height_inc: u32,
    /// Minimum aspect numerator (zero disables this constraint).
    pub min_aspect_num: u32,
    /// Minimum aspect denominator.
    pub min_aspect_den: u32,
    /// Maximum aspect numerator (zero disables this constraint).
    pub max_aspect_num: u32,
    /// Maximum aspect denominator.
    pub max_aspect_den: u32,
}
/// One authoritative DDX observation, staged and committed as a single unit.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometrySnapshot {
    pub state: WindowState,
    pub width: u32,
    pub height: u32,
    pub move_ack: u32,
    pub resize_ack: u32,
}
impl WindowState {
    /// Validate untrusted window hints before they reach platform APIs.
    pub fn validate(&self) -> Result<(), Error> {
        let dimension_limit = MAX_DIMENSION;
        if self.flags & !(ROOTLESS | OVERRIDE_REDIRECT | ABOVE | MINIMIZED | MAXIMIZED | FULLSCREEN)
            != 0
            || [
                self.min_width,
                self.min_height,
                self.max_width,
                self.max_height,
                self.base_width,
                self.base_height,
                self.width_inc,
                self.height_inc,
            ]
            .iter()
            .any(|&v| v > dimension_limit)
            || (self.max_width != 0 && self.max_width < self.min_width)
            || (self.max_height != 0 && self.max_height < self.min_height)
            || (self.min_aspect_num != 0 && self.min_aspect_den == 0)
            || (self.max_aspect_num != 0 && self.max_aspect_den == 0)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
    /// Snap a client size to ICCCM bounds and resize increments.
    pub fn constrain(&self, width: u32, height: u32) -> (u32, u32) {
        fn snap(value: u32, min: u32, max: u32, base: u32, inc: u32) -> u32 {
            let min = min.max(1);
            let max = if max == 0 { MAX_DIMENSION } else { max }.max(min);
            let value = value.clamp(min, max);
            let inc = inc.max(1);
            let stepped = base.saturating_add(value.saturating_sub(base) / inc * inc);
            if stepped < min {
                base.saturating_add(min.saturating_sub(base).div_ceil(inc) * inc)
                    .min(max)
            } else {
                stepped.min(max)
            }
        }
        let mut width = snap(
            width,
            self.min_width,
            self.max_width,
            self.base_width,
            self.width_inc,
        );
        let mut height = snap(
            height,
            self.min_height,
            self.max_height,
            self.base_height,
            self.height_inc,
        );
        if self.min_aspect_num != 0
            && self.min_aspect_den != 0
            && u64::from(width) * u64::from(self.min_aspect_den)
                < u64::from(height) * u64::from(self.min_aspect_num)
        {
            let limit = (u64::from(width) * u64::from(self.min_aspect_den)
                / u64::from(self.min_aspect_num)) as u32;
            height = snap(
                limit,
                self.min_height,
                self.max_height,
                self.base_height,
                self.height_inc,
            );
        }
        if self.max_aspect_num != 0
            && self.max_aspect_den != 0
            && u64::from(width) * u64::from(self.max_aspect_den)
                > u64::from(height) * u64::from(self.max_aspect_num)
        {
            let limit = (u64::from(height) * u64::from(self.max_aspect_num)
                / u64::from(self.max_aspect_den)) as u32;
            width = snap(
                limit,
                self.min_width,
                self.max_width,
                self.base_width,
                self.width_inc,
            );
        }
        (width, height)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resize_should_respect_terminal_base_and_character_increments() {
        let hints = WindowState {
            min_width: 82,
            min_height: 44,
            base_width: 2,
            base_height: 4,
            width_inc: 8,
            height_inc: 16,
            max_width: 802,
            max_height: 644,
            ..Default::default()
        };
        assert_eq!(hints.constrain(103, 83), (98, 68));
        assert_eq!(hints.constrain(1, 1), (82, 52));
        assert_eq!(hints.constrain(u32::MAX, u32::MAX), (802, 644));
    }
    #[test]
    fn hints_should_reject_zero_denominator_and_inverted_bounds() {
        assert!(
            WindowState {
                min_width: 100,
                max_width: 50,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            WindowState {
                min_aspect_num: 16,
                min_aspect_den: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            WindowState {
                min_aspect_num: 16,
                min_aspect_den: 9,
                max_aspect_num: 16,
                max_aspect_den: 9,
                ..Default::default()
            }
            .constrain(160, 100),
            (160, 90)
        );
    }
}
