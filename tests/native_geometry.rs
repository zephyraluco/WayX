//! Interactive real X11 client driven by the native Windows/Linux geometry harness.
use anyhow::{Context, Result, ensure};
use std::{
    io::{BufRead, Write},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
};

fn stable(conn: &RustConnection, window: u32, width: u16, height: u16, millis: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let geometry = conn.get_geometry(window)?.reply()?;
        if (geometry.width, geometry.height) == (width, height) {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "X11 geometry did not converge: {}x{}, expected {width}x{height}",
            geometry.width,
            geometry.height
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    while conn.poll_for_event()?.is_some() {}
    #[cfg(unix)]
    let host = if std::env::var_os("DISPLAY").is_some() {
        let (host, screen) = x11rb::connect(None)?;
        let root = host.setup().roots[screen].root;
        let deadline = Instant::now() + Duration::from_secs(5);
        let window = loop {
            let mut matching = None;
            let children = host.query_tree(root)?.reply()?.children;
            for id in children {
                let title = host
                    .get_property(false, id, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)?
                    .reply()?;
                if title.value == b"WayX geometry regression" {
                    let geometry = host.get_geometry(id)?.reply()?;
                    if (geometry.width, geometry.height) == (width, height) {
                        matching = Some(id);
                        break;
                    }
                }
            }
            if let Some(id) = matching {
                break id;
            }
            ensure!(
                Instant::now() < deadline,
                "Host window did not converge to {width}x{height}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        Some((host, window))
    } else {
        None
    };
    let until = Instant::now() + Duration::from_millis(millis);
    while Instant::now() < until {
        while let Some(event) = conn.poll_for_event()? {
            if let Event::ConfigureNotify(event) = event {
                ensure!(
                    (event.width, event.height) == (width, height),
                    "Passive window resized to {}x{}",
                    event.width,
                    event.height
                );
            }
        }
        let geometry = conn.get_geometry(window)?.reply()?;
        ensure!(
            (geometry.width, geometry.height) == (width, height),
            "Passive window geometry changed"
        );
        #[cfg(unix)]
        if let Some((host, window)) = &host {
            let geometry = host.get_geometry(*window)?.reply()?;
            ensure!(
                (geometry.width, geometry.height) == (width, height),
                "Native host geometry oscillated"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
#[cfg(unix)]
fn host_window() -> Result<(RustConnection, u32, u32)> {
    let (host, screen) = x11rb::connect(None)?;
    let root = host.setup().roots[screen].root;
    let children = host.query_tree(root)?.reply()?.children;
    for id in children {
        let title = host
            .get_property(false, id, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)?
            .reply()?;
        if title.value == b"WayX geometry regression" {
            return Ok((host, root, id));
        }
    }
    anyhow::bail!("No native geometry window")
}

fn stable_position(conn: &RustConnection, window: u32, x: i16, y: i16, millis: u64) -> Result<()> {
    #[cfg(unix)]
    let host = host_window()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let geometry = conn.get_geometry(window)?.reply()?;
        let matched = (geometry.x, geometry.y) == (x, y);
        #[cfg(unix)]
        let matched = {
            let native = host
                .0
                .translate_coordinates(host.2, host.1, 0, 0)?
                .reply()?;
            matched && (native.dst_x, native.dst_y) == (x, y)
        };
        if matched {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Position did not converge to {x},{y}; X is {},{}",
            geometry.x,
            geometry.y
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    while conn.poll_for_event()?.is_some() {}
    let until = Instant::now() + Duration::from_millis(millis);
    let mut notifications = 0;
    while Instant::now() < until {
        while let Some(event) = conn.poll_for_event()? {
            if let Event::ConfigureNotify(event) = event {
                ensure!(
                    (event.x, event.y) == (x, y),
                    "Position bounced to {},{}",
                    event.x,
                    event.y
                );
                notifications += 1;
            }
        }
        let geometry = conn.get_geometry(window)?.reply()?;
        ensure!(
            (geometry.x, geometry.y) == (x, y),
            "X position changed after convergence"
        );
        #[cfg(unix)]
        {
            let native = host
                .0
                .translate_coordinates(host.2, host.1, 0, 0)?
                .reply()?;
            ensure!(
                (native.dst_x, native.dst_y) == (x, y),
                "Native position changed after convergence"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ensure!(
        notifications <= 2,
        "Idle move echoes: {notifications} ConfigureNotify events"
    );
    Ok(())
}
#[test]
#[ignore = "driven by scripts/test-idle-cpu.py --geometry against a native server"]
fn native_geometry_client() -> Result<()> {
    let display: u16 = std::env::var("WAYX_GEOMETRY_DISPLAY")?.parse()?;
    let auth = std::path::PathBuf::from(std::env::var("WAYX_GEOMETRY_AUTH")?);
    let cookie = wayx::auth::xauthority_cookie(&auth, display)?;
    let stream = DefaultStream::from_tcp_stream(std::net::TcpStream::connect((
        std::net::Ipv4Addr::LOCALHOST,
        6000 + display,
    ))?)?
    .0;
    let conn = RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        cookie.to_vec(),
    )?;
    let root = conn.setup().roots[0].root;
    let window = conn.generate_id()?;
    conn.create_window(
        0,
        window,
        root,
        20,
        20,
        400,
        287,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?
    .check()?;
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"WayX geometry regression",
    )?;
    conn.map_window(window)?;
    conn.flush()?;
    println!("GEOMETRY READY");
    std::io::stdout().flush()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let args: Vec<_> = line.split_whitespace().collect();
        match args.as_slice() {
            ["check", width, height, millis] => stable(
                &conn,
                window,
                width.parse()?,
                height.parse()?,
                millis.parse()?,
            )?,
            ["remote", width, height] => {
                let width: u16 = width.parse()?;
                let height: u16 = height.parse()?;
                conn.configure_window(
                    window,
                    &ConfigureWindowAux::new()
                        .width(u32::from(width))
                        .height(u32::from(height)),
                )?
                .check()?;
                stable(&conn, window, width, height, 1000)?;
            }
            ["position", x, y, millis] => {
                stable_position(&conn, window, x.parse()?, y.parse()?, millis.parse()?)?
            }
            ["remote-position", x, y] => {
                let (x, y): (i16, i16) = (x.parse()?, y.parse()?);
                conn.configure_window(
                    window,
                    &ConfigureWindowAux::new().x(i32::from(x)).y(i32::from(y)),
                )?
                .check()?;
                stable_position(&conn, window, x, y, 1000)?;
            }
            #[cfg(unix)]
            ["local-position", x, y] => {
                let (x, y): (i16, i16) = (x.parse()?, y.parse()?);
                let (host, _, native) = host_window()?;
                for offset in 0..400 {
                    host.configure_window(
                        native,
                        &ConfigureWindowAux::new()
                            .x(i32::from(x) - 399 + offset)
                            .y(i32::from(y)),
                    )?;
                    // No WM runs under Xvfb. Supply the ICCCM root-coordinate
                    // notification that a real WM sends after moving its client.
                    let geometry = host.get_geometry(native)?.reply()?;
                    host.send_event(
                        false,
                        native,
                        EventMask::STRUCTURE_NOTIFY,
                        ConfigureNotifyEvent {
                            response_type: CONFIGURE_NOTIFY_EVENT,
                            sequence: 0,
                            event: native,
                            window: native,
                            above_sibling: 0,
                            x: x - 399 + offset as i16,
                            y,
                            width: geometry.width,
                            height: geometry.height,
                            border_width: 0,
                            override_redirect: false,
                        },
                    )?;
                }
                host.flush()?;
                stable_position(&conn, window, x, y, 1000)?;
            }
            ["border", width] => {
                conn.configure_window(
                    window,
                    &ConfigureWindowAux::new().border_width(width.parse::<u32>()?),
                )?
                .check()?;
            }
            ["hints"] => {
                let mut hints = [0u32; 18];
                hints[0] = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8);
                hints[5] = 40;
                hints[6] = 30;
                hints[7] = 1200;
                hints[8] = 800;
                hints[9] = 8;
                hints[10] = 10;
                conn.change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_NORMAL_HINTS,
                    AtomEnum::WM_SIZE_HINTS,
                    &hints,
                )?
                .check()?;
            }
            ["quit"] => break,
            _ => anyhow::bail!("Unknown geometry command: {line}"),
        }
        println!("GEOMETRY DONE");
        std::io::stdout().flush()?;
    }
    conn.destroy_window(window)?
        .check()
        .context("Destroy geometry test window")?;
    Ok(())
}
