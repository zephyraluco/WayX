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
            for id in host.query_tree(root)?.reply()?.children {
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
