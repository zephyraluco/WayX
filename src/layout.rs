//! Physical host desktop layout. Only the DDX translates host and X coordinates.
use crate::backend::{Error, MAX_DIMENSION};
pub const MAX_MONITORS: usize = 16;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DesktopLayout {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub monitors: [MonitorRect; MAX_MONITORS],
}
impl DesktopLayout {
    pub fn new(mut monitors: Vec<MonitorRect>) -> Result<Self, Error> {
        monitors.sort();
        if monitors.is_empty() || monitors.len() > MAX_MONITORS {
            return Err(Error::InvalidSize);
        }
        let origin_x = monitors
            .iter()
            .map(|m| m.x)
            .min()
            .ok_or(Error::InvalidSize)?;
        let origin_y = monitors
            .iter()
            .map(|m| m.y)
            .min()
            .ok_or(Error::InvalidSize)?;
        let right = monitors
            .iter()
            .map(|m| i64::from(m.x) + i64::from(m.width))
            .max()
            .ok_or(Error::InvalidSize)?;
        let bottom = monitors
            .iter()
            .map(|m| i64::from(m.y) + i64::from(m.height))
            .max()
            .ok_or(Error::InvalidSize)?;
        let mut layout = Self {
            origin_x,
            origin_y,
            width: u32::try_from(right - i64::from(origin_x)).map_err(|_| Error::InvalidSize)?,
            height: u32::try_from(bottom - i64::from(origin_y)).map_err(|_| Error::InvalidSize)?,
            count: monitors.len() as u32,
            ..Default::default()
        };
        layout.monitors[..monitors.len()].copy_from_slice(&monitors);
        layout.validate()?;
        Ok(layout)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.count == 0
            || self.count as usize > MAX_MONITORS
            || self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
            || u64::from(self.width) * u64::from(self.height) * 4 > 256 * 1024 * 1024
        {
            return Err(Error::InvalidSize);
        }
        for monitor in &self.monitors[..self.count as usize] {
            let x = i64::from(monitor.x) - i64::from(self.origin_x);
            let y = i64::from(monitor.y) - i64::from(self.origin_y);
            if monitor.width == 0
                || monitor.height == 0
                || x < 0
                || y < 0
                || x + i64::from(monitor.width) > i64::from(self.width)
                || y + i64::from(monitor.height) > i64::from(self.height)
            {
                return Err(Error::InvalidSize);
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_monitors_share_one_physical_origin_and_normalized_extent() {
        let layout = DesktopLayout::new(vec![
            MonitorRect {
                x: -1920,
                y: -200,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ])
        .unwrap();
        assert_eq!(
            (
                layout.origin_x,
                layout.origin_y,
                layout.width,
                layout.height
            ),
            (-1920, -200, 4480, 1640)
        );
        assert_eq!(-8 - layout.origin_x, 1912);
        let mut invalid = layout;
        invalid.width = 1280;
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn mirrored_monitors_preserve_distinct_outputs() {
        let monitor = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let layout = DesktopLayout::new(vec![monitor, monitor]).unwrap();
        assert_eq!(layout.count, 2);
        assert_eq!((layout.width, layout.height), (1920, 1080));
    }
    #[test]
    fn invalid_and_overflowing_layouts_are_rejected_before_allocation() {
        assert!(DesktopLayout::new(vec![]).is_err());
        assert!(
            DesktopLayout::new(vec![MonitorRect {
                width: 16384,
                height: 16384,
                ..Default::default()
            }])
            .is_err()
        );
        assert!(
            DesktopLayout::new(vec![
                MonitorRect {
                    x: i32::MIN,
                    width: 1,
                    height: 1,
                    y: 0
                },
                MonitorRect {
                    x: i32::MAX,
                    width: 1,
                    height: 1,
                    y: 0
                }
            ])
            .is_err()
        );
    }
}
