use std::collections::{BTreeMap, VecDeque};

pub const MAX_DIMENSION: u32 = 16384;
pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_TOTAL_FRAME_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_EVENTS: usize = 4096;
pub const MAX_WINDOWS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSize,
    InvalidBuffer,
    UnknownWindow,
    DuplicateWindow,
    InvalidParent,
    QueueFull,
    InvalidText,
    InvalidInput,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Debug)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA8, alpha always opaque for XRGB input.
    pub pixels: Vec<u8>,
    pub revision: u64,
    pub damage: Option<Rect>,
}
impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        let len = frame_len(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            revision: 1,
            damage: Some(Rect {
                x: 0,
                y: 0,
                width,
                height,
            }),
        })
    }
    /// Source contains only the damage rectangle, with an explicit row stride.
    pub fn upload(&mut self, rect: Rect, stride: usize, bytes: &[u8]) -> Result<(), Error> {
        if rect.width == 0 || rect.height == 0 {
            return Ok(());
        }
        if rect
            .x
            .checked_add(rect.width)
            .is_none_or(|x| x > self.width)
            || rect
                .y
                .checked_add(rect.height)
                .is_none_or(|y| y > self.height)
        {
            return Err(Error::InvalidSize);
        }
        let row = (rect.width as usize)
            .checked_mul(4)
            .ok_or(Error::InvalidBuffer)?;
        let needed = stride
            .checked_mul(rect.height as usize - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::InvalidBuffer)?;
        if stride < row || bytes.len() < needed {
            return Err(Error::InvalidBuffer);
        }
        for y in 0..rect.height as usize {
            let dst = ((rect.y as usize + y) * self.width as usize + rect.x as usize) * 4;
            self.pixels[dst..dst + row].copy_from_slice(&bytes[y * stride..y * stride + row]);
            for pixel in self.pixels[dst..dst + row].as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        self.revision = self.revision.wrapping_add(1);
        self.damage = Some(match self.damage {
            None => rect,
            Some(old) => {
                let x = old.x.min(rect.x);
                let y = old.y.min(rect.y);
                Rect {
                    x,
                    y,
                    width: (old.x + old.width).max(rect.x + rect.width) - x,
                    height: (old.y + old.height).max(rect.y + rect.height) - y,
                }
            }
        });
        Ok(())
    }
}
fn frame_len(width: u32, height: u32) -> Result<usize, Error> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(Error::InvalidSize);
    }
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(Error::InvalidSize)?;
    if len > MAX_FRAME_BYTES {
        return Err(Error::InvalidSize);
    }
    Ok(len)
}

#[derive(Debug)]
pub struct Window {
    pub id: u32,
    pub parent: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub mapped: bool,
    pub framebuffer: Framebuffer,
}

/// Stable wire events. Keys carry physical USB HID usage codes; DDX maps to X keycodes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputEvent {
    pub kind: u32,
    pub window: u32,
    pub detail: u32,
    pub flags: u32,
    pub x: f64,
    pub y: f64,
    pub text: [u8; 32],
}
pub mod event {
    pub const MOTION: u32 = 1;
    pub const BUTTON: u32 = 2;
    pub const KEY: u32 = 3;
    pub const SCROLL: u32 = 4;
    pub const FOCUS: u32 = 5;
    pub const CLOSE: u32 = 6;
    pub const RESIZE: u32 = 7;
    pub const TEXT: u32 = 8;
    pub const CLIPBOARD: u32 = 9;
    pub const SHUTDOWN: u32 = 10;
}

