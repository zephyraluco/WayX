//! Real rootless desktop coordinates, pointer limits and multi-output layout changes.
#[path = "support/core.rs"]
mod core;
use anyhow::{Context, Result, ensure};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wayx::{
    backend::{Backend, InputEvent, event},
    bridge::{API, Api},
    layout::{DesktopLayout, MonitorRect},
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        randr::{Connection as OutputConnection, ConnectionExt as _, NotifyMask},
        xproto::*,
    },
};
fn wait(mut condition: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition()? {
        ensure!(Instant::now() < deadline, "Desktop layout did not converge");
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn layout(monitors: &[(i32, i32, u32, u32)]) -> Result<DesktopLayout> {
    Ok(DesktopLayout::new(
        monitors
            .iter()
            .map(|&(x, y, width, height)| MonitorRect {
                x,
                y,
                width,
                height,
            })
            .collect(),
    )?)
}
#[test]
#[ignore = "requires built XLibre plugin; set WAYX_TEST_CORE"]
fn host_desktop_should_cover_pointer_rebase_outputs_and_keep_root_pixels_minimal() -> Result<()> {
    let initial = layout(&[(-1920, 0, 1920, 1080), (0, 0, 1920, 1080)])?;
    let mut initial_backend = Backend::default();
    initial_backend.desktop_layout = Some(initial);
    let backend = Arc::new(Mutex::new(initial_backend));
    let server = core::CoreServer::start(
        backend.clone(),
        Api { ..API },
        &[
            "XWayX",
            ":193",
            "-wayx-rootless",
            "-listen",
            "tcp",
            "-nolisten",
            "local",
            "-ac",
        ],
    )?;
    let result = (|| -> Result<()> {
        wait(|| {
            Ok(backend
                .lock()
                .unwrap()
                .windows
                .get(&1)
                .is_some_and(|w| w.geometry.width == 3840))
        })?;
        let (conn, screen) = x11rb::connect(Some("127.0.0.1:193"))?;
        let root = conn.setup().roots[screen].root;
        ensure!(
            conn.get_geometry(root)?.reply()?.width == 3840,
            "Host screen size was not applied before startup"
        );
        {
            let b = backend.lock().unwrap();
            let root = &b.windows[&1];
            ensure!(
                root.framebuffer.pixels.len() == 4 && root.geometry.width == 3840,
                "Root pixels must be independent of logical geometry"
            );
        }
        conn.randr_query_version(1, 5)?.reply()?;
        conn.randr_select_input(
            root,
            NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE,
        )?
        .check()?;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            2752,
            20,
            928,
            1027,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::BUTTON_PRESS),
        )?
        .check()?;
        conn.map_window(window)?.check()?;
        wait(|| {
            Ok(backend
                .lock()
                .unwrap()
                .windows
                .get(&window)
                .is_some_and(|w| w.mapped))
        })?;
        conn.warp_pointer(x11rb::NONE, window, 0, 0, 0, 0, 900, 1000)?
            .check()?;
        let pointer = conn.query_pointer(window)?.reply()?;
        ensure!(
            (pointer.root_x, pointer.root_y, pointer.win_x, pointer.win_y)
                == (3652, 1020, 900, 1000),
            "Visible right/bottom pointer was clipped: {pointer:?}"
        );
        {
            let mut b = backend.lock().unwrap();
            b.push_input(InputEvent {
                kind: event::MOTION,
                window,
                x: 900.,
                y: 1000.,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::BUTTON,
                window,
                detail: 1,
                flags: 1,
                ..Default::default()
            })?;
            b.push_input(InputEvent {
                kind: event::BUTTON,
                window,
                detail: 1,
                ..Default::default()
            })?;
        }
        wait(|| {
            while let Some(e) = conn.poll_for_event()? {
                if let Event::ButtonPress(e) = e {
                    ensure!(
                        (e.event_x, e.event_y, e.root_x, e.root_y) == (900, 1000, 3652, 1020),
                        "DDX click coordinates were clipped or translated twice"
                    );
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        backend.lock().unwrap().push_input(InputEvent {
            kind: event::MOVE,
            window,
            detail: 1,
            x: -1800.,
            y: 20.,
            ..Default::default()
        })?;
        wait(|| Ok(backend.lock().unwrap().windows[&window].geometry.move_ack == 1))?;
        ensure!(
            conn.get_geometry(window)?.reply()?.x == 120,
            "Host negative coordinate did not map into root"
        );
        ensure!(
            backend.lock().unwrap().windows[&window].geometry.state.x == -1800,
            "Snapshot must retain host coordinates"
        );
        // A new display to the left changes the origin and adds a CRTC.
        for next in [
            layout(&[
                (-2560, 0, 640, 1080),
                (-1920, 0, 1920, 1080),
                (0, 0, 1920, 1080),
            ])?,
            layout(&[(0, 0, 1920, 1080)])?,
        ] {
            {
                let mut b = backend.lock().unwrap();
                b.desktop_layout = Some(next);
                b.push_input(InputEvent {
                    kind: event::DESKTOP,
                    ..Default::default()
                })?;
            }
            wait(|| Ok(backend.lock().unwrap().windows[&1].geometry.width == next.width))?;
            let geometry = conn.get_geometry(window)?.reply()?;
            ensure!(
                i32::from(geometry.x) + next.origin_x == -1800,
                "Host window moved when desktop origin changed"
            );
            let resources = conn.randr_get_screen_resources_current(root)?.reply()?;
            let connected = resources
                .outputs
                .iter()
                .map(|&id| {
                    conn.randr_get_output_info(id, resources.config_timestamp)?
                        .reply()
                })
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .filter(|output| output.connection == OutputConnection::CONNECTED)
                .count();
            ensure!(
                connected == next.count as usize,
                "Hotplug output count is incorrect"
            );
            ensure!(
                backend.lock().unwrap().windows[&1].framebuffer.pixels.len() == 4,
                "Hotplug allocated a Rust root image"
            );
        }
        ensure!(
            conn.randr_set_screen_size(root, 1280, 800, 340, 210)?
                .check()
                .is_err(),
            "Rootless client changed host-owned screen dimensions"
        );
        let before = conn
            .randr_get_screen_resources_current(root)?
            .reply()?
            .config_timestamp;
        while conn.poll_for_event()?.is_some() {}
        for seq in 2..=301 {
            backend.lock().unwrap().push_input(InputEvent {
                kind: event::MOVE,
                window,
                detail: seq,
                x: f64::from(seq),
                y: 20.,
                ..Default::default()
            })?;
            wait(|| Ok(backend.lock().unwrap().windows[&window].geometry.move_ack == seq))?;
        }
        let after = conn
            .randr_get_screen_resources_current(root)?
            .reply()?
            .config_timestamp;
        ensure!(before == after, "Dragging changed the RandR configuration");
        while let Some(event) = conn.poll_for_event()? {
            ensure!(
                !matches!(
                    event,
                    Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_)
                ),
                "Dragging emitted a RandR layout notification"
            );
        }
        println!(
            "Visible right/bottom clicks, negative host coordinates, 2→3→1 outputs, root pixels=4 bytes and 300-step zero-RandR drag passed"
        );
        Ok(())
    })();
    server.finish()?;
    result.context("Host desktop layout integration")
}
