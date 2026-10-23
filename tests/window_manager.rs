//! Exercise ICCCM/EWMH against a real XLibre server, without a host window manager.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wayx::{
    backend::{Backend, InputEvent, event},
    bridge::{API, Api, Handle},
    selection::SelectionBridge,
    window,
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};
#[track_caller]
fn pump(wm: &mut SelectionBridge, mut ready: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        wm.tick()?;
        if ready()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    anyhow::bail!(
        "Window manager state did not converge at {}",
        std::panic::Location::caller()
    )
}
#[test]
#[ignore = "requires built XLibre plugin; set WAYX_TEST_CORE"]
fn window_management_should_route_requests_hints_states_and_cooperative_close() -> Result<()> {
    let path = std::env::var("WAYX_TEST_CORE").context("Set WAYX_TEST_CORE")?;
    let backend = Arc::new(Mutex::new(Backend::default()));
    let shared = backend.clone();
    let worker = std::thread::spawn(move || -> Result<i32> {
        // SAFETY: pinned plugin, live vtable, arguments and backend until run returns.
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
            ":195",
            "-wayx-rootless",
            "-screen",
            "0",
            "640x480x24",
            "-crtcs",
            "1@640x480",
            if cfg!(windows) {
                "-listen"
            } else {
                "-nolisten"
            },
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
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.lock().unwrap().windows.contains_key(&1) {
            ensure!(Instant::now() < deadline, "Server did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        // A reply may wake a socket watcher. Idle WM ticks must not manufacture replies.
        {
            let (audit, screen) = x11rb::connect(Some(if cfg!(windows) {
                "127.0.0.1:195"
            } else {
                ":195"
            }))?;
            let root = audit.setup().roots[screen].root;
            let owner = audit.generate_id()?;
            audit
                .create_window(
                    0,
                    owner,
                    root,
                    0,
                    0,
                    1,
                    1,
                    0,
                    WindowClass::INPUT_ONLY,
                    0,
                    &CreateWindowAux::new(),
                )?
                .check()?;
            let mut manager = wayx::wm::WindowManager::new(&audit, screen, owner, backend.clone())?;
            manager.tick(&audit)?;
            let before = audit.get_input_focus()?;
            let sequence = before.sequence_number();
            before.reply()?;
            for _ in 0..100 {
                manager.tick(&audit)?;
            }
            let after = audit.get_input_focus()?;
            ensure!(
                after.sequence_number() == sequence + 1,
                "Idle WM tick sent X11 queries"
            );
            after.reply()?;
            let selection = audit.intern_atom(false, b"WM_S0")?.reply()?.atom;
            audit
                .set_selection_owner(0u32, selection, x11rb::CURRENT_TIME)?
                .check()?;
            audit
                .change_window_attributes(
                    root,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
                )?
                .check()?;
            audit.destroy_window(owner)?.check()?;
        }
        let mut wm = SelectionBridge::connect(
            if cfg!(windows) {
                "127.0.0.1:195"
            } else {
                ":195"
            },
            backend.clone(),
        )?;
        wm.enable_window_management()?;
        let mut competitor = SelectionBridge::connect(
            if cfg!(windows) {
                "127.0.0.1:195"
            } else {
                ":195"
            },
            backend.clone(),
        )?;
        ensure!(
            competitor.enable_window_management().is_err(),
            "Second manager acquired screen"
        );
        let (conn, s) = x11rb::connect(Some(if cfg!(windows) {
            "127.0.0.1:195"
        } else {
            ":195"
        }))?;
        let root = conn.setup().roots[s].root;
        let atom = |name: &str| -> Result<u32> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let a = conn.generate_id()?;
        let b = conn.generate_id()?;
        let popup = conn.generate_id()?;
        for id in [a, b, popup] {
            conn.create_window(
                0,
                id,
                root,
                10,
                20,
                100,
                80,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().override_redirect(u32::from(id == popup)),
            )?
            .check()?;
        }
        let protocols = atom("WM_PROTOCOLS")?;
        let delete = atom("WM_DELETE_WINDOW")?;
        conn.change_property32(PropMode::REPLACE, a, protocols, AtomEnum::ATOM, &[delete])?;
        conn.change_property32(
            PropMode::REPLACE,
            b,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[a],
        )?;
        let mut hints = [0; 18];
        hints[0] = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8);
        hints[5] = 40;
        hints[6] = 30;
        hints[7] = 400;
        hints[8] = 300;
        hints[9] = 8;
        hints[10] = 10;
        hints[15] = 0;
        hints[16] = 0;
        conn.change_property32(
            PropMode::REPLACE,
            a,
            AtomEnum::WM_NORMAL_HINTS,
            AtomEnum::WM_SIZE_HINTS,
            &hints,
        )?;
        for id in [a, b, popup] {
            conn.map_window(id)?;
        }
        conn.flush()?;
        pump(&mut wm, || {
            let state = backend.lock().unwrap();
            Ok([a, b, popup]
                .iter()
                .all(|id| state.windows.get(id).is_some_and(|w| w.mapped)))
        })?;
        ensure!(backend.lock().unwrap().windows[&b].state.transient_for == a);
        ensure!(
            backend.lock().unwrap().windows[&popup].state.flags & window::OVERRIDE_REDIRECT != 0
        );
        let list = atom("_NET_CLIENT_LIST")?;
        pump(&mut wm, || {
            let ids: Vec<_> = conn
                .get_property(false, root, list, AtomEnum::WINDOW, 0, 32)?
                .reply()?
                .value32()
                .map(Iterator::collect)
                .unwrap_or_default();
            Ok(ids.contains(&a) && ids.contains(&b) && !ids.contains(&popup))
        })?;
        conn.configure_window(
            a,
            &ConfigureWindowAux::new().x(55).y(65).width(103).height(83),
        )?;
        conn.flush()?;
        pump(&mut wm, || {
            let state = backend.lock().unwrap();
            let w = &state.windows[&a];
            Ok(w.state.x == 55
                && w.state.y == 65
                && w.framebuffer.width == 96
                && w.framebuffer.height == 80)
        })?;
        for input in [
            InputEvent {
                kind: event::MOVE,
                window: a,
                x: 70.0,
                y: 90.0,
                ..Default::default()
            },
            InputEvent {
                kind: event::RESIZE,
                window: a,
                x: 135.0,
                y: 117.0,
                ..Default::default()
            },
        ] {
            backend.lock().unwrap().push_input(input)?;
        }
        pump(&mut wm, || {
            let g = conn.get_geometry(a)?.reply()?;
            Ok((g.x, g.y, g.width, g.height) == (70, 90, 128, 110))
        })?;
        let state_atom = atom("_NET_WM_STATE")?;
        let above = atom("_NET_WM_STATE_ABOVE")?;
        let full = atom("_NET_WM_STATE_FULLSCREEN")?;
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, a, state_atom, [1, above, full, 1, 0]),
        )?;
        conn.flush()?;
        pump(&mut wm, || {
            Ok(backend.lock().unwrap().windows[&a].state.flags
                & (window::ABOVE | window::FULLSCREEN)
                == window::ABOVE | window::FULLSCREEN)
        })?;
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::STATE,
            window: a,
            flags: window::MAXIMIZED,
            ..Default::default()
        })?;
        pump(&mut wm, || {
            let flags = backend.lock().unwrap().windows[&a].state.flags;
            Ok(flags & window::MAXIMIZED != 0
                && flags & window::FULLSCREEN == 0
                && flags & window::ABOVE != 0)
        })?;
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::STATE,
            window: a,
            flags: window::MINIMIZED,
            ..Default::default()
        })?;
        pump(&mut wm, || {
            Ok(backend.lock().unwrap().windows[&a].state.flags & window::MINIMIZED != 0)
        })?;
        let geometry = conn.get_geometry(a)?.reply()?;
        ensure!(
            (geometry.width, geometry.height) == (128, 110),
            "Minimizing changed restore geometry"
        );
        let wm_state = atom("WM_STATE")?;
        ensure!(
            conn.get_property(false, a, wm_state, wm_state, 0, 2)?
                .reply()?
                .value32()
                .is_some_and(|mut values| values.next() == Some(3)),
            "Application did not observe IconicState"
        );
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::STATE,
            window: a,
            ..Default::default()
        })?;
        pump(&mut wm, || {
            Ok(backend.lock().unwrap().windows[&a].state.flags & window::MINIMIZED == 0)
        })?;
        let active = atom("_NET_ACTIVE_WINDOW")?;
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT,
            ClientMessageEvent::new(32, b, active, [1, 0, 0, 0, 0]),
        )?;
        conn.flush()?;
        pump(&mut wm, || Ok(conn.get_input_focus()?.reply()?.focus == b))?;
        pump(&mut wm, || {
            Ok(conn
                .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)?
                .reply()?
                .value32()
                .is_some_and(|mut ids| ids.next() == Some(b)))
        })?;
        let stacking = atom("_NET_CLIENT_LIST_STACKING")?;
        let check_stacking = |wm: &mut SelectionBridge, managed: &[u32]| -> Result<()> {
            pump(wm, || {
                let expected: Vec<_> = conn
                    .query_tree(root)?
                    .reply()?
                    .children
                    .into_iter()
                    .filter(|id| managed.contains(id))
                    .collect();
                let actual: Vec<_> = conn
                    .get_property(false, root, stacking, AtomEnum::WINDOW, 0, 1024)?
                    .reply()?
                    .value32()
                    .map(Iterator::collect)
                    .unwrap_or_default();
                Ok(actual == expected && actual.len() == managed.len())
            })
        };
        check_stacking(&mut wm, &[a, b])?;
        for (id, sibling, mode) in [
            (a, b, StackMode::ABOVE),
            (popup, a, StackMode::BELOW),
            (b, popup, StackMode::BELOW),
        ] {
            conn.configure_window(
                id,
                &ConfigureWindowAux::new().sibling(sibling).stack_mode(mode),
            )?;
            conn.flush()?;
            check_stacking(&mut wm, &[a, b])?;
        }
        conn.reparent_window(b, a, 0, 0)?.check()?;
        check_stacking(&mut wm, &[a])?;
        conn.reparent_window(b, root, 30, 30)?.check()?;
        conn.map_window(b)?;
        conn.flush()?;
        check_stacking(&mut wm, &[a, b])?;
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::CLOSE,
            window: a,
            ..Default::default()
        })?;
        let mut received = false;
        pump(&mut wm, || {
            while let Some(event) = conn.poll_for_event()? {
                if let Event::ClientMessage(e) = event {
                    received |=
                        e.window == a && e.type_ == protocols && e.data.as_data32()[0] == delete;
                }
            }
            Ok(received)
        })?;
        ensure!(
            conn.get_window_attributes(a)?.reply()?.map_state == MapState::VIEWABLE,
            "Close bypassed the application"
        );
        conn.destroy_window(a)?.check()?;
        conn.flush()?;
        pump(&mut wm, || {
            Ok(!backend.lock().unwrap().windows.contains_key(&a))
        })?;
        ensure!(
            backend.lock().unwrap().windows[&b].mapped,
            "Closing one window affected another"
        );
        Ok(())
    })();
    backend.lock().unwrap().push_input(InputEvent {
        kind: event::SHUTDOWN,
        ..Default::default()
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() {
        ensure!(Instant::now() < deadline, "Server did not stop");
        std::thread::sleep(Duration::from_millis(5));
    }
    ensure!(worker.join().expect("XLibre worker panicked")? == 0);
    result
}
