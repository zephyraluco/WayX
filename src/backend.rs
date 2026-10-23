use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

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
    /// Tightly packed little-endian BGRX; the unused X byte is preserved.
    pub pixels: Vec<u8>,
    pub revision: u64,
    pub damage: crate::damage::Damage,
}
impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        let len = frame_len(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            revision: 1,
            damage: crate::damage::Damage::full(width, height),
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
        }
        self.revision = self.revision.wrapping_add(1).max(1);
        self.damage.add(rect, self.width, self.height);
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
    pub incarnation: u64,
    pub id: u32,
    pub parent: u32,
    pub title: String,
    pub mapped: bool,
    pub geometry: crate::window::GeometrySnapshot,
    pub geometry_generation: u64,
    pub framebuffer: Framebuffer,
}

/// Immutable metadata published only by output_flush. Pixel storage has its own
/// version and may advance while the UI still displays the previous geometry.
#[derive(Clone, Debug)]
pub struct WindowSnapshot {
    pub incarnation: u64,
    pub mapped: bool,
    pub title: Arc<str>,
    pub geometry: crate::window::GeometrySnapshot,
    pub generation: u64,
    pub pixel_size: (u32, u32),
    pub pixel_revision: u64,
}

/// Stable wire events. Keys carry physical USB HID usage codes; DDX maps to X keycodes.
/// MOVE carries physical host desktop coordinates; MOTION carries window-local pixels.
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
    pub const MOVE: u32 = 11;
    pub const STATE: u32 = 12;
    pub const DESKTOP: u32 = 13;
}