#[derive(Default)]
pub struct Backend {
    pub windows: BTreeMap<u32, Window>,
    pub focus: Option<u32>,
    pub clipboard: String,
    pub clipboard_revision: u64,
    events: VecDeque<InputEvent>,
}
impl Backend {
    pub fn create(&mut self, id: u32, parent: u32, width: u32, height: u32) -> Result<(), Error> {
        if id == 0 || self.windows.contains_key(&id) {
            return Err(Error::DuplicateWindow);
        }
        if parent != 0 && !self.windows.contains_key(&parent) {
            return Err(Error::InvalidParent);
        }
        if self.windows.len() >= MAX_WINDOWS {
            return Err(Error::InvalidSize);
        }
        let len = frame_len(width, height)?;
        if self
            .windows
            .values()
            .map(|w| w.framebuffer.pixels.len())
            .sum::<usize>()
            + len
            > MAX_TOTAL_FRAME_BYTES
        {
            return Err(Error::InvalidSize);
        }
        self.windows.insert(
            id,
            Window {
                id,
                parent,
                title: format!("WayX — {id:#x}"),
                x: 0,
                y: 0,
                mapped: true,
                framebuffer: Framebuffer::new(width, height)?,
            },
        );
        Ok(())
    }
    pub fn resize(&mut self, id: u32, width: u32, height: u32) -> Result<(), Error> {
        let old = self
            .windows
            .get(&id)
            .ok_or(Error::UnknownWindow)?
            .framebuffer
            .pixels
            .len();
        let len = frame_len(width, height)?;
        if self
            .windows
            .values()
            .map(|w| w.framebuffer.pixels.len())
            .sum::<usize>()
            - old
            + len
            > MAX_TOTAL_FRAME_BYTES
        {
            return Err(Error::InvalidSize);
        }
        self.windows.get_mut(&id).unwrap().framebuffer = Framebuffer::new(width, height)?;
        Ok(())
    }
    pub fn destroy(&mut self, id: u32) -> Result<(), Error> {
        if !self.windows.contains_key(&id) {
            return Err(Error::UnknownWindow);
        }
        let children: Vec<_> = self
            .windows
            .values()
            .filter(|w| w.parent == id)
            .map(|w| w.id)
            .collect();
        for child in children {
            self.destroy(child)?;
        }
        self.windows.remove(&id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        self.events.retain(|e| e.window != id);
        Ok(())
    }
    pub fn push_input(&mut self, input: InputEvent) -> Result<(), Error> {
        if !(event::MOTION..=event::SHUTDOWN).contains(&input.kind)
            || !input.x.is_finite()
            || !input.y.is_finite()
        {
            return Err(Error::InvalidInput);
        }
        if input.kind == event::TEXT
            && (input.detail > 32
                || std::str::from_utf8(&input.text[..input.detail as usize]).is_err())
        {
            return Err(Error::InvalidInput);
        }
        if input.window != 0 && !self.windows.contains_key(&input.window) {
            return Err(Error::UnknownWindow);
        }
        // Coalesce only adjacent motion, preserving key/button ordering.
        if input.kind == event::MOTION
            && let Some(last) = self.events.back_mut()
            && last.kind == event::MOTION
            && last.window == input.window
        {
            *last = input;
            return Ok(());
        }
        if self.events.len() == MAX_EVENTS {
            return Err(Error::QueueFull);
        }
        if input.kind == event::FOCUS {
            self.focus = (input.flags != 0).then_some(input.window);
        }
        self.events.push_back(input);
        Ok(())
    }
    pub fn poll_input(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }
    pub fn set_clipboard(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > 16 * 1024 * 1024 || text.contains('\0') {
            return Err(Error::InvalidText);
        }
        if self.clipboard != text {
            self.clipboard = text.to_owned();
            self.clipboard_revision = self.clipboard_revision.wrapping_add(1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_nonfinite_input_and_excess_window_count() {
        let mut b = Backend::default();
        assert_eq!(
            b.push_input(InputEvent {
                kind: event::MOTION,
                x: f64::NAN,
                ..Default::default()
            }),
            Err(Error::InvalidInput)
        );
        assert_eq!(
            b.push_input(InputEvent {
                kind: event::TEXT,
                detail: 33,
                ..Default::default()
            }),
            Err(Error::InvalidInput)
        );
        for id in 1..=MAX_WINDOWS as u32 {
            b.create(id, 0, 1, 1).unwrap();
        }
        assert_eq!(
            b.create(MAX_WINDOWS as u32 + 1, 0, 1, 1),
            Err(Error::InvalidSize)
        );
    }
    #[test]
    fn padded_damage_and_opaque_xrgb() {
        let mut fb = Framebuffer::new(4, 3).unwrap();
        fb.damage = None;
        let bytes = [1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 0];
        fb.upload(
            Rect {
                x: 2,
                y: 1,
                width: 1,
                height: 2,
            },
            8,
            &bytes,
        )
        .unwrap();
        assert_eq!(&fb.pixels[24..28], &[1, 2, 3, 255]);
        assert_eq!(&fb.pixels[40..44], &[4, 5, 6, 255]);
        assert_eq!(
            fb.damage,
            Some(Rect {
                x: 2,
                y: 1,
                width: 1,
                height: 2
            })
        );
    }
    #[test]
    fn rejects_overflow_and_short_buffer_without_mutation() {
        assert!(Framebuffer::new(u32::MAX, 2).is_err());
        let mut fb = Framebuffer::new(2, 2).unwrap();
        let before = fb.pixels.clone();
        assert!(
            fb.upload(
                Rect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 1
                },
                8,
                &[0; 8]
            )
            .is_err()
        );
        assert!(
            fb.upload(
                Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2
                },
                8,
                &[0; 8]
            )
            .is_err()
        );
        assert_eq!(fb.pixels, before);
    }
    #[test]
    fn parent_lifetime_and_focus() {
        let mut b = Backend::default();
        b.create(1, 0, 2, 2).unwrap();
        b.create(2, 1, 2, 2).unwrap();
        assert_eq!(b.create(2, 0, 2, 2), Err(Error::DuplicateWindow));
        b.push_input(InputEvent {
            kind: event::FOCUS,
            window: 2,
            flags: 1,
            ..Default::default()
        })
        .unwrap();
        b.destroy(1).unwrap();
        assert!(b.windows.is_empty());
        assert_eq!(b.focus, None);
        assert!(b.poll_input().is_none());
    }
    #[test]
    fn coalescing_preserves_button_order_and_backpressure() {
        let mut b = Backend::default();
        b.create(1, 0, 2, 2).unwrap();
        let motion = InputEvent {
            kind: event::MOTION,
            window: 1,
            ..Default::default()
        };
        b.push_input(motion).unwrap();
        b.push_input(InputEvent { x: 42., ..motion }).unwrap();
        b.push_input(InputEvent {
            kind: event::BUTTON,
            window: 1,
            ..Default::default()
        })
        .unwrap();
        b.push_input(motion).unwrap();
        assert_eq!(b.poll_input().unwrap().x, 42.);
        assert_eq!(b.poll_input().unwrap().kind, event::BUTTON);
        assert_eq!(b.poll_input().unwrap().kind, event::MOTION);
        for _ in 0..MAX_EVENTS {
            b.push_input(InputEvent {
                kind: event::KEY,
                window: 1,
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(b.push_input(motion), Err(Error::QueueFull));
    }
    #[test]
    fn clipboard_changes_are_versioned() {
        let mut b = Backend::default();
        b.set_clipboard("你好").unwrap();
        let version = b.clipboard_revision;
        b.set_clipboard("你好").unwrap();
        assert_eq!(b.clipboard_revision, version);
        assert_eq!(b.set_clipboard("bad\0text"), Err(Error::InvalidText));
    }
}
