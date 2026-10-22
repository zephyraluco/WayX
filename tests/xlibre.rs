//! Real XLibre protocol -> Pixman/FB -> Damage -> Rust, plus Rust -> X11 input.
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
};

fn wait_for(mut ready: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    anyhow::bail!("Timed out waiting for XLibre state")
}
#[test]
#[ignore = "requires built XLibre plugin; set WAYX_TEST_CORE and run --ignored"]
fn xlibre_draw_and_input_roundtrip() -> Result<()> {
    let core =
        std::env::var("WAYX_TEST_CORE").context("Set WAYX_TEST_CORE to libwayx-xserver.so")?;
    let display = std::env::var("WAYX_TEST_DISPLAY").unwrap_or_else(|_| ":191".into());
    let backend = Arc::new(Mutex::new(Backend::default()));
    let shared = backend.clone();
    let display_arg = display.clone();
    let worker = std::thread::spawn(move || -> Result<i32> {
        let library = unsafe { libloading::Library::new(core) }?;
        let run = unsafe {
            library.get::<unsafe extern "C" fn(
                *const Api,
                *mut Handle,
                i32,
                *mut *mut std::ffi::c_char,
            ) -> i32>(b"wayx_xserver_run\0")
        }?;
        let args: Vec<_> = [
            "XWayX",
            &display_arg,
            "-screen",
            "0",
            "320x240x24",
            "-crtcs",
            "1@320x240",
            if cfg!(windows) {
                "-listen"
            } else {
                "-nolisten"
            },
            "tcp",
            "+iglx",
            "-ac",
        ]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
        let mut pointers: Vec<_> = args.iter().map(|s| s.as_ptr().cast_mut()).collect();
        pointers.push(std::ptr::null_mut());
        let mut handle = Handle(shared);
        Ok(unsafe { run(&API, &mut handle, args.len() as i32, pointers.as_mut_ptr()) })
    });
    let outcome = (|| -> Result<()> {
        wait_for(|| backend.lock().unwrap().windows.contains_key(&1))?;
        let client_display = if cfg!(windows) {
            format!("127.0.0.1{display}")
        } else {
            display.clone()
        };
        let (conn, screen) = x11rb::connect(Some(&client_display))?;
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            root,
            10,
            20,
            100,
            80,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(0xff0000)
                .event_mask(
                    EventMask::KEY_PRESS | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION,
                ),
        )?
        .check()?;
        conn.map_window(win)?.check()?;
        conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)?
            .check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new().foreground(0x00ff00))?
            .check()?;
        conn.poly_fill_rectangle(
            win,
            gc,
            &[Rectangle {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            }],
        )?
        .check()?;
        conn.flush()?;
        wait_for(|| {
            let b = backend.lock().unwrap();
            let fb = &b.windows[&1].framebuffer;
            let green = (25 * 320 + 15) * 4;
            let red = (30 * 320 + 40) * 4;
            fb.pixels[green..green + 4] == [0, 255, 0, 255]
                && fb.pixels[red..red + 4] == [0, 0, 255, 255]
        })?;
        {
            let mut b = backend.lock().unwrap();
            b.push_input(InputEvent {
                kind: event::MOTION,
                window: 1,
                x: 30.,
                y: 40.,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::KEY,
                window: 1,
                detail: 4,
                flags: 1,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::KEY,
                window: 1,
                detail: 4,
                flags: 0,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::BUTTON,
                window: 1,
                detail: 1,
                flags: 1,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::BUTTON,
                window: 1,
                detail: 1,
                flags: 0,
                ..Default::default()
            })?;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut key, mut button) = (false, false);
        while Instant::now() < deadline && !(key && button) {
            if let Some(e) = conn.poll_for_event()? {
                match e {
                    Event::KeyPress(e) => key = e.detail == 38,
                    Event::ButtonPress(e) => button = e.detail == 1,
                    _ => {}
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        ensure!(
            key && button,
            "Expected X11 keycode 38 and button 1, got key={key}, button={button}"
        );
        glx_negotiation(&conn, win)?;
        scroll_roundtrip(backend.clone(), &conn, win, &client_display)?;
        #[cfg(feature = "desktop")]
        selection_roundtrip(backend.clone(), &client_display)?;
        conn.destroy_window(win)?.check()?;
        Ok(())
    })();
    backend.lock().unwrap().push_input(InputEvent {
        kind: event::SHUTDOWN,
        ..Default::default()
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    ensure!(worker.is_finished(), "XLibre did not shut down");
    ensure!(
        worker.join().expect("XLibre worker panic")? == 0,
        "XLibre failed"
    );
    outcome
}

#[cfg(feature = "desktop")]
fn selection_roundtrip(backend: wayx::bridge::SharedBackend, display: &str) -> Result<()> {
    use x11rb::wrapper::ConnectionExt as _;
    let (bridge_conn, screen) = x11rb::connect(Some(display))?;
    let mut bridge = wayx::selection::SelectionBridge::new(bridge_conn, screen, backend.clone())?;
    let (conn, screen) = x11rb::connect(Some(display))?;
    let window = conn.generate_id()?;
    conn.create_window(
        0,
        window,
        conn.setup().roots[screen].root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?
    .check()?;
    let atom = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let clipboard = atom(b"CLIPBOARD")?;
    let utf8 = atom(b"UTF8_STRING")?;
    let property = atom(b"WAYX_TEST")?;
    let incr = atom(b"INCR")?;
    // Native -> X11, including the ICCCM incremental transfer handshake.
    for text in ["你好, WayX".to_owned(), "转发".repeat(25000)] {
        backend.lock().unwrap().set_clipboard(&text)?;
        bridge.tick()?;
        conn.convert_selection(window, clipboard, utf8, property, x11rb::CURRENT_TIME)?
            .check()?;
        conn.flush()?;
        let mut received = Vec::new();
        let mut incremental = false;
        let mut done = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !done {
            bridge.tick()?;
            while let Some(event) = conn.poll_for_event()? {
                let fetch = match event {
                    Event::SelectionNotify(e) => {
                        ensure!(e.property != 0, "Selection conversion refused");
                        true
                    }
                    Event::PropertyNotify(e) => {
                        incremental && e.atom == property && e.state == Property::NEW_VALUE
                    }
                    _ => false,
                };
                if fetch {
                    let value = conn
                        .get_property(true, window, property, AtomEnum::ANY, 0, 8 * 1024 * 1024)?
                        .reply()?;
                    if value.type_ == incr {
                        incremental = true;
                        conn.delete_property(window, property)?;
                    } else if value.type_ == utf8 {
                        received.extend_from_slice(&value.value);
                        done = !incremental || value.value.is_empty();
                    }
                }
            }
            conn.flush()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        ensure!(
            done && received == text.as_bytes(),
            "Native -> X11 clipboard mismatch"
        );
    }
    // X11 -> native, including repeated ownership by the same window and INCR.
    for expected in ["远端剪贴板".to_owned(), "远端增量".repeat(18000)] {
        conn.set_selection_owner(window, clipboard, x11rb::CURRENT_TIME)?
            .check()?;
        conn.flush()?;
        let mut outgoing: Option<(u32, u32, usize)> = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && backend.lock().unwrap().clipboard != expected {
            bridge.tick()?;
            while let Some(event) = conn.poll_for_event()? {
                match event {
                    Event::SelectionRequest(e) => {
                        if expected.len() > 60000 {
                            conn.change_window_attributes(
                                e.requestor,
                                &ChangeWindowAttributesAux::new()
                                    .event_mask(EventMask::PROPERTY_CHANGE),
                            )?;
                            conn.change_property32(
                                PropMode::REPLACE,
                                e.requestor,
                                e.property,
                                incr,
                                &[expected.len() as u32],
                            )?;
                            outgoing = Some((e.requestor, e.property, 0));
                        } else {
                            conn.change_property8(
                                PropMode::REPLACE,
                                e.requestor,
                                e.property,
                                utf8,
                                expected.as_bytes(),
                            )?;
                        }
                        conn.send_event(
                            false,
                            e.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: e.time,
                                requestor: e.requestor,
                                selection: e.selection,
                                target: e.target,
                                property: e.property,
                            },
                        )?;
                    }
                    Event::PropertyNotify(e) if e.state == Property::DELETE => {
                        if let Some((receiver, property, offset)) = outgoing
                            && receiver == e.window
                            && property == e.atom
                        {
                            let end = (offset + 60000).min(expected.len());
                            conn.change_property8(
                                PropMode::REPLACE,
                                receiver,
                                property,
                                utf8,
                                &expected.as_bytes()[offset..end],
                            )?;
                            outgoing = if end == offset {
                                None
                            } else {
                                Some((receiver, property, end))
                            };
                        }
                    }
                    _ => {}
                }
            }
            conn.flush()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        ensure!(
            backend.lock().unwrap().clipboard == expected,
            "X11 -> native clipboard mismatch"
        );
    }
    Ok(())
}

fn scroll_roundtrip(
    backend: wayx::bridge::SharedBackend,
    core: &x11rb::rust_connection::RustConnection,
    window: u32,
    display: &str,
) -> Result<()> {
    use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceClassData, XIEventMask};
    let (xi, _) = x11rb::connect(Some(display))?;
    xi.xinput_xi_query_version(2, 1)?.reply()?;
    let devices = xi.xinput_xi_query_device(0u16)?.reply()?;
    let pointer = devices
        .infos
        .iter()
        .find(|device| device.name == b"Xvfb mouse")
        .context("Missing WayX pointer")?;
    let scroll: Vec<_> = pointer
        .classes
        .iter()
        .filter_map(|class| match &class.data {
            DeviceClassData::Scroll(scroll) => Some(scroll),
            _ => None,
        })
        .collect();
    ensure!(scroll.len() == 2, "Missing XI2 smooth-scroll axes");
    xi.xinput_xi_select_events(
        window,
        &[xinput::EventMask {
            deviceid: 1,
            mask: vec![XIEventMask::MOTION],
        }],
    )?
    .check()?;
    xi.flush()?;
    while core.poll_for_event()?.is_some() {}
    for (dx, dy, vertical, horizontal) in [(-0.125, 0.125, 4, 6), (0.125, -0.125, 5, 7)] {
        for _ in 0..16 {
            backend.lock().unwrap().push_input(InputEvent {
                kind: event::SCROLL,
                window: 1,
                x: dx,
                y: dy,
                ..Default::default()
            })?;
            // Let XLibre publish each fractional sample, rather than only a final coalesced motion.
            std::thread::sleep(Duration::from_millis(12));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let (mut up, mut left, mut fractional) = (0, 0, false);
        while Instant::now() < deadline {
            while let Some(event) = core.poll_for_event()? {
                if let Event::ButtonPress(event) = event {
                    match event.detail {
                        n if n == vertical => up += 1,
                        n if n == horizontal => left += 1,
                        _ => {}
                    }
                }
            }
            while let Some(event) = xi.poll_for_event()? {
                if let Event::XinputMotion(event) = event {
                    fractional |= event.axisvalues.iter().any(|value| value.frac != 0);
                }
            }
            if up == 2 && left == 2 && fractional {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ensure!(
            up == 2 && left == 2,
            "Fractional scroll lost or duplicated: up={up}, left={left}"
        );
        ensure!(fractional, "XI2 did not deliver fractional valuators");
    }
    let point = core.query_pointer(window)?.reply()?;
    ensure!(
        point.root_x == 30 && point.root_y == 40,
        "Scrolling moved the pointer"
    );
    Ok(())
}

fn glx_negotiation(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Result<()> {
    use x11rb::protocol::glx::ConnectionExt as _;
    ensure!(
        !conn.query_extension(b"MIT-SHM")?.reply()?.present,
        "Network display must not advertise host shared memory"
    );
    let version = conn.glx_query_version(1, 4)?.reply()?;
    ensure!(
        version.major_version == 1 && version.minor_version >= 4,
        "Missing GLX 1.4 negotiation"
    );
    let configs = conn.glx_get_fb_configs(0)?.reply()?;
    ensure!(configs.num_fb_configs > 0, "No GLX FBConfigs");
    let id = configs
        .property_list
        .as_chunks::<2>()
        .0
        .iter()
        .find(|pair| pair[0] == 0x8013)
        .context("Missing FBConfig ID")?[1];
    let context = conn.generate_id()?;
    conn.glx_create_new_context(context, id, 0, 0x8014, 0, true)?
        .check()?;
    ensure!(
        conn.glx_is_direct(context)?.reply()?.is_direct,
        "Context incorrectly advertised as indirect"
    );
    let current = conn.glx_make_current(window, context, 0)?.reply()?;
    ensure!(
        conn.glx_get_string(current.context_tag, 0x1f00)?
            .reply()
            .is_err(),
        "Server GL command accepted on a client context"
    );
    conn.glx_make_current(0, 0, current.context_tag)?.reply()?;
    conn.glx_destroy_context(context)?.check()?;
    let indirect = conn.generate_id()?;
    ensure!(
        conn.glx_create_new_context(indirect, id, 0, 0x8014, 0, false)?
            .check()
            .is_err(),
        "Unimplemented indirect rendering was accepted"
    );
    Ok(())
}