#[derive(Default)]
pub struct Backend {
    /// DDX working state; UI lifecycle decisions use published_windows instead.
    pub windows: BTreeMap<u32, Window>,
    pub published_windows: BTreeMap<u32, Arc<WindowSnapshot>>,
    dirty_windows: BTreeSet<u32>,
    next_incarnation: u64,
    pub focus: Option<u32>,
    pub requested_focus: Option<u32>,
    pub clipboard: String,
    pub clipboard_revision: u64,
    events: VecDeque<InputEvent>,
    pub(crate) ui_wake: Option<crate::notify::UiWake>,
    pub(crate) pending_geometry: BTreeMap<u32, crate::window::GeometrySnapshot>,
    pub(crate) pending_maps: BTreeMap<u32, bool>,
    pub desktop_layout: Option<crate::layout::DesktopLayout>,
    pub worker_exited: bool,
    pub(crate) input_wake: Option<crate::notify::InputWake>,
}
impl Backend {
    pub(crate) fn changed(&mut self, id: u32) {
        self.dirty_windows.insert(id);
    }
    pub fn notify_ui(&self) {
        if let Some(wake) = &self.ui_wake {
            wake();
        }
    }
    pub fn flush_output(&mut self) {
        for (id, geometry) in std::mem::take(&mut self.pending_geometry) {
            if let Some(window) = self.windows.get_mut(&id)
                && window.geometry != geometry
            {
                window.geometry = geometry;
                window.geometry_generation = window.geometry_generation.wrapping_add(1).max(1);
                self.dirty_windows.insert(id);
            }
        }
        // A newly mapped window must not become visible before its first
        // committed geometry. The whole publication holds the backend lock.
        for (id, mapped) in std::mem::take(&mut self.pending_maps) {
            if let Some(window) = self.windows.get_mut(&id)
                && window.mapped != mapped
            {
                window.mapped = mapped;
                self.dirty_windows.insert(id);
            }
        }
        let changed = !self.dirty_windows.is_empty();
        for id in std::mem::take(&mut self.dirty_windows) {
            if let Some(window) = self.windows.get(&id) {
                let title = self
                    .published_windows
                    .get(&id)
                    .filter(|old| old.title.as_ref() == window.title)
                    .map_or_else(|| Arc::from(window.title.as_str()), |old| old.title.clone());
                self.published_windows.insert(
                    id,
                    Arc::new(WindowSnapshot {
                        incarnation: window.incarnation,
                        mapped: window.mapped,
                        title,
                        geometry: window.geometry,
                        generation: window.geometry_generation,
                        pixel_size: (window.framebuffer.width, window.framebuffer.height),
                        pixel_revision: window.framebuffer.revision,
                    }),
                );
            } else {
                self.published_windows.remove(&id);
            }
        }
        if changed {
            self.notify_ui();
        }
    }

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
        self.next_incarnation = self.next_incarnation.wrapping_add(1).max(1);
        self.windows.insert(
            id,
            Window {
                incarnation: self.next_incarnation,
                id,
                parent,
                title: format!("WayX — {id:#x}"),
                mapped: false,
                geometry: crate::window::GeometrySnapshot {
                    width,
                    height,
                    ..Default::default()
                },
                geometry_generation: 0,
                framebuffer: Framebuffer::new(width, height)?,
            },
        );
        self.changed(id);
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
        let window = self.windows.get_mut(&id).unwrap();
        let revision = window.framebuffer.revision.wrapping_add(1).max(1);
        window.framebuffer = Framebuffer::new(width, height)?;
        window.framebuffer.revision = revision;
        self.changed(id);
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
        self.changed(id);
        self.pending_geometry.remove(&id);
        self.pending_maps.remove(&id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        if self.requested_focus == Some(id) {
            self.requested_focus = None;
        }
        self.events.retain(|e| e.window != id);
        Ok(())
    }
    pub fn push_input(&mut self, input: InputEvent) -> Result<(), Error> {
        if !(event::MOTION..=event::DESKTOP).contains(&input.kind)
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
        // Coalesce only adjacent moves of the same kind and window. Never cross
        // key/button, resize, state, or lifecycle ordering boundaries.
        if matches!(input.kind, event::MOTION | event::MOVE)
            && let Some(last) = self.events.back_mut()
            && last.kind == input.kind
            && last.window == input.window
        {
            *last = input;
            return Ok(());
        }
        if self.events.len() == MAX_EVENTS {
            return Err(Error::QueueFull);
        }
        if input.kind == event::FOCUS {
            if input.flags != 0 {
                self.focus = Some(input.window);
            } else if self.focus == Some(input.window) {
                self.focus = None;
            }
        }
        self.events.push_back(input);
        if let Some(wake) = &mut self.input_wake {
            // A failed wake must never silently strand input in a sleeping server.
            if let Err(error) = wake.signal() {
                tracing::error!(%error, "input wake failed");
                return Err(Error::InvalidInput);
            }
        }
        Ok(())
    }
    pub fn poll_input(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }
    pub(crate) fn rearm_input(&mut self) -> Result<(), ()> {
        if !self.events.is_empty()
            && let Some(wake) = &mut self.input_wake
        {
            wake.signal().map_err(|_| ())?;
        }
        Ok(())
    }
    pub fn set_clipboard(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > 16 * 1024 * 1024 || text.contains('\0') {
            return Err(Error::InvalidText);
        }
        if self.clipboard != text {
            self.clipboard = text.to_owned();
            self.clipboard_revision = self.clipboard_revision.wrapping_add(1);
            self.notify_ui();
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
    fn resize_back_to_original_size_has_a_new_pixel_version() {
        let mut backend = Backend::default();
        backend.create(1, 0, 2, 2).unwrap();
        let original = backend.windows[&1].framebuffer.revision;
        backend.resize(1, 3, 3).unwrap();
        backend.resize(1, 2, 2).unwrap();
        assert_ne!(backend.windows[&1].framebuffer.revision, original);
    }
    #[test]
    fn padded_damage_preserves_xrgb_bytes() {
        let mut fb = Framebuffer::new(4, 3).unwrap();
        fb.damage.clear();
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
        assert_eq!(&fb.pixels[24..28], &[1, 2, 3, 0]);
        assert_eq!(&fb.pixels[40..44], &[4, 5, 6, 0]);
        assert_eq!(
            fb.damage.rects(),
            &[Rect {
                x: 2,
                y: 1,
                width: 1,
                height: 2
            }]
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
    fn drag_bursts_coalesce_without_crossing_resize_or_window_boundaries() {
        let mut backend = Backend::default();
        backend.create(1, 0, 2, 2).unwrap();
        backend.create(2, 0, 2, 2).unwrap();
        for sequence in 1..=400 {
            backend
                .push_input(InputEvent {
                    kind: event::MOVE,
                    window: 1,
                    detail: sequence,
                    x: sequence as f64,
                    ..Default::default()
                })
                .unwrap();
        }
        backend
            .push_input(InputEvent {
                kind: event::RESIZE,
                window: 1,
                x: 100.,
                y: 100.,
                ..Default::default()
            })
            .unwrap();
        backend
            .push_input(InputEvent {
                kind: event::MOVE,
                window: 1,
                detail: 401,
                ..Default::default()
            })
            .unwrap();
        backend
            .push_input(InputEvent {
                kind: event::MOVE,
                window: 2,
                detail: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(backend.poll_input().unwrap().detail, 400);
        assert_eq!(backend.poll_input().unwrap().kind, event::RESIZE);
        assert_eq!(backend.poll_input().unwrap().detail, 401);
        assert_eq!(backend.poll_input().unwrap().window, 2);
        assert!(backend.poll_input().is_none());
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
