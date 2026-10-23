//! Bounded upload regions. Preserve sparse updates without unbounded GPU calls.
use crate::backend::Rect;

const MAX_RECTS: usize = 32;
// Merge a small gap when its extra pixels cost less than another upload call.
const CALL_PIXELS: u64 = 16;
#[derive(Debug, Default)]
pub struct Damage {
    rects: Vec<Rect>,
}
fn area(rect: Rect) -> u64 {
    u64::from(rect.width) * u64::from(rect.height)
}
fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}
impl Damage {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            rects: vec![Rect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        }
    }
    pub fn clear(&mut self) {
        self.rects.clear();
    }
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }
    /// Callers validate rectangles against framebuffer dimensions first.
    pub(crate) fn add(&mut self, mut rect: Rect, width: u32, height: u32) {
        let mut index = 0;
        while index < self.rects.len() {
            let old = self.rects[index];
            let merged = union(old, rect);
            if area(merged) <= area(old) + area(rect) + CALL_PIXELS {
                self.rects.swap_remove(index);
                rect = merged;
                index = 0;
            } else {
                index += 1;
            }
        }
        self.rects.push(rect);
        let pixels = self.rects.iter().copied().map(area).sum::<u64>();
        let full = u64::from(width) * u64::from(height);
        if self.rects.len() > MAX_RECTS
            || pixels * 4 >= full * 3
            || pixels + self.rects.len() as u64 * CALL_PIXELS >= full + CALL_PIXELS
        {
            self.rects.clear();
            self.rects.push(Rect {
                x: 0,
                y: 0,
                width,
                height,
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distant_corners_do_not_amplify_into_a_full_frame() {
        let mut damage = Damage::default();
        for (x, y) in [(0, 0), (1900, 1000)] {
            damage.add(
                Rect {
                    x,
                    y,
                    width: 10,
                    height: 10,
                },
                1920,
                1080,
            );
        }
        assert_eq!(damage.rects().len(), 2);
        assert_eq!(damage.rects().iter().copied().map(area).sum::<u64>(), 200);
    }
    #[test]
    fn overlapping_strips_merge_and_excess_fragments_are_bounded() {
        let mut damage = Damage::default();
        damage.add(
            Rect {
                x: 10,
                y: 10,
                width: 10,
                height: 10,
            },
            2000,
            2000,
        );
        damage.add(
            Rect {
                x: 15,
                y: 10,
                width: 10,
                height: 10,
            },
            2000,
            2000,
        );
        assert_eq!(
            damage.rects(),
            &[Rect {
                x: 10,
                y: 10,
                width: 15,
                height: 10
            }]
        );
        for index in 0..100 {
            damage.add(
                Rect {
                    x: index * 15,
                    y: index * 15,
                    width: 1,
                    height: 1,
                },
                2000,
                2000,
            );
            assert!(damage.rects().len() <= MAX_RECTS);
        }
        assert_eq!(
            damage.rects(),
            &[Rect {
                x: 0,
                y: 0,
                width: 2000,
                height: 2000
            }]
        );
    }
    #[test]
    fn dense_updates_use_one_full_upload() {
        let mut damage = Damage::default();
        damage.add(
            Rect {
                x: 0,
                y: 0,
                width: 90,
                height: 90,
            },
            100,
            100,
        );
        assert_eq!(
            damage.rects(),
            &[Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100
            }]
        );
    }
}
