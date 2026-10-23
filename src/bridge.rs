//! C ABI v4. Callers own pointers and must keep buffers valid for each call.
//! The handle may be shared across threads; freeing it requires all calls to stop.
use crate::backend::{Backend, InputEvent, Rect};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

pub const ABI_VERSION: u32 = 4;
pub type SharedBackend = Arc<Mutex<Backend>>;
pub struct Handle(pub SharedBackend);

fn guarded(f: impl FnOnce() -> Result<(), ()>) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => 0,
        Ok(Err(())) => -1,
        Err(_) => -2,
    }
}
unsafe fn access(handle: *mut Handle, f: impl FnOnce(&mut Backend) -> Result<(), ()>) -> i32 {
    guarded(|| {
        let handle = unsafe { handle.as_ref() }.ok_or(())?;
        let mut backend = handle.0.lock().map_err(|_| ())?;
        f(&mut backend)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn wayx_backend_new() -> *mut Handle {
    Box::into_raw(Box::new(Handle(Arc::new(Mutex::new(Backend::default())))))
}
/// # Safety
/// `handle` is null or a live handle created by WayX. No concurrent calls may remain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_backend_free(handle: *mut Handle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}
/// # Safety
/// `handle` must be null or a live WayX handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_create(
    handle: *mut Handle,
    id: u32,
    parent: u32,
    width: u32,
    height: u32,
) -> i32 {
    unsafe {
        access(handle, |b| {
            b.create(id, parent, width, height).map_err(|_| ())?;
            b.output_pending = true;
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be null or a live WayX handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_destroy(handle: *mut Handle, id: u32) -> i32 {
    unsafe {
        access(handle, |b| {
            b.destroy(id).map_err(|_| ())?;
            b.output_pending = true;
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be null or a live WayX handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_resize(
    handle: *mut Handle,
    id: u32,
    width: u32,
    height: u32,
) -> i32 {
    unsafe {
        access(handle, |b| {
            b.resize(id, width, height).map_err(|_| ())?;
            b.output_pending = true;
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be live. `text` points to `len` readable UTF-8 bytes or is null when len=0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_title(
    handle: *mut Handle,
    id: u32,
    text: *const u8,
    len: usize,
) -> i32 {
    unsafe {
        access(handle, |b| {
            let text = read_text(text, len, 4096)?;
            let window = b.windows.get_mut(&id).ok_or(())?;
            if window.title != text {
                window.title = text.to_owned();
                b.output_pending = true;
            }
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be null or live. The window id must exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_map(handle: *mut Handle, id: u32, mapped: u32) -> i32 {
    unsafe {
        access(handle, |b| {
            if !b.windows.contains_key(&id) {
                return Err(());
            }
            b.pending_maps.insert(id, mapped != 0);
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be live; `data` points to `len` readable bytes for this call.
/// Source is XRGB/BGRA little-endian. It starts at the damage origin, not screen origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_frame_damage(
    handle: *mut Handle,
    id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    stride: usize,
    data: *const u8,
    len: usize,
) -> i32 {
    unsafe {
        access(handle, |b| {
            if len > crate::backend::MAX_FRAME_BYTES || (data.is_null() && len != 0) {
                return Err(());
            }
            let data = if len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(data, len)
            };
            b.windows
                .get_mut(&id)
                .ok_or(())?
                .framebuffer
                .upload(
                    Rect {
                        x,
                        y,
                        width,
                        height,
                    },
                    stride,
                    data,
                )
                .map_err(|_| ())?;
            if width != 0 && height != 0 {
                b.output_pending = true;
            }
            Ok(())
        })
    }
}
/// # Safety
/// `handle` must be live; `out` must point to writable InputEvent storage.
/// Returns 1 for an event, 0 for empty, negative on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_input_poll(handle: *mut Handle, out: *mut InputEvent) -> i32 {
    let mut found = false;
    let result = unsafe {
        access(handle, |b| {
            if out.is_null() {
                return Err(());
            }
            if let Some(event) = b.poll_input() {
                out.write(event);
                found = true;
            }
            Ok(())
        })
    };
    if result < 0 { result } else { i32::from(found) }
}
unsafe fn read_text<'a>(data: *const u8, len: usize, max: usize) -> Result<&'a str, ()> {
    if len > max || (data.is_null() && len != 0) {
        return Err(());
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    if text.contains('\0') {
        return Err(());
    }
    Ok(text)
}
/// # Safety
/// `handle` must be live; text points to len readable bytes, or null when len=0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_clipboard_set(
    handle: *mut Handle,
    text: *const u8,
    len: usize,
) -> i32 {
    unsafe {
        access(handle, |b| {
            b.set_clipboard(read_text(text, len, 16 * 1024 * 1024)?)
                .map_err(|_| ())
        })
    }
}
/// # Safety
/// `handle` must be live; needed is writable; data has capacity writable bytes.
/// Query with data=null and capacity=0. Output is UTF-8 without a trailing NUL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_clipboard_get(
    handle: *mut Handle,
    data: *mut u8,
    capacity: usize,
    needed: *mut usize,
) -> i32 {
    unsafe {
        access(handle, |b| {
            if needed.is_null() {
                return Err(());
            }
            needed.write(b.clipboard.len());
            if capacity == 0 {
                return Ok(());
            }
            if data.is_null() || capacity < b.clipboard.len() {
                return Err(());
            }
            std::ptr::copy_nonoverlapping(b.clipboard.as_ptr(), data, b.clipboard.len());
            Ok(())
        })
    }
}
/// # Safety
/// Handle must be live; state points to a readable WindowState for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_state(
    handle: *mut Handle,
    id: u32,
    state: *const crate::window::WindowState,
) -> i32 {
    unsafe {
        access(handle, |backend| {
            let state = state.as_ref().ok_or(())?;
            state.validate().map_err(|_| ())?;
            if state.transient_for == id {
                return Err(());
            }
            let window = backend.windows.get(&id).ok_or(())?;
            let mut geometry = backend
                .pending_geometry
                .get(&id)
                .copied()
                .unwrap_or(window.geometry);
            geometry.state = *state;
            backend.pending_geometry.insert(id, geometry);
            Ok(())
        })
    }
}
/// # Safety
/// Handle is live; geometry points to one complete post-processing observation.
unsafe extern "C" fn window_geometry(
    handle: *mut Handle,
    id: u32,
    geometry: *const crate::window::GeometrySnapshot,
) -> i32 {
    unsafe {
        access(handle, |backend| {
            let geometry = *geometry.as_ref().ok_or(())?;
            geometry.state.validate().map_err(|_| ())?;
            if !backend.windows.contains_key(&id)
                || geometry.state.transient_for == id
                || geometry.width == 0
                || geometry.height == 0
                || geometry.width > crate::backend::MAX_DIMENSION
                || geometry.height > crate::backend::MAX_DIMENSION
            {
                return Err(());
            }
            backend.pending_geometry.insert(id, geometry);
            Ok(())
        })
    }
}
/// # Safety
/// Handle is live; width and height point to distinct writable u32 values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wayx_window_constrain(
    handle: *mut Handle,
    id: u32,
    width: *mut u32,
    height: *mut u32,
) -> i32 {
    unsafe {
        access(handle, |backend| {
            if width == height {
                return Err(());
            }
            let (Some(width), Some(height)) = (width.as_mut(), height.as_mut()) else {
                return Err(());
            };
            let window = backend.windows.get(&id).ok_or(())?;
            // DDX constraints can use staged hints; UI geometry remains committed.
            let geometry = backend
                .pending_geometry
                .get(&id)
                .unwrap_or(&window.geometry);
            let size = geometry.state.constrain(*width, *height);
            *width = size.0;
            *height = size.1;
            Ok(())
        })
    }
}
/// # Safety
/// Handle must be live. The borrowed socket remains owned by the backend.
unsafe extern "C" fn input_fd(handle: *mut Handle) -> i32 {
    let mut fd = -1;
    let result = unsafe {
        access(handle, |b| {
            if b.input_wake.is_none() {
                b.input_wake = Some(crate::notify::InputWake::new().map_err(|_| ())?);
            }
            let wake = b.input_wake.as_mut().ok_or(())?;
            fd = wake.descriptor().map_err(|_| ())?;
            // Include input queued before the server registered its socket.
            wake.signal().map_err(|_| ())
        })
    };
    if result < 0 { result } else { fd }
}
/// # Safety
/// Handle must be live; called on the server thread before draining input.
unsafe extern "C" fn input_ack(handle: *mut Handle) -> i32 {
    unsafe {
        access(handle, |b| {
            b.input_wake.as_mut().ok_or(())?.drain().map_err(|_| ())
        })
    }
}
/// # Safety
/// Handle must be live; called after publishing a batch of output mutations.
unsafe extern "C" fn output_flush(handle: *mut Handle) -> i32 {
    unsafe {
        access(handle, |b| {
            b.flush_output();
            b.rearm_input()
        })
    }
}
#[repr(C)]
pub struct Api {
    // Appended fields below require rebuilding the DDX for ABI v4.
    pub abi_version: u32,
    pub struct_size: u32,
    pub window_create: unsafe extern "C" fn(*mut Handle, u32, u32, u32, u32) -> i32,
    pub window_destroy: unsafe extern "C" fn(*mut Handle, u32) -> i32,
    pub window_resize: unsafe extern "C" fn(*mut Handle, u32, u32, u32) -> i32,
    pub window_title: unsafe extern "C" fn(*mut Handle, u32, *const u8, usize) -> i32,
    pub window_map: unsafe extern "C" fn(*mut Handle, u32, u32) -> i32,
    pub frame_damage:
        unsafe extern "C" fn(*mut Handle, u32, u32, u32, u32, u32, usize, *const u8, usize) -> i32,
    pub input_poll: unsafe extern "C" fn(*mut Handle, *mut InputEvent) -> i32,
    pub clipboard_set: unsafe extern "C" fn(*mut Handle, *const u8, usize) -> i32,
    pub clipboard_get: unsafe extern "C" fn(*mut Handle, *mut u8, usize, *mut usize) -> i32,
    pub window_state:
        unsafe extern "C" fn(*mut Handle, u32, *const crate::window::WindowState) -> i32,
    pub window_constrain: unsafe extern "C" fn(*mut Handle, u32, *mut u32, *mut u32) -> i32,
    pub input_fd: unsafe extern "C" fn(*mut Handle) -> i32,
    pub input_ack: unsafe extern "C" fn(*mut Handle) -> i32,
    pub output_flush: unsafe extern "C" fn(*mut Handle) -> i32,
    pub window_geometry:
        unsafe extern "C" fn(*mut Handle, u32, *const crate::window::GeometrySnapshot) -> i32,
}
pub static API: Api = Api {
    window_geometry,
    input_fd,
    input_ack,
    output_flush,
    abi_version: ABI_VERSION,
    struct_size: std::mem::size_of::<Api>() as u32,
    window_create: wayx_window_create,
    window_destroy: wayx_window_destroy,
    window_resize: wayx_window_resize,
    window_title: wayx_window_title,
    window_map: wayx_window_map,
    frame_damage: wayx_frame_damage,
    input_poll: wayx_input_poll,
    clipboard_set: wayx_clipboard_set,
    clipboard_get: wayx_clipboard_get,
    window_state: wayx_window_state,
    window_constrain: wayx_window_constrain,
};
#[unsafe(no_mangle)]
pub extern "C" fn wayx_get_api() -> *const Api {
    &API
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_rejects_null_and_bad_utf8_and_roundtrips_clipboard() {
        unsafe {
            assert_eq!(wayx_window_create(std::ptr::null_mut(), 1, 0, 1, 1), -1);
            let h = wayx_backend_new();
            assert_eq!(wayx_window_create(h, 1, 0, 2, 2), 0);
            assert_eq!(wayx_window_title(h, 1, [255].as_ptr(), 1), -1);
            assert_eq!(wayx_clipboard_set(h, b"hello".as_ptr(), 5), 0);
            let mut needed = 0;
            assert_eq!(
                wayx_clipboard_get(h, std::ptr::null_mut(), 0, &mut needed),
                0
            );
            assert_eq!(needed, 5);
            let mut out = [0; 5];
            assert_eq!(wayx_clipboard_get(h, out.as_mut_ptr(), 5, &mut needed), 0);
            assert_eq!(&out, b"hello");
            wayx_backend_free(h);
        }
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn output_batch_notifies_once_and_ignores_unchanged_metadata() {
        let count = Arc::new(AtomicUsize::new(0));
        let observer = count.clone();
        let mut backend = Backend::default();
        backend.ui_wake = Some(Arc::new(move || {
            observer.fetch_add(1, Ordering::SeqCst);
        }));
        let mut handle = Handle(Arc::new(Mutex::new(backend)));
        unsafe {
            assert_eq!(wayx_window_create(&mut handle, 1, 0, 2, 2), 0);
            assert_eq!(wayx_window_title(&mut handle, 1, b"test".as_ptr(), 4), 0);
            for _ in 0..20 {
                assert_eq!(
                    wayx_frame_damage(&mut handle, 1, 0, 0, 2, 2, 8, [0u8; 16].as_ptr(), 16),
                    0
                );
            }
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert_eq!(output_flush(&mut handle), 0);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(wayx_window_title(&mut handle, 1, b"test".as_ptr(), 4), 0);
            assert_eq!(wayx_window_map(&mut handle, 1, 0), 0);
            assert_eq!(wayx_window_state(&mut handle, 1, &Default::default()), 0);
            assert_eq!(output_flush(&mut handle), 0);
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }
}

#[cfg(test)]
mod geometry_batch_tests {
    use super::*;
    use crate::window::{GeometrySnapshot, WindowState};
    fn handle() -> Handle {
        let mut backend = Backend::default();
        backend.create(1, 0, 400, 287).unwrap();
        Handle(Arc::new(Mutex::new(backend)))
    }
    fn published(handle: &Handle) -> (GeometrySnapshot, u64) {
        let backend = handle.0.lock().unwrap();
        let window = &backend.windows[&1];
        (window.geometry, window.geometry_generation)
    }
    fn snapshot(sequence: u32, x: i32) -> GeometrySnapshot {
        GeometrySnapshot {
            state: WindowState {
                x,
                flags: crate::window::ROOTLESS,
                ..Default::default()
            },
            width: 400,
            height: 287,
            move_ack: sequence,
            resize_ack: 17,
        }
    }
    #[test]
    fn poll_and_damage_cannot_acknowledge_unprocessed_geometry() {
        let mut handle = handle();
        let old = published(&handle);
        handle
            .0
            .lock()
            .unwrap()
            .push_input(InputEvent {
                kind: crate::backend::event::MOVE,
                window: 1,
                detail: 11,
                x: 237.,
                ..Default::default()
            })
            .unwrap();
        let mut event = InputEvent::default();
        unsafe {
            assert_eq!(wayx_input_poll(&mut handle, &mut event), 1);
            assert_eq!(
                wayx_frame_damage(&mut handle, 1, 0, 0, 1, 1, 4, [0u8; 4].as_ptr(), 4),
                0
            );
            assert_eq!(output_flush(&mut handle), 0);
        }
        assert_eq!(published(&handle), old);
    }
    #[test]
    fn ui_sees_whole_old_or_new_snapshot_across_interleaved_batches() {
        let mut handle = handle();
        let old = published(&handle);
        let first = snapshot(10, 236);
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &first), 0);
        }
        assert_eq!(published(&handle), old);
        // A later completion replaces the entire observation, never just its ack.
        let latest = snapshot(11, 237);
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &latest), 0);
        }
        assert_eq!(published(&handle), old);
        unsafe {
            assert_eq!(output_flush(&mut handle), 0);
        }
        assert_eq!(published(&handle), (latest, 1));
        let next = snapshot(12, 238);
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &next), 0);
        }
        assert_eq!(published(&handle), (latest, 1));
        unsafe {
            assert_eq!(output_flush(&mut handle), 0);
        }
        assert_eq!(published(&handle), (next, 2));
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &next), 0);
            assert_eq!(output_flush(&mut handle), 0);
        }
        assert_eq!(published(&handle), (next, 2));
    }
    #[test]
    fn rejected_resize_and_unchanged_move_have_explicit_final_snapshots() {
        let mut handle = handle();
        let final_geometry = snapshot(11, 236); // Server kept its old position/size.
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &final_geometry), 0);
            assert_eq!(output_flush(&mut handle), 0);
        }
        assert_eq!(published(&handle).0, final_geometry);
        assert_eq!(published(&handle).0.resize_ack, 17);
    }
    #[test]
    fn destroy_discards_pending_geometry_and_mapping() {
        let mut handle = handle();
        unsafe {
            assert_eq!(window_geometry(&mut handle, 1, &snapshot(11, 237)), 0);
            assert_eq!(wayx_window_map(&mut handle, 1, 1), 0);
            assert!(!handle.0.lock().unwrap().windows[&1].mapped);
            assert_eq!(wayx_window_destroy(&mut handle, 1), 0);
            assert_eq!(output_flush(&mut handle), 0);
        }
        let backend = handle.0.lock().unwrap();
        assert!(backend.windows.is_empty());
        assert!(backend.pending_geometry.is_empty());
        assert!(backend.pending_maps.is_empty());
    }
}
