//! X11 pixels <-> native physical pixels. No remote DPI or toolkit settings.
use crate::layout::{DesktopLayout, MonitorRect};
use std::{str::FromStr, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ScalePolicy {
    #[default]
    Auto,
    Fixed(f64),
}
impl FromStr for ScalePolicy {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "auto" {
            return Ok(Self::Auto);
        }
        match value.parse::<f64>() {
            Ok(value) if value.is_finite() && (0.5..=4.).contains(&value) => Ok(Self::Fixed(value)),
            _ => Err("scale must be auto or a finite number between 0.5 and 4".into()),
        }
    }
}
impl ScalePolicy {
    pub fn resolve(self, native: f64) -> f64 {
        match self {
            Self::Fixed(value) => value,
            Self::Auto if native.is_finite() && native > 0. => native,
            _ => 1.,
        }
    }
}

pub fn physical_size(size: (u32, u32), factor: f64) -> (u32, u32) {
    let dimension = |value| {
        (f64::from(value) * factor)
            .round()
            .clamp(1., u32::MAX as f64) as u32
    };
    (dimension(size.0), dimension(size.1))
}
pub fn remote_size(size: (u32, u32), factor: f64) -> (u32, u32) {
    physical_size(size, 1. / factor)
}

#[derive(Clone, Debug, PartialEq)]
struct Screen {
    physical: MonitorRect,
    remote: MonitorRect,
    factor: f64,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenMap {
    screens: Vec<Screen>,
}
fn distance(rect: MonitorRect, point: (i32, i32)) -> i64 {
    let x = i64::from(point.0);
    let y = i64::from(point.1);
    let dx = (i64::from(rect.x) - x)
        .max(0)
        .max(x - (i64::from(rect.x) + i64::from(rect.width) - 1));
    let dy = (i64::from(rect.y) - y)
        .max(0)
        .max(y - (i64::from(rect.y) + i64::from(rect.height) - 1));
    dx + dy
}
fn translate(point: (i32, i32), from: MonitorRect, to: MonitorRect, factor: f64) -> (i32, i32) {
    let axis = |value: i32, origin: i32, target: i32| {
        (f64::from(target) + (f64::from(value) - f64::from(origin)) * factor)
            .round()
            .clamp(i32::MIN as f64, i32::MAX as f64) as i32
    };
    (axis(point.0, from.x, to.x), axis(point.1, from.y, to.y))
}
impl ScreenMap {
    /// Preserve physical neighbour ordering, packing scaled outputs without overlap.
    /// Mixed-DPI desktops may have logical gaps; each output has an invertible local map.
    pub fn new(
        mut monitors: Vec<(MonitorRect, f64)>,
        policy: ScalePolicy,
    ) -> anyhow::Result<Arc<Self>> {
        monitors.sort_by_key(|(rect, _)| (distance(*rect, (0, 0)), rect.x, rect.y));
        let mut screens: Vec<Screen> = Vec::new();
        for (physical, native_factor) in monitors {
            let factor = policy.resolve(native_factor);
            let (width, height) = remote_size((physical.width, physical.height), factor);
            let mut remote = MonitorRect {
                x: (physical.x as f64 / factor).round() as i32,
                y: (physical.y as f64 / factor).round() as i32,
                width,
                height,
            };
            if let Some(anchor) = screens
                .iter()
                .min_by_key(|s| distance(s.physical, (physical.x, physical.y)))
            {
                if anchor.physical == physical {
                    remote = anchor.remote;
                } else {
                    let dx = i64::from(physical.x) - i64::from(anchor.physical.x);
                    let dy = i64::from(physical.y) - i64::from(anchor.physical.y);
                    remote.x = anchor
                        .remote
                        .x
                        .saturating_add((dx as f64 / anchor.factor).round() as i32);
                    remote.y = anchor
                        .remote
                        .y
                        .saturating_add((dy as f64 / anchor.factor).round() as i32);
                    if dx >= i64::from(anchor.physical.width) {
                        remote.x = anchor
                            .remote
                            .x
                            .saturating_add(anchor.remote.width as i32)
                            .saturating_add(
                                ((dx - i64::from(anchor.physical.width)) as f64 / anchor.factor)
                                    .round() as i32,
                            );
                    } else if -dx >= i64::from(physical.width) {
                        remote.x = anchor.remote.x.saturating_sub(width as i32).saturating_sub(
                            ((-dx - i64::from(physical.width)) as f64 / factor).round() as i32,
                        );
                    } else if dy >= i64::from(anchor.physical.height) {
                        remote.y = anchor
                            .remote
                            .y
                            .saturating_add(anchor.remote.height as i32)
                            .saturating_add(
                                ((dy - i64::from(anchor.physical.height)) as f64 / anchor.factor)
                                    .round() as i32,
                            );
                    } else if -dy >= i64::from(physical.height) {
                        remote.y = anchor
                            .remote
                            .y
                            .saturating_sub(height as i32)
                            .saturating_sub(
                                ((-dy - i64::from(physical.height)) as f64 / factor).round() as i32,
                            );
                    }
                    // Resolve gaps/offsets that would otherwise overlap after mixed scaling.
                    for _ in 0..screens.len() {
                        let overlap = screens.iter().find(|s| {
                            i64::from(remote.x) < i64::from(s.remote.x) + i64::from(s.remote.width)
                                && i64::from(s.remote.x)
                                    < i64::from(remote.x) + i64::from(remote.width)
                                && i64::from(remote.y)
                                    < i64::from(s.remote.y) + i64::from(s.remote.height)
                                && i64::from(s.remote.y)
                                    < i64::from(remote.y) + i64::from(remote.height)
                        });
                        let Some(other) = overlap else { break };
                        if dx.abs() >= dy.abs() {
                            remote.x = if dx >= 0 {
                                other.remote.x.saturating_add(other.remote.width as i32)
                            } else {
                                other.remote.x.saturating_sub(width as i32)
                            };
                        } else {
                            remote.y = if dy >= 0 {
                                other.remote.y.saturating_add(other.remote.height as i32)
                            } else {
                                other.remote.y.saturating_sub(height as i32)
                            };
                        }
                    }
                }
            }
            screens.push(Screen {
                physical,
                remote,
                factor,
            });
        }
        let map = Arc::new(Self { screens });
        map.layout()?;
        Ok(map)
    }
    pub fn layout(&self) -> Result<DesktopLayout, crate::backend::Error> {
        DesktopLayout::new(self.screens.iter().map(|s| s.remote).collect())
    }
    pub fn to_physical(&self, point: (i32, i32)) -> (i32, i32) {
        self.screens
            .iter()
            .min_by_key(|s| distance(s.remote, point))
            .map_or(point, |s| translate(point, s.remote, s.physical, s.factor))
    }
    pub fn to_remote(&self, point: (i32, i32)) -> (i32, i32) {
        self.screens
            .iter()
            .min_by_key(|s| distance(s.physical, point))
            .map_or(point, |s| {
                translate(point, s.physical, s.remote, 1. / s.factor)
            })
    }
    pub fn factor_at(&self, point: (i32, i32)) -> f64 {
        self.screens
            .iter()
            .min_by_key(|s| distance(s.remote, point))
            .map_or(1., |s| s.factor)
    }
}

#[derive(Default)]
pub struct DisplayOptions {
    pub frame_limit: Option<u32>,
    pub scale: ScalePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_the_single_scale_option() {
        assert_eq!("auto".parse(), Ok(ScalePolicy::Auto));
        for value in ["0.5", "1", "1.25", "1.5", "2", "4"] {
            assert!(value.parse::<ScalePolicy>().is_ok());
        }
        for value in ["0", "-1", "NaN", "inf", "4.1", "", "fit"] {
            assert!(value.parse::<ScalePolicy>().is_err());
        }
    }
    #[test]
    fn scales_sizes_without_changing_source_pixels() {
        assert_eq!(physical_size((1920, 1080), 2.), (3840, 2160));
        for factor in [0.5, 1., 1.25, 1.5, 2., 4.] {
            let size = physical_size((800, 600), factor);
            assert_eq!(remote_size(size, factor), (800, 600));
        }
    }
    #[test]
    fn maps_mixed_dpi_and_negative_displays_both_ways() {
        let map = ScreenMap::new(
            vec![
                (
                    MonitorRect {
                        x: -1920,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    1.,
                ),
                (
                    MonitorRect {
                        x: 0,
                        y: 0,
                        width: 3840,
                        height: 2160,
                    },
                    2.,
                ),
            ],
            ScalePolicy::Auto,
        )
        .unwrap();
        assert_eq!(
            (map.layout().unwrap().width, map.layout().unwrap().height),
            (3840, 1080)
        );
        for point in [(-1800, 100), (-1, 300), (100, 100), (1500, 900)] {
            assert_eq!(map.to_remote(map.to_physical(point)), point);
        }
        assert_eq!(map.to_physical((100, 100)), (200, 200));
        assert_eq!(map.factor_at((-100, 100)), 1.);
        assert_eq!(map.factor_at((100, 100)), 2.);
    }
    #[test]
    fn fixed_one_preserves_existing_monitor_coordinates() {
        let rects = vec![
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 1920,
                y: -100,
                width: 3840,
                height: 2160,
            },
        ];
        let map = ScreenMap::new(
            rects.iter().map(|r| (*r, 2.)).collect(),
            ScalePolicy::Fixed(1.),
        )
        .unwrap();
        assert_eq!(map.layout().unwrap(), DesktopLayout::new(rects).unwrap());
    }
}
