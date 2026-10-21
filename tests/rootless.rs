//! Real redirected-window pixels and independent native-window lifetimes.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wayx::{
    backend::{Backend, InputEvent, event},
    bridge::{API, Api, Handle},
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

fn wait_for(mut predicate: impl FnMut() -> bool) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if predicate() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    anyhow::bail!("Rootless state did not converge")
}
#[test]
#[ignore = "requires built XLibre plugin; set WAYX_TEST_CORE"]
fn rootless_should_preserve_occluded_pixels_and_independent_windows() -> Result<()> {
    let path = std::env::var("WAYX_TEST_CORE").context("Set WAYX_TEST_CORE")?;
    let backend = Arc::new(Mutex::new(Backend::default()));
    let shared = backend.clone();
    let worker = std::thread::spawn(move || -> Result<i32> {
        // SAFETY: trusted plugin remains loaded, and all arguments/handle live until return.
        let library = unsafe { libloading::Library::new(path) }?;
        let run = unsafe {
            library.get::<unsafe extern "C" fn(
                *const Api,
                *mut Handle,
                i32,
                *mut *mut std::ffi::c_char,
            ) -> i32>(b"wayx_xserver_run\0")
        }?;
        let args = [
            "XWayX",
            ":194",
            "-wayx-rootless",
            "-screen",
            "0",
            "320x240x24",
            "-crtcs",
            "1@320x240",
            "-nolisten",
            "tcp",
            "-ac",
        ]
        .iter()
        .map(|s| CString::new(*s))
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut argv: Vec<_> = args.iter().map(|s| s.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let mut handle = Handle(shared);
        Ok(unsafe { run(&API, &mut handle, args.len() as i32, argv.as_mut_ptr()) })
    });
    let result = (|| -> Result<()> {
        wait_for(|| backend.lock().unwrap().windows.contains_key(&1))?;
        ensure!(
            !backend.lock().unwrap().windows[&1].mapped,
            "Root desktop must stay hidden"
        );
        let (conn, s) = x11rb::connect(Some(":194"))?;
        let root = conn.setup().roots[s].root;
        let a = conn.generate_id()?;
        let b = conn.generate_id()?;
        let child = conn.generate_id()?;
        for (id, color) in [(a, 0xff0000), (b, 0x0000ff)] {
            conn.create_window(
                0,
                id,
                root,
                10,
                20,
                100,
                80,
                2,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .background_pixel(color)
                    .event_mask(EventMask::KEY_PRESS | EventMask::BUTTON_PRESS),
            )?
            .check()?;
        }
        conn.create_window(
            0,
            child,
            a,
            5,
            5,
            10,
            10,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(0x00ff00),
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            a,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"independent A",
        )?
        .check()?;
        conn.map_window(a)?.check()?;
        conn.map_window(child)?.check()?;
        conn.map_window(b)?.check()?;
        conn.flush()?;
        wait_for(|| {
            let state = backend.lock().unwrap();
            let (Some(a), Some(b)) = (state.windows.get(&a), state.windows.get(&b)) else {
                return false;
            };
            let p = (7 * 100 + 7) * 4;
            a.mapped
                && b.mapped
                && a.framebuffer.pixels[..4] == [0, 0, 255, 255]
                && a.framebuffer.pixels[p..p + 4] == [0, 255, 0, 255]
                && b.framebuffer.pixels[..4] == [255, 0, 0, 255]
                && a.title == "independent A"
        })?;
        ensure!(
            !backend.lock().unwrap().windows.contains_key(&child),
            "Nested children must be composited into their top-level window"
        );
        // Draw while the entire window is occluded: desktop cropping would fail this check.
        let gc = conn.generate_id()?;
        conn.create_gc(gc, a, &CreateGCAux::new().foreground(0x112233))?
            .check()?;
        conn.poly_fill_rectangle(
            a,
            gc,
            &[Rectangle {
                x: 30,
                y: 30,
                width: 10,
                height: 10,
            }],
        )?
        .check()?;
        conn.flush()?;
        wait_for(|| {
            let state = backend.lock().unwrap();
            let p = (32 * 100 + 32) * 4;
            state.windows[&a].framebuffer.pixels[p..p + 4] == [0x33, 0x22, 0x11, 255]
        })?;
        {
            let mut state = backend.lock().unwrap();
            state.push_input(InputEvent {
                kind: event::FOCUS,
                window: a,
                flags: 1,
                ..Default::default()
            })?;
            state.push_input(InputEvent {
                kind: event::MOTION,
                window: a,
                x: 30.,
                y: 30.,
                ..Default::default()
            })?;
            state.push_input(InputEvent {
                kind: event::KEY,
                window: a,
                detail: 4,
                flags: 1,
                ..Default::default()
            })?;
            state.push_input(InputEvent {
                kind: event::KEY,
                window: a,
                detail: 4,
                flags: 0,
                ..Default::default()
            })?;
            state.push_input(InputEvent {
                kind: event::BUTTON,
                window: a,
                detail: 1,
                flags: 1,
                ..Default::default()
            })?;
            state.push_input(InputEvent {
                kind: event::BUTTON,
                window: a,
                detail: 1,
                flags: 0,
                ..Default::default()
            })?;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut key, mut button) = (false, false);
        while Instant::now() < deadline && !(key && button) {
            while let Some(e) = conn.poll_for_event()? {
                match e {
                    Event::KeyPress(e) => key = e.event == a && e.detail == 38,
                    Event::ButtonPress(e) => {
                        button = e.event == a && e.event_x == 30 && e.event_y == 30
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ensure!(
            key && button,
            "Rootless input was not delivered to the selected window"
        );
        conn.configure_window(a, &ConfigureWindowAux::new().width(80).height(60))?
            .check()?;
        conn.flush()?;
        wait_for(|| {
            let state = backend.lock().unwrap();
            state.windows[&a].framebuffer.width == 80 && state.windows[&a].framebuffer.height == 60
        })?;
        conn.unmap_window(a)?.check()?;
        conn.flush()?;
        wait_for(|| !backend.lock().unwrap().windows[&a].mapped)?;
        ensure!(
            backend.lock().unwrap().windows[&b].mapped,
            "Unmapping A must not affect B"
        );
        conn.map_window(a)?.check()?;
        conn.flush()?;
        wait_for(|| backend.lock().unwrap().windows[&a].mapped)?;
        conn.destroy_window(a)?.check()?;
        conn.flush()?;
        wait_for(|| !backend.lock().unwrap().windows.contains_key(&a))?;
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::CLOSE,
            window: b,
            ..Default::default()
        })?;
        wait_for(|| !backend.lock().unwrap().windows.contains_key(&b))?;
        ensure!(
            !worker.is_finished(),
            "Closing one native window terminated the X server"
        );
        Ok(())
    })();
    backend.lock().unwrap().push_input(InputEvent {
        kind: event::SHUTDOWN,
        ..Default::default()
    })?;
    wait_for(|| worker.is_finished())?;
    ensure!(
        worker.join().expect("XLibre worker panicked")? == 0,
        "XLibre returned failure"
    );
    result
}
