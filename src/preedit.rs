//! Small opaque composition overlay for XIM clients without preedit callbacks.
//! System fonts are shaped with fallback; no fonts are bundled in the runtime.
use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};

pub(crate) struct PreeditImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
pub(crate) struct Painter {
    fonts: FontSystem,
    cache: SwashCache,
}
impl Painter {
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }
    pub fn paint(
        &mut self,
        text: &str,
        cursor: Option<(usize, usize)>,
        max_width: u32,
    ) -> PreeditImage {
        let limit = max_width.clamp(1, 1024);
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(16., 22.));
        buffer.set_size(
            &mut self.fonts,
            Some(limit.saturating_sub(8).max(1) as f32),
            Some(44.),
        );
        buffer.set_text(&mut self.fonts, text, &Attrs::new(), Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut width = 8;
        let mut height = 26;
        for run in buffer.layout_runs() {
            width = width.max((run.line_w.ceil() as u32).saturating_add(8));
            height = height.max((run.line_top + run.line_height).ceil() as u32 + 4);
        }
        let width = width.min(limit);
        let height = height.min(48);
        let mut pixels = vec![255u8; (width * height * 4) as usize];
        buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            Color::rgb(20, 20, 20),
            |x, y, w, h, color| {
                for py in y..y + h as i32 {
                    for px in x..x + w as i32 {
                        let (px, py) = (px + 4, py + 2);
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let offset = (py as usize * width as usize + px as usize) * 4;
                        let rgba = color.as_rgba();
                        for channel in 0..3 {
                            pixels[offset + channel] = ((rgba[channel] as u32 * rgba[3] as u32
                                + 255 * (255 - rgba[3] as u32))
                                / 255) as u8;
                        }
                    }
                }
            },
        );
        // XIM callback clients render their own feedback. Here show a clear
        // underline and the native cursor (UTF-8 byte offset, not scalar index).
        let caret = cursor.map_or(text.len(), |(_, end)| end.min(text.len()));
        let mut line_offset = 0;
        for run in buffer.layout_runs() {
            let y = (run.line_top + run.line_height).ceil() as u32;
            let end = (run.line_w.ceil() as u32 + 4).min(width);
            if y < height {
                for x in 4..end {
                    put(&mut pixels, width, x, y, [30, 70, 180, 255]);
                }
            }
            let offset = text.lines().nth(run.line_i).map_or(0, str::len);
            if run.line_i > 0 {
                line_offset = text
                    .split_inclusive('\n')
                    .take(run.line_i)
                    .map(str::len)
                    .sum();
            }
            let first = run.glyphs.first().map_or(0, |glyph| glyph.start);
            let last = run.glyphs.last().map_or(offset, |glyph| glyph.end);
            if cursor.is_some() && caret >= line_offset + first && caret <= line_offset + last {
                let mut x = 4.;
                for glyph in run.glyphs {
                    if glyph.end <= caret - line_offset {
                        x = glyph.x + glyph.w + 4.;
                    }
                }
                let x = (x as u32).min(width.saturating_sub(1));
                for py in run.line_top as u32 + 2..y.min(height) {
                    put(&mut pixels, width, x, py, [20, 20, 20, 255]);
                }
            }
        }
        // The display texture uses XRGB's little-endian BGRA byte order.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        PreeditImage {
            width,
            height,
            pixels,
        }
    }
}
fn put(pixels: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let offset = (y as usize * width as usize + x as usize) * 4;
    if let Some(pixel) = pixels.get_mut(offset..offset + 4) {
        pixel.copy_from_slice(&color);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_should_rasterize_system_font_glyphs_without_a_caret() {
        let mut painter = Painter::new();
        let image = painter.paint("A", None, 100);
        let content = &image.pixels[2 * image.width as usize * 4..16 * image.width as usize * 4];
        assert!(
            content
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != [255, 255, 255])
        );
    }
    #[test]
    fn overlay_should_clip_to_small_windows_and_draw_feedback() {
        let mut painter = Painter::new();
        for width in [1, 32, 1024, 4096] {
            let image = painter.paint("中文 日本語 한국어", Some((3, 3)), width);
            assert!(image.width <= width.min(1024));
            assert_eq!(
                image.pixels.len(),
                (image.width * image.height * 4) as usize
            );
            assert!(
                image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[3] == 255)
            );
            assert!(
                image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[..3] != [255, 255, 255])
            );
        }
    }
}
